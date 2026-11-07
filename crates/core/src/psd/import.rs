//! PSD/PSB -> `Document`.
//!
//! The file's layer tree (bottom-to-top, a group = its children then its own
//! record) maps onto our flat list (bottom-first, a folder HEADER above its
//! children at `depth + 1`) by emitting each group's children before the
//! group itself. Every layer becomes a raster layer from its stored pixels
//! (8-bit straight RGBA, cropped to the canvas); what did not come across is
//! collected as notes.

use photocraft_psd::{BlendMode, BlockData, GrayImage, LayerNode, PsdFile, RgbaImage, SectionType};

use super::blend::blend_from_psd;
use super::{PsdError, PsdImport};
use crate::doc::{Blend, Document, Layer, LayerMask};
use crate::ora::{mask_from_image, paint_into_layer};
use crate::tile::TILE_SIZE;

/// Deepest folder nesting we accept. A group at nesting 250 or deeper is an
/// error rather than a silent drop of its children.
const MAX_DEPTH: usize = 250;

/// Block keys that mark a layer whose content we only keep as pixels. One
/// note per kind, with a count; the layer keeps its name.
const KINDS: [(&str, &[&[u8; 4]]); 5] = [
    ("text layer(s) came in as pixels", &[b"TySh"]),
    ("layer(s) with effects: effects dropped", &[b"lfx2", b"lrFX"]),
    (
        "adjustment, fill or shape layer(s): only their stored pixels came in",
        &[
            b"levl", b"brit", b"curv", b"expA", b"hue2", b"selc", b"blnc", b"mixr", b"grdm", b"phfl", b"clrL", b"vibA", b"SoCo",
            b"GdFl", b"PtFl",
        ],
    ),
    ("smart object(s) came in as pixels", &[b"SoLd", b"PlLd"]),
    ("vector mask(s) dropped", &[b"vmsk", b"vsms"]),
];

struct Walk<'a> {
    file: &'a PsdFile,
    canvas: (u32, u32),
    notes: Vec<String>,
    /// Per `KINDS` entry, then Blend If: how many layers had it.
    kind_counts: [usize; KINDS.len() + 1],
    /// Pixel layers whose pixels we tried to decode, and how many failed.
    decoded: usize,
    failed: usize,
    first_error: Option<String>,
}

pub(super) fn load_from_bytes(bytes: &[u8]) -> Result<PsdImport, PsdError> {
    let file = PsdFile::from_bytes(bytes)?;
    let canvas = (file.header.width, file.header.height);
    let mut walk = Walk {
        file: &file,
        canvas,
        notes: Vec::new(),
        kind_counts: [0; KINDS.len() + 1],
        decoded: 0,
        failed: 0,
        first_error: None,
    };
    let mut layers = Vec::new();
    if file.layer_info.is_some() {
        for node in file.layer_tree_max_depth(MAX_DEPTH) {
            walk.emit(&node, 0, &mut layers)?;
        }
    }
    // An all-blank "success" would be a silent failure.
    if walk.decoded > 0 && walk.failed == walk.decoded {
        return Err(PsdError(walk.first_error.take().unwrap_or_default()));
    }
    if layers.is_empty() {
        // A flat file (no layer records): its merged image is the page.
        let img = file.composite_rgba8()?;
        let mut l = Layer::new("Background");
        paint_cropped(&mut l, img, canvas);
        layers.push(l);
    }

    let mut notes = std::mem::take(&mut walk.notes);
    for (i, (what, _)) in KINDS.iter().enumerate() {
        if walk.kind_counts[i] > 0 {
            notes.push(format!("{} {what}", walk.kind_counts[i]));
        }
    }
    if walk.kind_counts[KINDS.len()] > 0 {
        notes.push(format!("{} layer(s) with Blend If ranges: ranges ignored", walk.kind_counts[KINDS.len()]));
    }

    let mut doc = Document::new(canvas.0, canvas.1);
    doc.layers = layers;
    doc.dpi = file.resolution().map(|r| (f64::from(r.h_res_fixed) / 65536.0).round() as u32).filter(|d| *d > 0);
    // Same finishing sequence as the ORA loader. PSD `lyid` values are not
    // used: ids are ours to mint.
    doc.ensure_ids();
    doc.normalize_depths();
    doc.active = doc.layers.len() - 1;
    doc.clear_history();
    doc.touch();
    Ok(PsdImport { doc, notes })
}

