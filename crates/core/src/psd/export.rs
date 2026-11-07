//! `Document` -> PSD/PSB bytes through the crate's builder.
//!
//! Our flat list maps onto the builder's groups the way the legacy writer
//! did: a depth INCREASE opens a group (named after the folder header that
//! will close it), and the header closes it. Each layer is written at its
//! tight non-blank bounds, straight 8-bit, from `display_tiles()` (the caller
//! runs `refresh_derived` first, the CODE-MAP rule).

use std::collections::HashMap;
use std::sync::Arc;

use photocraft_psd::resources::{VersionInfo, ids};
use photocraft_psd::{BlendMode, GroupSpec, LayerSpec, MaskSpec, PixelData, PsdBuilder, Rect, TaggedBlock, Version};

use super::blend::blend_to_psd;
use super::{PsdError, needs_psb};
use crate::doc::{Document, Layer, LayerMask};
use crate::export::{Background, composite_for_export};
use crate::tile::{TILE_SIZE, Tile, TileIdx};

/// Fix15 premultiplied -> straight 8-bit (the legacy writer's rounding).
pub(super) fn straight8(p: [u16; 4]) -> [u8; 4] {
    let a = p[3] as u32;
    if a == 0 {
        return [0, 0, 0, 0];
    }
    let un = |c: u16| (((c as u32 * 32768 / a).min(32768) * 255 + 16384) / 32768) as u8;
    [un(p[0]), un(p[1]), un(p[2]), ((a * 255 + 16384) / 32768) as u8]
}

fn opacity8(o: f32) -> u8 {
    (o.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// The canvas-clamped bounds `x0, y0, x1, y1` of the given tiles, or `None`.
fn bounds<'a>(size: (u32, u32), idxs: impl Iterator<Item = &'a TileIdx>) -> Option<(i32, i32, i32, i32)> {
    let t = TILE_SIZE as i32;
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for idx in idxs {
        let (ox, oy) = idx.origin();
        x0 = x0.min(ox);
        y0 = y0.min(oy);
        x1 = x1.max(ox + t);
        y1 = y1.max(oy + t);
    }
    // Our off-canvas tiles are scratch, not content.
    let (x0, y0, x1, y1) = (x0.max(0), y0.max(0), x1.min(size.0 as i32), y1.min(size.1 as i32));
    (x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
}

/// A layer's pixels at their tight non-blank bounds.
fn pixel_spec(name: &str, size: (u32, u32), tiles: &HashMap<TileIdx, Arc<Tile>>) -> LayerSpec {
    let live = || tiles.iter().filter(|(_, t)| !t.is_blank());
    let Some((x0, y0, x1, y1)) = bounds(size, live().map(|(i, _)| i)) else {
        return LayerSpec::new(name, 0, 0, 0, 0, PixelData::Rgba8(Vec::new()));
    };
    let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let mut px = vec![0u8; w * h * 4];
    for (idx, t) in live() {
        let (ox, oy) = idx.origin();
        for py in 0..TILE_SIZE {
            let y = oy + py as i32;
            if y < y0 || y >= y1 {
                continue;
            }
            for qx in 0..TILE_SIZE {
                let x = ox + qx as i32;
                if x < x0 || x >= x1 {
                    continue;
                }
                let o = ((y - y0) as usize * w + (x - x0) as usize) * 4;
                px[o..o + 4].copy_from_slice(&straight8(t.pixel(qx, py)));
            }
        }
    }
    LayerSpec::new(name, x0, y0, w as u32, h as u32, PixelData::Rgba8(px))
}

/// The user mask: coverage over the bounds of the mask's tiles, 255 where
/// a tile is absent (our rule: absent = visible, for every mask).
fn mask_spec(size: (u32, u32), m: &LayerMask) -> Option<MaskSpec> {
    let (x0, y0, x1, y1) = bounds(size, m.tiles.keys())?;
    let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let mut data = vec![255u8; w * h];
    for (idx, t) in &m.tiles {
        let (ox, oy) = idx.origin();
        for py in 0..TILE_SIZE {
            let y = oy + py as i32;
            if y < y0 || y >= y1 {
                continue;
            }
            for qx in 0..TILE_SIZE {
                let x = ox + qx as i32;
                if x < x0 || x >= x1 {
                    continue;
                }
                let a = t.pixel(qx, py)[3] as u32;
                data[(y - y0) as usize * w + (x - x0) as usize] = ((a * 255 + 16384) / 32768) as u8;
            }
        }
    }
    Some(MaskSpec { rect: Rect::from_xywh(x0, y0, w as u32, h as u32), data, default_color: 255, disabled: !m.enabled })
}

fn layer_spec(size: (u32, u32), l: &Layer) -> LayerSpec {
    let mut s = pixel_spec(&l.name, size, l.display_tiles());
    s.blend_mode = blend_to_psd(l.blend);
    s.opacity = opacity8(l.opacity);
    s.visible = l.visible;
    s.clipping = l.clip;
    s.mask = l.mask.as_ref().filter(|m| !m.tiles.is_empty()).and_then(|m| mask_spec(size, m));
    if l.lock || l.lock_alpha {
        // lspf bits: 0 transparency, 1 pixels, 2 position.
        s.extra_blocks.push(TaggedBlock::protection(if l.lock { 7 } else { 1 }));
    }
    s
}

fn group_spec(l: &Layer) -> GroupSpec {
    GroupSpec {
        name: l.name.clone(),
        blend_mode: if l.through { BlendMode::PassThrough } else { blend_to_psd(l.blend) },
        opacity: opacity8(l.opacity),
        visible: l.visible,
        open: l.open,
    }
}

pub(super) fn to_bytes(doc: &Document) -> Result<Vec<u8>, PsdError> {
    let size = doc.size;
    let mut b = PsdBuilder::new(size.0, size.1);
    if let Some(dpi) = doc.dpi {
        b = b.resolution(f64::from(dpi));
    }
    if needs_psb(size) {
        b = b.version(Version::Psb);
    }
    // One entry per builder record (layer, group divider, group folder), in
    // push order: true where the record needs the transparency-protected flag.
    let mut protect: Vec<bool> = Vec::new();

    let mut depth = 0u8;
    for (li, l) in doc.layers.iter().enumerate() {
        while l.depth > depth {
            // A group opens beneath these children; its header is the next
            // folder up at this depth.
            let header = doc.layers[li + 1..]
                .iter()
                .find(|h| h.depth == depth && h.folder)
                .ok_or_else(|| PsdError("unbalanced folder nesting".into()))?;
            b.begin_group(group_spec(header));
            protect.push(false);
            depth += 1;
        }
        if l.folder {
            if depth == l.depth {
                // An empty folder: no child opened its group.
                b.begin_group(group_spec(l));
                protect.push(false);
            }
            depth = l.depth;
            if l.is_frame() && l.display_tiles().values().any(|t| !t.is_blank()) {
                // Frame border ink rides as the group's top child: readers
                // may ignore pixels on a group record.
                let mut s = pixel_spec(&format!("{} (border)", l.name), size, l.display_tiles());
                s.visible = l.visible;
                b.push_layer(s);
                protect.push(false);
            }
            b.end_group()?;
            protect.push(false);
        } else {
            b.push_layer(layer_spec(size, l));
            protect.push(l.lock_alpha);
        }
    }
    // Groups still open at the top would desync every reader.
    if depth != 0 {
        return Err(PsdError("unbalanced folder nesting".into()));
    }

    // Drafts are excluded from the composite exactly like PNG.
    b.composite(PixelData::Rgba8(composite_for_export(doc, Background::Transparent).into_raw()));
    let mut file = b.build()?;
    for (rec, &p) in file.layers_mut().iter_mut().zip(&protect) {
        if p {
            rec.flags.set_transparency_protected(true);
        }
    }
    if let Some(r) = file.resources.iter_mut().find(|r| r.id == ids::VERSION_INFO)
        && let Ok(mut vi) = VersionInfo::from_bytes(&r.data)
    {
        vi.writer = photocraft_psd::descriptor::UnicodeString::new("MangaNakama");
        r.data = vi.to_bytes();
    }
    Ok(file.to_bytes()?)
}