impl Walk<'_> {
    fn emit(&mut self, node: &LayerNode, depth: usize, out: &mut Vec<Layer>) -> Result<(), PsdError> {
        match node {
            LayerNode::Group { index, children, .. } => {
                if depth >= MAX_DEPTH {
                    return Err(PsdError(format!("folders nested deeper than {MAX_DEPTH} levels")));
                }
                // Children first: they sit below their header in our list.
                for c in children {
                    self.emit(c, depth + 1, out)?;
                }
                out.push(self.folder_layer(*index, depth));
            }
            LayerNode::Layer { index } => out.push(self.pixel_layer(*index, depth)),
        }
        Ok(())
    }

    fn blend_note(&mut self, name: &str, mode: BlendMode, note: Option<&str>) {
        if let Some(n) = note {
            match mode {
                BlendMode::Unknown(k) => self.notes.push(format!("{name}: {n} ('{}')", String::from_utf8_lossy(&k))),
                _ => self.notes.push(format!("{name}: {n}")),
            }
        }
    }

    fn pixel_layer(&mut self, index: usize, depth: usize) -> Layer {
        // Borrowed from the file, not from `self`, so the counters stay writable.
        let file = self.file;
        let rec = &file.layers()[index];
        let name = rec.name();
        let mut l = Layer::new(name.clone());
        l.depth = depth as u8;
        // Fill opacity scales only the layer's own pixels; with no effects
        // rendered, folding it into the opacity is exact.
        l.opacity = (f32::from(rec.opacity) / 255.0) * (f32::from(rec.fill_opacity()) / 255.0);
        let (blend, note) = blend_from_psd(rec.blend_mode);
        l.blend = blend;
        l.visible = rec.is_visible();
        l.clip = rec.clipping != 0;
        let lspf = match rec.block(b"lspf").and_then(|b| b.parsed()) {
            Some(Ok(BlockData::Protection(f))) => f,
            _ => 0,
        };
        l.lock_alpha = rec.flags.transparency_protected() || lspf & 1 != 0;
        l.lock = lspf & 6 == 6;
        let mode = rec.blend_mode;
        let irrelevant = rec.flags.pixel_data_irrelevant();
        let mask_disabled = rec.layer_mask().is_some_and(|m| m.disabled());
        for (i, (_, keys)) in KINDS.iter().enumerate() {
            if keys.iter().any(|k| rec.block(k).is_some()) {
                self.kind_counts[i] += 1;
            }
        }
        let full_range = rec.blending_ranges.ranges().iter().all(|r| r.source == [0, 0, 255, 255] && r.dest == [0, 0, 255, 255]);
        if !full_range {
            self.kind_counts[KINDS.len()] += 1;
        }
        self.blend_note(&name, mode, note);

        let Some(view) = file.layer(index) else { return l };
        if !irrelevant {
            self.decoded += 1;
            match view.rgba8() {
                Ok(img) => paint_cropped(&mut l, img, self.canvas),
                Err(e) => {
                    self.failed += 1;
                    self.notes.push(format!("{name}: pixels not decoded ({e})"));
                    self.first_error.get_or_insert_with(|| format!("{name}: {e}"));
                }
            }
        }
        match view.user_mask() {
            None => {}
            // A mask with no tiles hides nothing on the canvas (default 255,
            // nothing painted inside the page): the same as no mask, and the
            // exporter writes none for it, so it is not created.
            Some(Ok(g)) => l.mask = Some(mask_of(&g, !mask_disabled, self.canvas)).filter(|m| !m.tiles.is_empty()),
            Some(Err(e)) => self.notes.push(format!("{name}: mask not decoded ({e})")),
        }
        // Linked is our default; the PSD link flag is not reliably mapped.
        l.mask_linked = true;
        l
    }

    fn folder_layer(&mut self, index: usize, depth: usize) -> Layer {
        // Borrowed from the file, not from `self`, so the counters stay writable.
        let file = self.file;
        let rec = &file.layers()[index];
        let name = rec.name();
        let mut l = Layer::new(name.clone());
        l.folder = true;
        l.depth = depth as u8;
        l.open = rec.section_type() == SectionType::OpenFolder;
        // The divider block carries the group's mode; the record's own field
        // is the fallback for writers that leave it out.
        let mode = rec.section_divider().and_then(|s| s.blend_mode).unwrap_or(rec.blend_mode);
        l.through = mode == BlendMode::PassThrough;
        l.opacity = f32::from(rec.opacity) / 255.0;
        l.visible = rec.is_visible();
        l.clip = false;
        let has_mask = rec.layer_mask().is_some();
        if l.through {
            l.blend = Blend::Normal;
        } else {
            let (blend, note) = blend_from_psd(mode);
            l.blend = blend;
            self.blend_note(&name, mode, note);
        }
        // Our Layers palette cannot show or remove a folder mask; importing
        // one would be a trap. A group record's own channels are ignored.
        if has_mask {
            self.notes.push(format!("{name}: folder mask skipped"));
        }
        l
    }
}

/// Paint a decoded layer into `l`, dropping what lies off the canvas (our
/// off-canvas tiles are scratch, not content).
fn paint_cropped(l: &mut Layer, img: RgbaImage, canvas: (u32, u32)) {
    let Some((x0, y0, x1, y1)) = clip_rect(img.left, img.top, img.width, img.height, canvas) else { return };
    let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
    let inside = x0 == i64::from(img.left) && y0 == i64::from(img.top) && w == img.width && h == img.height;
    let out = if inside {
        image::RgbaImage::from_raw(w, h, img.data)
    } else {
        let src_w = img.width as usize;
        let mut buf = Vec::with_capacity(w as usize * h as usize * 4);
        for y in y0..y1 {
            let sy = (y - i64::from(img.top)) as usize;
            let sx = (x0 - i64::from(img.left)) as usize;
            let start = (sy * src_w + sx) * 4;
            buf.extend_from_slice(&img.data[start..start + w as usize * 4]);
        }
        image::RgbaImage::from_raw(w, h, buf)
    };
    if let Some(out) = out {
        paint_into_layer(l, &out, x0 as i32, y0 as i32);
    }
}

/// `left, top, width, height` intersected with the canvas, as
/// `x0, y0, x1, y1`; `None` when nothing is on the canvas.
fn clip_rect(left: i32, top: i32, width: u32, height: u32, canvas: (u32, u32)) -> Option<(i64, i64, i64, i64)> {
    let x0 = i64::from(left).max(0);
    let y0 = i64::from(top).max(0);
    let x1 = (i64::from(left) + i64::from(width)).min(i64::from(canvas.0));
    let y1 = (i64::from(top) + i64::from(height)).min(i64::from(canvas.1));
    (x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
}

/// Our mask from the file's user mask. Absent tiles read as visible in our
/// model, so a mask whose outside is shown (default 255) becomes tiles over
/// its rectangle only; any other default needs the whole canvas filled with
/// that value and the rectangle pasted in.
fn mask_of(g: &GrayImage, enabled: bool, canvas: (u32, u32)) -> LayerMask {
    let t = TILE_SIZE as i64;
    let (ox, oy, w, h, fill) = if g.default_value == 255 {
        match clip_rect(g.left, g.top, g.width, g.height, canvas) {
            // Padded out to whole tiles with the default: a partial tile
            // would otherwise read zero (hidden) outside the rectangle.
            Some((x0, y0, x1, y1)) => {
                let (ax0, ay0) = (x0.div_euclid(t) * t, y0.div_euclid(t) * t);
                let (ax1, ay1) = ((x1 + t - 1).div_euclid(t) * t, (y1 + t - 1).div_euclid(t) * t);
                (ax0, ay0, (ax1 - ax0) as u32, (ay1 - ay0) as u32, 255u8)
            }
            None => (0, 0, 0, 0, 255),
        }
    } else {
        (0, 0, canvas.0, canvas.1, g.default_value)
    };
    let mut img = image::RgbaImage::from_pixel(w, h, image::Rgba([fill; 4]));
    if let Some((x0, y0, x1, y1)) = clip_rect(g.left, g.top, g.width, g.height, canvas) {
        for y in y0..y1 {
            for x in x0..x1 {
                let si = (y - i64::from(g.top)) as usize * g.width as usize + (x - i64::from(g.left)) as usize;
                let c = g.data.get(si).copied().unwrap_or(fill);
                let (dx, dy) = (x - ox, y - oy);
                if dx >= 0 && dy >= 0 && dx < i64::from(w) && dy < i64::from(h) {
                    img.put_pixel(dx as u32, dy as u32, image::Rgba([c; 4]));
                }
            }
        }
    }
    mask_from_image(img, enabled, false, Some((ox as i32, oy as i32)), None)
}
