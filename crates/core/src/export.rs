//! CPU compositing and PNG export.
//!
//! This is the **exact** path (docs/ARCHITECTURE.md: "display may approximate
//! fix15 -> unorm with a shader scale; export/save paths convert exactly on the
//! CPU"). It walks the layer stack with the shared formulas in `core::blend`,
//! the same ones the GPU implements as fixed-function blend states.
//!
//! Work is done tile by tile: one 64x64 f32 accumulator (64 KiB) rather than a
//! full-canvas float buffer (a 2048² document would be 64 MiB, a B4/600dpi one
//! far worse).

use std::path::Path;

use crate::blend::{Rgba, blend_premul, px_to_f32, scale_opacity, to_u8, unpremultiply_u8};
use crate::doc::Document;
use crate::tile::{TILE_PIXELS, TILE_SIZE, TileIdx};

/// What sits underneath the layer stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Background {
    /// Nothing: the exported PNG keeps the document's alpha.
    Transparent,
    /// Opaque paper white — what the editor shows you, and what the GPU
    /// compositor always uses.
    #[default]
    White,
    /// Any opaque colour.
    Solid([u8; 3]),
}

impl Background {
    /// The background as a premultiplied 0..1 pixel.
    fn premul(self) -> Rgba {
        match self {
            Background::Transparent => [0.0, 0.0, 0.0, 0.0],
            Background::White => [1.0, 1.0, 1.0, 1.0],
            Background::Solid(c) => [
                c[0] as f32 / 255.0,
                c[1] as f32 / 255.0,
                c[2] as f32 / 255.0,
                1.0,
            ],
        }
    }
}

/// Composite the whole document to a straight (non-premultiplied) RGBA8 image
/// the size of the canvas.
///
/// Hidden layers and layers at zero opacity are skipped. Tiles outside the
/// canvas are ignored — the canvas is the page. This is the SCREEN composite
/// (what the editor shows): draft layers are included. Export paths use
/// [`composite_for_export`], fill sampling uses [`composite_for_fill`].
pub fn composite(doc: &Document, background: Background) -> image::RgbaImage {
    composite_size(
        doc,
        background,
        doc.size.0.max(1),
        doc.size.1.max(1),
        0,
        0,
        CompOpts::Screen,
    )
}

/// The PNG/export composite: draft layers (CSP 下書き, cascading through
/// folders) are excluded — a draft shows on screen but never prints.
pub fn composite_for_export(doc: &Document, background: Background) -> image::RgbaImage {
    composite_size(
        doc,
        background,
        doc.size.0.max(1),
        doc.size.1.max(1),
        0,
        0,
        CompOpts::Export,
    )
}

/// The fill/wand sampling composite: drafts excluded unless `refer_drafts`,
/// and the reference layer is sampled even when its own eye is off (CSP
/// 参照レイヤー — keep roughs hidden, fill against them). Ancestors' eyes
/// still gate it.
pub fn composite_for_fill(
    doc: &Document,
    background: Background,
    refer_drafts: bool,
) -> image::RgbaImage {
    composite_size(
        doc,
        background,
        doc.size.0.max(1),
        doc.size.1.max(1),
        0,
        0,
        CompOpts::Fill { refer_drafts },
    )
}

/// The fill's OPACITY canvas (row 40/120, owner verdict 2026-08-27): the
/// same layer walk as [`composite_for_fill`] — drafts per the flag, the
/// reference set forced in — composited over TRANSPARENT so the alpha
/// channel carries the art's true merged coverage instead of the white
/// paper's 255. RGB channels are premultiplied-then-unpremultiplied and
/// carry no meaning here; callers read alpha only.
pub fn composite_alpha_for_fill(doc: &Document, refer_drafts: bool) -> Vec<u8> {
    composite_size(
        doc,
        Background::Transparent,
        doc.size.0.max(1),
        doc.size.1.max(1),
        0,
        0,
        CompOpts::Fill { refer_drafts },
    )
    .pixels()
    .map(|p| p.0[3])
    .collect()
}

/// Row 105: an arbitrary sub-rect of the EXPORT composite (drafts out) —
/// the correction layer's derivation source. Kept `pub(crate)` so the
/// correction module reuses the one true walk instead of growing a second
/// compositor that could disagree.
pub(crate) fn composite_rect_export(
    doc: &Document,
    background: Background,
    w: u32,
    h: u32,
    ox: i32,
    oy: i32,
) -> image::RgbaImage {
    composite_size(doc, background, w, h, ox, oy, CompOpts::Export)
}

/// Which layers a composite walks.
#[derive(Clone, Copy)]
enum CompOpts {
    /// Visible layers, drafts included — what the editor shows.
    Screen,
    /// Visible non-draft layers — the printed/exported page.
    Export,
    /// Fill sampling: drafts unless `refer_drafts`, reference layer forced in.
    Fill { refer_drafts: bool },
}

impl CompOpts {
    fn skip_drafts(self) -> bool {
        match self {
            CompOpts::Screen => false,
            CompOpts::Export => true,
            CompOpts::Fill { refer_drafts } => !refer_drafts,
        }
    }

    fn force_reference_visible(self) -> bool {
        matches!(self, CompOpts::Fill { .. })
    }

    /// LP-022: does this composite apply the decrease-colour PREVIEW?
    ///
    /// Screen only, and that asymmetry is the whole feature. The layer
    /// colour (LP-016/LP-017) is a real rendering property and prints; the
    /// expression preview is a question you asked the screen ("what would
    /// this look like at 1-bit?") and must not reach the exported page or
    /// the colour the fill tool samples.
    fn preview_expression(self) -> bool {
        matches!(self, CompOpts::Screen)
    }
}

/// The displayed colour at one canvas pixel (straight RGB over paper white) —
/// the eyedropper. Costs one tile's compositing walk, not the whole canvas.
pub fn composite_pixel(doc: &Document, x: i32, y: i32) -> Option<[u8; 3]> {
    if x < 0 || y < 0 || x as u32 >= doc.size.0 || y as u32 >= doc.size.1 {
        return None;
    }
    let img = composite_size(doc, Background::White, 1, 1, x, y, CompOpts::Screen);
    let p = img.get_pixel(0, 0).0;
    Some([p[0], p[1], p[2]])
}

/// The box an eyedropper of side `n` covers around `(x, y)`, clipped to the
/// canvas: `(x0, y0, w, h)`. `None` when the pick itself is off-canvas.
///
/// Odd `n` centres on the pixel. Even `n` cannot — there is no half pixel to
/// centre on — so it leans down-right, which is also where CSP's 2×2 lands.
/// Near an edge the box is CLIPPED rather than slid inward: averaging in
/// paper white that is not on the canvas would tint every edge pick.
///
/// One definition, shared by all three reference modes (`composite_pixel_avg`
/// here and the app's layer/reference samplers) — if they disagreed about the
/// box, "3×3" would mean three things.
pub fn sample_box(size: (u32, u32), x: i32, y: i32, n: u32) -> Option<(i32, i32, u32, u32)> {
    if x < 0 || y < 0 || x as u32 >= size.0 || y as u32 >= size.1 {
        return None;
    }
    let n = n.clamp(1, 64) as i32;
    let back = (n - 1) / 2;
    let x0 = (x - back).max(0);
    let y0 = (y - back).max(0);
    let x1 = (x - back + n - 1).min(size.0 as i32 - 1);
    let y1 = (y - back + n - 1).min(size.1 as i32 - 1);
    Some((x0, y0, (x1 - x0 + 1) as u32, (y1 - y0 + 1) as u32))
}

/// sRGB EOTF (encoded → linear light), byte for byte the same curve as
/// `downsample.wgsl`.
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Average opaque straight-RGB samples **IN LINEAR LIGHT**.
///
/// WHY NOT THE BYTES. The canvas holds display-encoded values, so the mean of
/// the bytes is not the mean of the light: equal parts black and white average
/// to 128 that way, when half the light actually looks like ~188. A manga page
/// is black ink on white paper and almost nothing else, which is exactly the
/// content that suffers. The mip chain already learned this the hard way
/// (`downsample.wgsl`, owner report 2026-08-20: our zoomed-out linework read
/// harsh and chunky next to CSP's) — so the eyedropper's average uses the same
/// curve, and a 5×5 pick AGREES with what the zoomed-out view shows for that
/// patch instead of contradicting it. (Not bit-identical: the mip chain is a
/// chain of 2×2 boxes plus trilinear filtering, this is one box. Same rule,
/// same neighbourhood — and two different answers to "what does this area look
/// like" would be one too many.)
///
/// A single sample is returned VERBATIM: the transfer round-trip is inside a
/// quantization step but not provably the identity, and the 1×1 default has to
/// pick exactly the byte it picks today.
pub fn average_srgb(samples: &[[u8; 3]]) -> Option<[u8; 3]> {
    match samples {
        [] => None,
        [one] => Some(*one),
        _ => {
            let mut acc = [0.0f32; 3];
            for s in samples {
                for c in 0..3 {
                    acc[c] += srgb_to_linear(s[c] as f32 / 255.0);
                }
            }
            let n = samples.len() as f32;
            Some(std::array::from_fn(|c| {
                (linear_to_srgb(acc[c] / n) * 255.0)
                    .round()
                    .clamp(0.0, 255.0) as u8
            }))
        }
    }
}

/// The eyedropper's sample: the displayed colour over the `n`×`n` box around
/// `(x, y)`, averaged in linear light. `n == 1` is [`composite_pixel`] exactly.
///
/// One compositing walk for the whole box (at most 2×2 tiles for the sizes the
/// UI offers), never one per pixel.
pub fn composite_pixel_avg(doc: &Document, x: i32, y: i32, n: u32) -> Option<[u8; 3]> {
    let (x0, y0, w, h) = sample_box(doc.size, x, y, n)?;
    let img = composite_size(doc, Background::White, w, h, x0, y0, CompOpts::Screen);
    let px: Vec<[u8; 3]> = img.pixels().map(|p| [p.0[0], p.0[1], p.0[2]]).collect();
    average_srgb(&px)
}

/// Composite an arbitrary canvas-space rect. `(ox, oy)` is the canvas pixel that
/// lands at image pixel (0, 0). Used by ORA save to crop a layer to its tiles.
///
/// # Folder isolation (true group clipping)
///
/// Layers at depth `d` composite into accumulator `d` (0 = the page). When
/// the walk reaches a folder header at depth `d`, accumulator `d + 1` holds
/// its children's isolated composite: a frame folder multiplies it by the
/// panel coverage mask, then the group blends onto accumulator `d` with the
/// folder's opacity and blend mode, then the header's own raster (the border
/// ink) draws on top, and the child accumulator resets. Clip layers multiply
/// their source by the base layer's alpha before blending — no extra buffer.
fn composite_size(
    doc: &Document,
    background: Background,
    w: u32,
    h: u32,
    ox: i32,
    oy: i32,
    opts: CompOpts,
) -> image::RgbaImage {
    let mut img = image::RgbaImage::new(w, h);
    let bg = background.premul();
    let mut eff = doc.effective_visibility();
    if opts.force_reference_visible() {
        // The whole reference SET (RF-001), not just the topmost.
        for ri in doc.reference_layers() {
            eff[ri] = true;
        }
    }
    if opts.skip_drafts() {
        let drafts = doc.effective_drafts();
        for (e, d) in eff.iter_mut().zip(&drafts) {
            if *d {
                *e = false;
            }
        }
    }
    let bases = doc.clip_bases();
    // Clip-to-folder: folder headers serving as a clip base, and the group
    // alpha captured for the CURRENT tile at each such folder's close (the
    // accumulator is consumed there). Captured after the frame mask — panel
    // coverage is part of a frame folder's ink — and before opacity/blend,
    // the raw-display-alpha rule layer bases follow. A hidden folder never
    // captures (its children are not walked): a missing entry = zero ink.
    let folder_base: Vec<bool> = {
        let mut fb = vec![false; doc.layers.len()];
        for b in bases.iter().flatten() {
            if doc.layers[*b].folder {
                fb[*b] = true;
            }
        }
        fb
    };
    let mut folder_alpha: std::collections::HashMap<usize, Vec<f32>> = Default::default();
    let max_depth = doc
        .layers
        .iter()
        .map(|l| l.depth as usize)
        .max()
        .unwrap_or(0);
    // LF-002 Through: real depth → effective accumulator depth. A
    // through-folder maps its child depth onto its OWN effective depth
    // (children blend as if loose); a normal folder maps it one deeper
    // (the sealed group). The sequential walk keeps sibling folders
    // independent — each header re-maps the depth below it before its
    // children are reached.
    let mut collapse: Vec<usize> = (0..=max_depth + 1).collect();
    for l in &doc.layers {
        if l.folder {
            let e = collapse[l.depth as usize];
            collapse[l.depth as usize + 1] = if l.through { e } else { e + 1 };
        }
    }
    let mut accs: Vec<Vec<Rgba>> = (0..=max_depth + 1)
        .map(|_| vec![[0.0f32; 4]; TILE_PIXELS])
        .collect();
    let order = doc.composite_order();

    let t = TILE_SIZE as i32;
    let tx0 = ox.div_euclid(t);
    let ty0 = oy.div_euclid(t);
    let tx1 = (ox + w as i32 - 1).div_euclid(t);
    let ty1 = (oy + h as i32 - 1).div_euclid(t);

    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            let idx = TileIdx::new(tx, ty);

            // Does any visible layer have this tile?
            let touched = doc
                .layers
                .iter()
                .zip(&eff)
                .any(|(l, vis)| *vis && l.opacity > 0.0 && l.display_tile(idx).is_some());

            accs[0].fill(bg);
            for a in accs.iter_mut().skip(1) {
                a.fill([0.0; 4]);
            }
            folder_alpha.clear();
            if touched {
                // FB-overflow: escaped layers re-seat above their frame
                // folder header — `order` is the shared walk, `ed` the
                // effective depth (the header's own, for an escapee).
                for &(li, ed) in &order {
                    let layer = &doc.layers[li];
                    if !eff[li] {
                        continue;
                    }
                    let d = ed as usize;
                    // LF-002 Through: a through-folder's children collapse
                    // onto the folder's own effective accumulator.
                    let cd = collapse[d];
                    if layer.folder {
                        if layer.through {
                            // The seal is removed: no group close, no group
                            // blend, no frame-mask clip. The header's own
                            // raster (border ink) still draws at its depth.
                            if layer.opacity > 0.0
                                && let Some(tile) = layer.tile(idx)
                            {
                                let data = tile.data();
                                for (p, dst) in accs[cd].iter_mut().enumerate() {
                                    let o = p * 4;
                                    let s = scale_opacity(
                                        px_to_f32([data[o], data[o + 1], data[o + 2], data[o + 3]]),
                                        layer.opacity,
                                    );
                                    if s[3] <= 0.0 {
                                        continue;
                                    }
                                    *dst = blend_premul(crate::doc::Blend::Normal, s, *dst);
                                }
                            }
                            continue;
                        }
                        let lvl = cd + 1;
                        // 0. FB-knockout: a plain folder's derived mat (the
                        // border effect grown from the union of children
                        // ink) lies on the page just beneath the group,
                        // scaled by the folder's opacity. Frame folders
                        // never carry one (set_edge refuses them).
                        if layer.edge.is_some() && layer.opacity > 0.0 {
                            if let Some(mt) = layer.edge_tiles().and_then(|m| m.get(&idx)) {
                                let data = mt.data();
                                for (p, dst) in accs[cd].iter_mut().enumerate() {
                                    let o = p * 4;
                                    let s = scale_opacity(
                                        px_to_f32([data[o], data[o + 1], data[o + 2], data[o + 3]]),
                                        layer.opacity,
                                    );
                                    if s[3] <= 0.0 {
                                        continue;
                                    }
                                    *dst = blend_premul(crate::doc::Blend::Normal, s, *dst);
                                }
                            }
                        }
                        // 1. Clip the group to the panels (frame folders).
                        if let Some(mask) = layer.mask_tiles() {
                            let cov = mask.get(&idx);
                            for (p, slot) in accs[lvl].iter_mut().enumerate() {
                                let m = cov
                                    .map(|mt| mt.data()[p * 4 + 3] as f32 / 32768.0)
                                    .unwrap_or(0.0);
                                for c in slot.iter_mut() {
                                    *c *= m;
                                }
                            }
                        }
                        // 1½. Clip-to-folder: someone above clips to this
                        // group — its alpha dies at step 3, so capture now.
                        if folder_base[li] {
                            folder_alpha.insert(li, accs[lvl].iter().map(|p| p[3]).collect());
                        }
                        // 2. Blend the isolated group, then the border ink.
                        if layer.opacity > 0.0 {
                            let (group, target) = split_two(&mut accs, lvl, cd);
                            for (src, dst) in group.iter().zip(target.iter_mut()) {
                                let s = scale_opacity(*src, layer.opacity);
                                if s == [0.0; 4] {
                                    continue;
                                }
                                *dst = blend_premul(layer.blend, s, *dst);
                            }
                            if let Some(tile) = layer.tile(idx) {
                                let data = tile.data();
                                for (p, dst) in target.iter_mut().enumerate() {
                                    let o = p * 4;
                                    let s = scale_opacity(
                                        px_to_f32([data[o], data[o + 1], data[o + 2], data[o + 3]]),
                                        layer.opacity,
                                    );
                                    if s[3] <= 0.0 {
                                        continue;
                                    }
                                    *dst = blend_premul(crate::doc::Blend::Normal, s, *dst);
                                }
                            }
                        }
                        // 3. The group is consumed; a later folder reuses it.
                        accs[lvl].fill([0.0; 4]);
                        continue;
                    }

                    if layer.opacity <= 0.0 {
                        continue;
                    }
                    let Some(tile) = layer.display_tile(idx) else {
                        continue;
                    };
                    // Clip-to-folder: a folder base's alpha comes from the
                    // capture above, never from the header's own raster
                    // (that is only the border ink).
                    let base_folder = bases[li].filter(|&b| doc.layers[b].folder);
                    let base_alpha = base_folder.and_then(|b| folder_alpha.get(&b));
                    let base_tile = match base_folder {
                        Some(_) => None,
                        None => bases[li].and_then(|b| doc.layers[b].display_tile(idx)),
                    };
                    let clipped = bases[li].is_some();
                    let data = tile.data();
                    let tint = layer.layer_colour;
                    let sub = layer.layer_sub_colour;
                    let expr = if opts.preview_expression() {
                        layer.expression
                    } else {
                        crate::doc::LayerExpression::Colour
                    };
                    // LM-005: the layer mask scales the SOURCE alpha (coverage
                    // in the mask tile's alpha; an ABSENT tile = unmasked,
                    // i.e. visible — `mask_cov` stays None below and nothing
                    // scales). The GPU compositor agrees (gpu/lib.rs).
                    let mask_cov = layer
                        .mask
                        .as_ref()
                        .filter(|m| m.enabled)
                        .and_then(|m| m.tiles.get(&idx))
                        .map(|mt| mt.data());
                    for (i, slot) in accs[cd].iter_mut().enumerate() {
                        let o = i * 4;
                        // LP-016/017/022: the per-layer display maths apply to
                        // the SOURCE ink before opacity/clipping/blending —
                        // the same point, in the same order, as the GPU
                        // shader (tiles.wgsl / blend2.wgsl). The expression
                        // reduce runs FIRST so that mono + a two-tone pair is
                        // a real two-colour layer rather than a thresholded
                        // ramp.
                        let mut base_px = [data[o], data[o + 1], data[o + 2], data[o + 3]];
                        base_px = crate::blend::expression_reduce(base_px, expr);
                        if let Some(t) = tint {
                            base_px = crate::blend::layer_colour_tint(base_px, t, sub);
                        }
                        let mut src = scale_opacity(px_to_f32(base_px), layer.opacity);
                        if let Some(md) = mask_cov {
                            let m = md[i * 4 + 3] as f32 / 32768.0;
                            for c in src.iter_mut() {
                                *c *= m;
                            }
                        }
                        if clipped {
                            let m = match base_alpha {
                                Some(fa) => fa[i],
                                None => base_tile
                                    .map(|bt| bt.data()[o + 3] as f32 / 32768.0)
                                    .unwrap_or(0.0),
                            };
                            for c in src.iter_mut() {
                                *c *= m;
                            }
                        }
                        if src[3] <= 0.0 && src[0] <= 0.0 && src[1] <= 0.0 && src[2] <= 0.0 {
                            continue; // fully transparent source: no-op in every mode
                        }
                        *slot = blend_premul(layer.blend, src, *slot);
                    }
                }
            }

            // Blit the accumulator into the image, clipped.
            let acc = &accs[0];
            let (px0, py0) = idx.origin();
            for ly in 0..TILE_SIZE {
                let iy = py0 + ly as i32 - oy;
                if iy < 0 || iy >= h as i32 {
                    continue;
                }
                for lx in 0..TILE_SIZE {
                    let ix = px0 + lx as i32 - ox;
                    if ix < 0 || ix >= w as i32 {
                        continue;
                    }
                    let p = acc[ly * TILE_SIZE + lx];
                    let out = match background {
                        // Opaque background: alpha is 1 by construction, so skip
                        // the divide and keep the channels exact.
                        Background::Transparent => unpremultiply_u8(p),
                        _ => [to_u8(p[0]), to_u8(p[1]), to_u8(p[2]), 255],
                    };
                    img.put_pixel(ix as u32, iy as u32, image::Rgba(out));
                }
            }
        }
    }
    img
}

/// Two disjoint `&mut` accumulators out of the stack (`hi > lo`).
fn split_two(accs: &mut [Vec<Rgba>], hi: usize, lo: usize) -> (&[Rgba], &mut [Rgba]) {
    debug_assert!(hi > lo);
    let (a, b) = accs.split_at_mut(hi);
    (&b[0], &mut a[lo])
}

/// One layer on its own, cropped to its tile bounding box.
///
/// Returns the image plus the canvas-space `(x, y)` offset of its top-left
/// corner. `None` when the layer has no tiles. Layer opacity/blend are **not**
/// baked in — they are stored as ORA attributes.
pub fn layer_image(layer: &crate::doc::Layer) -> Option<(image::RgbaImage, i32, i32)> {
    let (x, y, w, h) = layer.tile_bounds()?;
    let mut img = image::RgbaImage::new(w, h);
    for (idx, tile) in layer.tiles() {
        let (px0, py0) = idx.origin();
        let data = tile.data();
        for ly in 0..TILE_SIZE {
            let iy = py0 + ly as i32 - y;
            if iy < 0 || iy >= h as i32 {
                continue;
            }
            for lx in 0..TILE_SIZE {
                let ix = px0 + lx as i32 - x;
                if ix < 0 || ix >= w as i32 {
                    continue;
                }
                let o = (ly * TILE_SIZE + lx) * 4;
                let straight =
                    unpremultiply_u8(px_to_f32([data[o], data[o + 1], data[o + 2], data[o + 3]]));
                img.put_pixel(ix as u32, iy as u32, image::Rgba(straight));
            }
        }
    }
    Some((img, x, y))
}

/// Composite and write a PNG — the EXPORT composite (draft layers excluded).
pub fn save_png(doc: &Document, path: &Path, background: Background) -> image::ImageResult<()> {
    composite_for_export(doc, background).save(path)
}

// --- print finishing ---------------------------------------------------------

/// The finishing decisions a submission target fixes: output resolution and
/// expression colour. `dpi == 0` means "the work's own resolution, no
/// resample" — the same `0 = no dpi` convention `PageSetup::dpi` uses.
///
/// Whether a spread leaves as two files lives on the export dialog
/// (`export_all_split`) and is NOT duplicated here: one value, one home.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExportFinish {
    pub dpi: u32,
    pub colour: crate::doc::LayerExpression,
    pub split_spreads: bool,
}

impl Default for ExportFinish {
    /// Today's untouched Export All Pages run, byte for byte.
    fn default() -> Self {
        Self {
            dpi: 0,
            colour: crate::doc::LayerExpression::Colour,
            split_spreads: false,
        }
    }
}

/// A named finish. `note` is the hover text — it carries WHY the numbers
/// are these numbers, because a preset whose provenance is invisible gets
/// second-guessed by every user who knows the norm.
pub struct ExportPreset {
    pub name: &'static str,
    pub note: &'static str,
    pub finish: ExportFinish,
}

/// The built-in print finishes.
///
/// The resolutions are the Japanese submission pair every magazine and
/// doujin printer states: **monochrome 1-bit at 600 dpi, greyscale and
/// colour at 350 dpi**. B4 vs B5 vs A4 is the WORK's page setup
/// (`PageSetup::presets`), not an export decision — without a crop-to-trim
/// the paper size changes nothing here, so naming a preset "B4" would be
/// a lie with a number in it.
///
/// Every entry must be a DISTINCT triple: the picker reads the current
/// draft back to find its name (`matching_preset`), so two presets with
/// the same values would make one of them unreachable.
pub const PRINT_PRESETS: &[ExportPreset] = &[
    ExportPreset {
        name: "Print mono 600 dpi (モノクロ二値)",
        note: "commercial manuscript and doujinshi interiors: 1-bit at 600 dpi, \
               spreads split into single pages",
        finish: ExportFinish {
            dpi: 600,
            colour: crate::doc::LayerExpression::Mono,
            split_spreads: true,
        },
    },
    ExportPreset {
        name: "Print grey 350 dpi (グレースケール)",
        note: "the greyscale half of the same submission spec — tone-free \
               shading, 350 dpi, spreads split",
        finish: ExportFinish {
            dpi: 350,
            colour: crate::doc::LayerExpression::Grey,
            split_spreads: true,
        },
    },
    ExportPreset {
        name: "Print colour 350 dpi (カラー)",
        note: "colour pages and covers at the Japanese print norm; 300 dpi is \
               the western print-on-demand equivalent",
        finish: ExportFinish {
            dpi: 350,
            colour: crate::doc::LayerExpression::Colour,
            split_spreads: true,
        },
    },
    ExportPreset {
        name: "Web full colour 150 dpi",
        note: "screen delivery: full colour, roughly 1000–1500 px on the short \
               edge, and a spread stays ONE image",
        finish: ExportFinish {
            dpi: 150,
            colour: crate::doc::LayerExpression::Colour,
            split_spreads: false,
        },
    },
];

/// Which built-in a draft currently spells, or `None` for "Custom".
///
/// The picker DERIVES its selection instead of storing an index, so
/// editing any control flips it to Custom with no bookkeeping and no
/// stale-index door to miss.
pub fn matching_preset(finish: ExportFinish) -> Option<usize> {
    PRINT_PRESETS.iter().position(|p| p.finish == finish)
}

/// The resample factor for a work at `work_dpi` finishing at `out_dpi`.
///
/// **Never above 1.0.** Upsampling to hit a printer's number invents
/// detail that is not in the page; the honest answer to "600 dpi from a
/// 350 dpi work" is 350 dpi, said out loud in the dialog.
pub fn finish_scale(out_dpi: u32, work_dpi: Option<u32>) -> f32 {
    match (out_dpi, work_dpi) {
        (0, _) | (_, None) => 1.0,
        (out, Some(work)) if work > 0 => (out as f32 / work as f32).min(1.0),
        _ => 1.0,
    }
}

/// One straight RGBA8 pixel reduced to grey or 1-bit, by the same rules
/// `blend::expression_reduce` applies to premultiplied fix15 — the mean of
/// the channels as the value, a 50 % threshold on BOTH value and alpha.
/// (Pinned against it by `u8_reduce_agrees_with_the_fix15_preview`.)
fn reduce_u8(p: [u8; 4], e: crate::doc::LayerExpression) -> [u8; 4] {
    use crate::doc::LayerExpression as E;
    if e == E::Colour {
        return p;
    }
    let sum = p[0] as u32 + p[1] as u32 + p[2] as u32;
    match e {
        E::Colour => p,
        E::Grey => {
            let v = ((sum + 1) / 3) as u8;
            [v, v, v, p[3]]
        }
        E::Mono => {
            let a = if p[3] >= 128 { 255 } else { 0 };
            let v = if sum * 2 >= 255 * 3 { a } else { 0 };
            [v, v, v, a]
        }
    }
}

/// Apply a finish to one composited page image.
///
/// **Resample first, reduce colour last.** A 1-bit threshold taken before
/// the downscale is immediately averaged back into grey by the filter, so
/// the file that reaches the printer is not 1-bit at all — the order here
/// is the whole correctness of the mono preset.
/// What rectangle of the page an export writes. `Paper` is every export
/// before profiles existed, byte for byte; the other two are the crop the
/// PRINT_PRESETS comment always promised ("without a crop-to-trim the paper
/// size changes nothing here").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ExportCrop {
    /// The whole canvas (paper), as always.
    #[default]
    Paper,
    /// Trim + bleed — what a printer wants on the plate.
    TrimBleed,
    /// Trim only — what a reader sees; the web target.
    Trim,
}

/// The crop rectangle in page pixels, `[x0, y0, x1, y1]`, for a document
/// `doc_px` big under `setup`. Handles SPREADS: a canvas materially wider
/// than the setup's paper is two pages side by side, so the rect keeps the
/// left page's left inset and the right page's right inset (the middle is
/// the fold — nothing to cut there). Degenerate setups (zero trim — the
/// pixel-canvas presets) and crops that would leave nothing fall back to
/// the full canvas rather than emit an empty file.
pub fn crop_rect_px(
    setup: &crate::page::PageSetup,
    doc_px: (u32, u32),
    crop: ExportCrop,
) -> [u32; 4] {
    let full = [0, 0, doc_px.0, doc_px.1];
    if crop == ExportCrop::Paper {
        return full;
    }
    let r = match crop {
        ExportCrop::Trim => setup.trim_rect_px(),
        _ => setup.bleed_rect_px(),
    };
    let [x0, y0, x1, y1] = r;
    if !(x1 > x0 && y1 > y0) {
        return full;
    }
    let (pw, _) = setup.paper_px();
    let (dw, dh) = (doc_px.0 as f32, doc_px.1 as f32);
    // Spread test mirrors the export loop's width heuristic: same evidence,
    // same verdict, or the crop would disagree with the split decision.
    let spread = doc_px.0 as f32 > pw as f32 * 1.5;
    let x1 = if spread { dw - (pw as f32 - x1) } else { x1 };
    [
        (x0.max(0.0)) as u32,
        (y0.max(0.0)) as u32,
        (x1.min(dw).round()) as u32,
        (y1.min(dh).round()) as u32,
    ]
}

/// `finish_image` with the profile-era knobs: crop first (crop is in PAGE
/// pixels, so it must precede any resample), then dpi scale, then the
/// exact-height fit (`px_height` wins over dpi when both are set — a web
/// target speced in pixels means those pixels), then colour reduction.
/// Neither path ever upsamples.
pub fn finish_image_cropped(
    img: image::RgbaImage,
    crop_px: [u32; 4],
    scale: f32,
    px_height: u32,
    colour: crate::doc::LayerExpression,
) -> image::RgbaImage {
    let [x0, y0, x1, y1] = crop_px;
    let img = if x1 > x0 && y1 > y0 && (x1 - x0 < img.width() || y1 - y0 < img.height()) {
        image::imageops::crop_imm(
            &img,
            x0,
            y0,
            (x1 - x0).min(img.width() - x0),
            (y1 - y0).min(img.height() - y0),
        )
        .to_image()
    } else {
        img
    };
    if px_height > 0 && px_height < img.height() {
        // Exact-height fit, resized HERE so the output height is the asked
        // number, not a rounding neighbour of it. Never up: a 1200px-tall
        // crop asked for 2048 stays 1200 (the dialog says so, not us).
        let w =
            ((img.width() as f32 * px_height as f32 / img.height() as f32).round() as u32).max(1);
        let img =
            image::imageops::resize(&img, w, px_height, image::imageops::FilterType::Lanczos3);
        return finish_image(img, 1.0, colour);
    }
    finish_image(img, scale, colour)
}

pub fn finish_image(
    img: image::RgbaImage,
    scale: f32,
    colour: crate::doc::LayerExpression,
) -> image::RgbaImage {
    let mut img = if scale < 1.0 {
        let w = ((img.width() as f32 * scale).round() as u32).max(1);
        let h = ((img.height() as f32 * scale).round() as u32).max(1);
        image::imageops::resize(&img, w, h, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };
    if colour != crate::doc::LayerExpression::Colour {
        for px in img.pixels_mut() {
            px.0 = reduce_u8(px.0, colour);
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::f32_to_fix15;
    use crate::doc::Blend;

    /// Fill one tile of `layer` with a straight colour at the given alpha.
    fn fill_tile(doc: &mut Document, layer: usize, idx: TileIdx, rgba: [f32; 4]) {
        let premul = [
            f32_to_fix15(rgba[0] * rgba[3]),
            f32_to_fix15(rgba[1] * rgba[3]),
            f32_to_fix15(rgba[2] * rgba[3]),
            f32_to_fix15(rgba[3]),
        ];
        let tile = doc.layers[layer].tile_mut(idx);
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                tile.set_pixel(x, y, premul);
            }
        }
    }

    #[test]
    fn drafts_show_on_screen_but_not_in_export() {
        let mut doc = Document::new(128, 128);
        fill_tile(&mut doc, 0, TileIdx::new(0, 0), [1.0, 0.0, 0.0, 1.0]);
        doc.set_layer_draft(0, true);

        let screen = composite(&doc, Background::White);
        let printed = composite_for_export(&doc, Background::White);
        assert_eq!(screen.get_pixel(5, 5).0[1], 0, "screen shows the draft");
        assert_eq!(printed.get_pixel(5, 5).0[1], 255, "no draft: paper white");

        // Refer-drafts toggles the fill sampler only.
        let sampled = composite_for_fill(&doc, Background::White, true);
        assert_eq!(sampled.get_pixel(5, 5).0[1], 0, "opted in");
        let skipped = composite_for_fill(&doc, Background::White, false);
        assert_eq!(skipped.get_pixel(5, 5).0[1], 255, "opted out");
    }

    #[test]
    fn fill_sampling_sees_a_hidden_reference_layer() {
        let mut doc = Document::new(128, 128);
        fill_tile(&mut doc, 0, TileIdx::new(0, 0), [0.0, 0.0, 1.0, 1.0]);
        doc.set_layer_reference(0, true);
        doc.set_layer_visible(0, false);

        let sampled = composite_for_fill(&doc, Background::White, true);
        assert_eq!(sampled.get_pixel(5, 5).0[0], 0, "hidden reference sampled");
        let screen = composite(&doc, Background::White);
        assert_eq!(screen.get_pixel(5, 5).0[0], 255, "screen honours the eye");
    }

    /// E-016. The box is one shared definition: odd sizes centre, even sizes
    /// lean down-right, and the canvas edge CLIPS instead of sliding the box
    /// back inward (sliding would average in pixels the pen is not over).
    #[test]
    fn the_average_box_centres_on_odd_and_clips_at_the_edge() {
        let size = (100, 100);
        assert_eq!(sample_box(size, 50, 50, 1), Some((50, 50, 1, 1)));
        assert_eq!(sample_box(size, 50, 50, 3), Some((49, 49, 3, 3)));
        assert_eq!(sample_box(size, 50, 50, 5), Some((48, 48, 5, 5)));
        // Even: no half pixel to centre on, so it takes the pixel and its
        // down-right neighbour.
        assert_eq!(sample_box(size, 50, 50, 2), Some((50, 50, 2, 2)));
        // Corners clip to what is actually on the canvas.
        assert_eq!(sample_box(size, 0, 0, 5), Some((0, 0, 3, 3)));
        assert_eq!(sample_box(size, 99, 99, 3), Some((98, 98, 2, 2)));
        // Off-canvas picks stay None — the eyedropper's "outside the canvas".
        assert_eq!(sample_box(size, -1, 50, 3), None);
        assert_eq!(sample_box(size, 100, 50, 3), None);
    }

    /// The colour-space call, stated as a test: equal parts black and white
    /// average to ~188 (half the LIGHT), never to the byte mean of 128. Same
    /// curve as the mip downsample, so a 5×5 pick agrees with the zoomed-out
    /// view.
    #[test]
    fn averaging_is_in_linear_light_not_in_bytes() {
        let avg = average_srgb(&[[0, 0, 0], [255, 255, 255]]).unwrap();
        for c in avg {
            assert!(
                (185..=191).contains(&c),
                "half black + half white must read ~188, got {c} (128 = averaged the bytes)"
            );
        }
        // A single sample is the pixel itself, byte for byte — the 1×1 default
        // must not drift through a transfer round-trip.
        for v in 0..=255u8 {
            assert_eq!(average_srgb(&[[v, v, v]]), Some([v, v, v]));
        }
        assert_eq!(average_srgb(&[]), None);
        // Equal samples average to themselves.
        assert_eq!(average_srgb(&[[40, 90, 200]; 9]), Some([40, 90, 200]));
    }

    /// E-016 end to end: the 1×1 default is `composite_pixel` exactly, and a
    /// 2×2 over the ink/paper boundary returns the grey the area reads as.
    #[test]
    fn the_eyedroppers_average_matches_one_pixel_at_size_one() {
        let mut doc = Document::new(128, 128);
        let idx = TileIdx::new(0, 0);
        let black = [0, 0, 0, f32_to_fix15(1.0)];
        {
            let tile = doc.layers[0].tile_mut(idx);
            tile.set_pixel(10, 10, black);
            tile.set_pixel(11, 10, black);
        }
        // Default radius: the same byte the one-pixel path returns.
        for (x, y) in [(10, 10), (10, 11), (0, 0)] {
            assert_eq!(
                composite_pixel_avg(&doc, x, y, 1),
                composite_pixel(&doc, x, y),
                "1×1 must stay the old single-pixel pick at ({x}, {y})"
            );
        }
        // 2×2 at (10, 10) covers two inked pixels and two of bare paper.
        let avg = composite_pixel_avg(&doc, 10, 10, 2).unwrap();
        assert!(
            (185..=191).contains(&avg[0]),
            "half ink half paper must read ~188, got {avg:?}"
        );
        assert_eq!(
            composite_pixel_avg(&doc, 10, 10, 0),
            composite_pixel(&doc, 10, 10)
        );
        assert_eq!(composite_pixel_avg(&doc, -1, 10, 3), None);
    }

    #[test]
    fn empty_document_exports_as_the_background() {
        let doc = Document::new(64, 64);
        let img = composite(&doc, Background::White);
        assert_eq!(img.dimensions(), (64, 64));
        assert_eq!(img.get_pixel(0, 0).0, [255, 255, 255, 255]);

        let img = composite(&doc, Background::Transparent);
        assert_eq!(img.get_pixel(0, 0).0, [0, 0, 0, 0]);

        let img = composite(&doc, Background::Solid([10, 20, 30]));
        assert_eq!(img.get_pixel(0, 0).0, [10, 20, 30, 255]);
    }

    #[test]
    fn half_alpha_black_over_white_is_mid_grey() {
        let mut doc = Document::new(64, 64);
        fill_tile(&mut doc, 0, TileIdx::new(0, 0), [0.0, 0.0, 0.0, 0.5]);
        let img = composite(&doc, Background::White);
        // 0.5 premultiplied black over white -> 0.5 -> 128 (round-half-up).
        assert_eq!(img.get_pixel(10, 10).0, [128, 128, 128, 255]);
    }

    #[test]
    fn layer_opacity_and_blend_are_honoured() {
        let mut doc = Document::new(64, 64);
        fill_tile(&mut doc, 0, TileIdx::new(0, 0), [0.0, 0.0, 0.0, 1.0]);
        doc.set_layer_opacity(0, 0.5);
        let img = composite(&doc, Background::White);
        assert_eq!(img.get_pixel(0, 0).0, [128, 128, 128, 255]);

        // Multiply: opaque 50% grey over white = 50% grey.
        doc.set_layer_opacity(0, 1.0);
        doc.set_layer_blend(0, Blend::Multiply);
        fill_tile(&mut doc, 0, TileIdx::new(0, 0), [0.5, 0.5, 0.5, 1.0]);
        let img = composite(&doc, Background::White);
        assert_eq!(img.get_pixel(0, 0).0, [128, 128, 128, 255]);

        // Screen over white stays white.
        doc.set_layer_blend(0, Blend::Screen);
        let img = composite(&doc, Background::White);
        assert_eq!(img.get_pixel(0, 0).0, [255, 255, 255, 255]);

        // Hidden layers vanish.
        doc.set_layer_visible(0, false);
        let img = composite(&doc, Background::White);
        assert_eq!(img.get_pixel(0, 0).0, [255, 255, 255, 255]);
    }

    /// LP-016/LP-017 through the EXPORT path: the two-tone pair is display
    /// maths the exported PNG must carry too (it is what the page looks
    /// like), and the sub colour has to be inert in every shape that means
    /// "not set" — otherwise a file drawn before the second slot existed
    /// exports different pixels than it used to.
    #[test]
    fn the_sub_colour_reaches_the_export_and_off_is_the_old_output() {
        let mut doc = Document::new(128, 128);
        // Black, mid grey and white ink, plus a translucent tile: the tint
        // maths run per-pixel on unpremultiplied value, so partial coverage
        // is where a wrong divide would show.
        fill_tile(&mut doc, 0, TileIdx::new(0, 0), [0.0, 0.0, 0.0, 1.0]);
        fill_tile(&mut doc, 0, TileIdx::new(1, 0), [0.5, 0.5, 0.5, 1.0]);
        fill_tile(&mut doc, 0, TileIdx::new(0, 1), [1.0, 1.0, 1.0, 1.0]);
        fill_tile(&mut doc, 0, TileIdx::new(1, 1), [0.25, 0.25, 0.25, 0.5]);

        // A sub colour ALONE is nothing: the white end only moves once the
        // layer has a colour at all. The GPU agrees by construction — the
        // no-tint sentinel returns before it unpacks the sub word.
        let plain = composite_for_export(&doc, Background::White);
        assert!(doc.set_layer_sub_colour(0, Some([255, 192, 0])));
        assert_eq!(
            composite_for_export(&doc, Background::White).into_raw(),
            plain.clone().into_raw(),
            "a sub colour without a layer colour must change nothing"
        );

        assert!(doc.set_layer_sub_colour(0, None));
        assert!(doc.set_layer_colour(0, Some([0, 0, 255])));
        let main_only = composite_for_export(&doc, Background::White);
        assert_eq!(main_only.get_pixel(10, 10).0, [0, 0, 255, 255], "ink→blue");
        assert_eq!(main_only.get_pixel(10, 74).0, [255, 255, 255, 255], "white");

        // Both slots set: black takes the main colour, white takes the sub.
        assert!(doc.set_layer_sub_colour(0, Some([255, 192, 0])));
        let two_tone = composite_for_export(&doc, Background::White);
        assert_eq!(
            two_tone.get_pixel(10, 10).0,
            [0, 0, 255, 255],
            "the black end is still the main colour"
        );
        for (c, want) in two_tone.get_pixel(10, 74).0[..3].iter().zip([255, 192, 0]) {
            assert!(
                (*c as i32 - want).abs() <= 1,
                "the white end takes the sub colour, got {:?}",
                two_tone.get_pixel(10, 74).0
            );
        }

        // The compatibility promise, byte for byte: white sub == no sub ==
        // what this document exported before the second slot existed.
        assert!(doc.set_layer_sub_colour(0, Some([255, 255, 255])));
        assert_eq!(
            composite_for_export(&doc, Background::White).into_raw(),
            main_only.clone().into_raw(),
            "an explicit white sub is the LP-016 output"
        );
        assert!(doc.set_layer_sub_colour(0, None));
        assert_eq!(
            composite_for_export(&doc, Background::White).into_raw(),
            main_only.into_raw(),
            "clearing the sub restores the LP-016 output"
        );
    }

    #[test]
    fn layers_stack_bottom_first() {
        let mut doc = Document::new(64, 64);
        fill_tile(&mut doc, 0, TileIdx::new(0, 0), [1.0, 0.0, 0.0, 1.0]);
        doc.add_layer("top");
        fill_tile(&mut doc, 1, TileIdx::new(0, 0), [0.0, 0.0, 1.0, 1.0]);
        let img = composite(&doc, Background::White);
        assert_eq!(
            img.get_pixel(0, 0).0,
            [0, 0, 255, 255],
            "layers[1] is on top"
        );
    }

    #[test]
    fn folder_state_cascades_onto_children() {
        // [black child (depth 1), folder header] — hiding the folder hides
        // the child; folder opacity scales it.
        let mut doc = Document::new(64, 64);
        doc.layers[0].depth = 1;
        fill_tile(&mut doc, 0, TileIdx::new(0, 0), [0.0, 0.0, 0.0, 1.0]);
        let mut folder = crate::doc::Layer::new("F");
        folder.folder = true;
        doc.layers.push(folder);

        let img = composite(&doc, Background::White);
        assert_eq!(img.get_pixel(0, 0).0, [0, 0, 0, 255]);

        doc.set_layer_opacity(1, 0.5);
        let img = composite(&doc, Background::White);
        assert_eq!(img.get_pixel(0, 0).0, [128, 128, 128, 255]);

        doc.set_layer_opacity(1, 1.0);
        doc.set_layer_visible(1, false);
        let img = composite(&doc, Background::White);
        assert_eq!(img.get_pixel(0, 0).0, [255, 255, 255, 255]);
    }

    #[test]
    fn frame_folder_truly_clips_and_the_gutter_shows_through() {
        use crate::frame::FrameSet;
        let mut doc = Document::new(128, 128);
        // Below the folder: opaque red across the whole page.
        for ty in 0..2 {
            for tx in 0..2 {
                fill_tile(&mut doc, 0, TileIdx::new(tx, ty), [1.0, 0.0, 0.0, 1.0]);
            }
        }
        let fs = FrameSet::single_rect([32.0, 32.0, 96.0, 96.0], 4.0);
        let hi = doc.add_frame_folder("F", fs);
        let draw = hi - 1;
        // Green everywhere on the draw layer inside the folder.
        for ty in 0..2 {
            for tx in 0..2 {
                fill_tile(&mut doc, draw, TileIdx::new(tx, ty), [0.0, 1.0, 0.0, 1.0]);
            }
        }

        let img = composite(&doc, Background::White);
        assert_eq!(
            img.get_pixel(8, 8).0,
            [255, 0, 0, 255],
            "TRUE isolation: art below the folder shows through the gutter"
        );
        assert_eq!(
            img.get_pixel(64, 64).0,
            [0, 255, 0, 255],
            "children clipped to the panel show inside"
        );
        let border = img.get_pixel(64, 32).0;
        assert!(
            border[0] < 40 && border[1] < 40,
            "border ink on top: {border:?}"
        );

        // The White child hides the red below INSIDE the panel only.
        doc.set_layer_visible(draw, false);
        let img = composite(&doc, Background::White);
        assert_eq!(
            img.get_pixel(64, 64).0,
            [255, 255, 255, 255],
            "White base inside"
        );
        assert_eq!(img.get_pixel(8, 8).0, [255, 0, 0, 255], "gutter still red");

        // Folder hidden: only the red base remains (border included).
        doc.set_layer_visible(hi, false);
        let img = composite(&doc, Background::White);
        assert_eq!(img.get_pixel(64, 32).0, [255, 0, 0, 255]);
    }

    /// FB-knockout: a plain folder's Border effect becomes a mat laid just
    /// beneath the group — white behind the children's ink, grown by the
    /// width, dimmed with the folder's opacity, gone when the flag drops.
    #[test]
    fn folder_knockout_mats_behind_the_group() {
        let mut doc = Document::new(128, 128);
        for ty in 0..2 {
            for tx in 0..2 {
                fill_tile(&mut doc, 0, TileIdx::new(tx, ty), [1.0, 0.0, 0.0, 1.0]);
            }
        }
        let fi = doc.add_folder_above(0, "chara");
        let child = doc.add_layer_in_folder(fi, "ink").unwrap();
        let hdr = doc.layers.len() - 1;
        fill_tile(&mut doc, child, TileIdx::new(0, 0), [0.0, 0.0, 0.0, 1.0]);
        assert!(doc.set_edge(
            hdr,
            Some(crate::edge::EdgeParams {
                width_px: 4.0,
                colour: [255, 255, 255],
                ..Default::default()
            })
        ));
        doc.refresh_derived(600);
        let mats = doc.layers[hdr].edge_tiles().expect("mat derived");
        assert!(
            mats.contains_key(&TileIdx::new(1, 0)),
            "mat spills into the neighbour tile: {:?}",
            mats.keys().collect::<Vec<_>>()
        );

        let img = composite(&doc, Background::White);
        assert_eq!(img.get_pixel(30, 30).0, [0, 0, 0, 255], "ink on top");
        assert_eq!(
            img.get_pixel(66, 30).0,
            [255, 255, 255, 255],
            "the mat rings the group's ink"
        );
        assert_eq!(
            img.get_pixel(100, 100).0,
            [255, 0, 0, 255],
            "far away the backdrop stands"
        );

        // Folder opacity dims the mat with the group.
        doc.set_layer_opacity(doc.layers.len() - 1, 0.5);
        doc.refresh_derived(600);
        let img = composite(&doc, Background::White);
        let ring = img.get_pixel(66, 30).0;
        assert!(
            ring[0] > 200 && ring[1] > 100 && ring[1] < 200,
            "half-strength mat over red: {ring:?}"
        );

        // Effect off: the ring is red backdrop again.
        doc.set_layer_opacity(doc.layers.len() - 1, 1.0);
        assert!(doc.set_edge(doc.layers.len() - 1, None));
        doc.refresh_derived(600);
        let img = composite(&doc, Background::White);
        assert_eq!(img.get_pixel(66, 30).0, [255, 0, 0, 255]);
    }

    /// FB-overflow: the escape flag re-seats a child above its frame
    /// folder — outside the panel mask AND over the border ink — and
    /// turning it off restores the clip exactly.
    #[test]
    fn escaped_layer_bursts_out_of_the_panel_and_over_the_border() {
        use crate::frame::FrameSet;
        let mut doc = Document::new(128, 128);
        let fs = FrameSet::single_rect([32.0, 32.0, 96.0, 96.0], 4.0);
        let hi = doc.add_frame_folder("F", fs);
        let draw = hi - 1;
        for ty in 0..2 {
            for tx in 0..2 {
                fill_tile(&mut doc, draw, TileIdx::new(tx, ty), [0.0, 1.0, 0.0, 1.0]);
            }
        }

        let img = composite(&doc, Background::White);
        assert_eq!(
            img.get_pixel(8, 8).0,
            [255, 255, 255, 255],
            "clipped: nothing outside the panel"
        );
        let border = img.get_pixel(64, 32).0;
        assert!(
            border[0] < 40 && border[1] < 40,
            "border ink on top while clipped: {border:?}"
        );

        assert!(doc.set_layer_escape(draw, true));
        let img = composite(&doc, Background::White);
        assert_eq!(
            img.get_pixel(8, 8).0,
            [0, 255, 0, 255],
            "escaped: bursts outside the panel"
        );
        assert_eq!(
            img.get_pixel(64, 32).0,
            [0, 255, 0, 255],
            "escaped: drawn over the border ink"
        );

        assert!(doc.set_layer_escape(draw, false));
        let img = composite(&doc, Background::White);
        assert_eq!(
            img.get_pixel(8, 8).0,
            [255, 255, 255, 255],
            "flag off: the clip is back"
        );

        // The flag refuses where it would lie: folders, and layers with no
        // frame folder above them.
        assert!(!doc.set_layer_escape(hi, true), "folders refuse");
        let mut flat = Document::new(64, 64);
        assert!(
            !flat.set_layer_escape(0, true),
            "no frame folder above: refuse"
        );
    }

    #[test]
    fn clip_layer_shows_only_over_its_base() {
        let mut doc = Document::new(128, 64);
        // Base: half-alpha blue on the LEFT tile only.
        fill_tile(&mut doc, 0, TileIdx::new(0, 0), [0.0, 0.0, 1.0, 0.5]);
        doc.add_layer("paint");
        doc.set_layer_clip(1, true);
        for tx in 0..2 {
            fill_tile(&mut doc, 1, TileIdx::new(tx, 0), [1.0, 0.0, 0.0, 1.0]);
        }

        let img = composite(&doc, Background::White);
        // Over the base: half-alpha blue over white ([128,128,255]) with the
        // opaque red clipped to alpha 0.5 on top: 0.5·red + 0.5·backdrop.
        assert_eq!(img.get_pixel(10, 10).0, [191, 64, 128, 255]);
        // Off the base: the clip layer contributes nothing.
        assert_eq!(img.get_pixel(80, 10).0, [255, 255, 255, 255]);
    }

    /// docs/CLIPPING-SCENARIOS.md 2a: a layer clipped to a FOLDER shows
    /// only over the group's combined ink, at the group's raw alpha — the
    /// header's opacity does not fold into the base (the same raw-alpha
    /// rule layer bases follow), and a hidden folder is zero ink.
    #[test]
    fn clip_layer_over_a_folder_clips_to_the_group_ink() {
        let mut doc = Document::new(128, 64);
        // Group ink: opaque green on the LEFT tile only, inside folder F.
        let hi = doc.add_folder_above(0, "F");
        let inside = doc.add_layer_in_folder(hi, "in").unwrap();
        fill_tile(&mut doc, inside, TileIdx::new(0, 0), [0.0, 1.0, 0.0, 1.0]);
        let hi = hi + 1; // the child inserted below shifted the header up
        // The clipped layer above the folder: red across both tiles.
        let top = doc.add_layer_above(hi, "Shade");
        doc.set_layer_clip(top, true);
        for tx in 0..2 {
            fill_tile(&mut doc, top, TileIdx::new(tx, 0), [1.0, 0.0, 0.0, 1.0]);
        }

        let img = composite(&doc, Background::White);
        assert_eq!(img.get_pixel(10, 10).0, [255, 0, 0, 255], "over group ink");
        assert_eq!(
            img.get_pixel(80, 10).0,
            [255, 255, 255, 255],
            "off the group ink the clip layer contributes nothing"
        );

        // The capture happens before the folder's opacity/blend: turning the
        // folder down does not thin the clipped layer.
        doc.layers[hi].opacity = 0.25;
        let img = composite(&doc, Background::White);
        assert_eq!(
            img.get_pixel(10, 10).0,
            [255, 0, 0, 255],
            "base alpha is the group's RAW alpha"
        );

        // A hidden folder never composites its children: zero base ink.
        doc.set_layer_visible(hi, false);
        let img = composite(&doc, Background::White);
        assert_eq!(
            img.get_pixel(10, 10).0,
            [255, 255, 255, 255],
            "hidden folder = the clip has nothing to sit on"
        );
    }

    #[test]
    fn transparent_export_keeps_straight_colour() {
        let mut doc = Document::new(64, 64);
        fill_tile(&mut doc, 0, TileIdx::new(0, 0), [1.0, 0.0, 0.0, 0.5]);
        let img = composite(&doc, Background::Transparent);
        let p = img.get_pixel(0, 0).0;
        assert_eq!(p[3], 128);
        assert_eq!(p[0], 255, "un-premultiplied red must come back as 255");
    }

    #[test]
    fn layer_image_is_cropped_to_its_tiles() {
        let mut doc = Document::new(512, 512);
        fill_tile(&mut doc, 0, TileIdx::new(2, 3), [0.0, 1.0, 0.0, 1.0]);
        let (img, x, y) = layer_image(&doc.layers[0]).unwrap();
        assert_eq!((x, y), (128, 192));
        assert_eq!(img.dimensions(), (64, 64));
        assert_eq!(img.get_pixel(0, 0).0, [0, 255, 0, 255]);
        assert!(layer_image(&crate::doc::Layer::new("empty")).is_none());
    }
}

#[cfg(test)]
mod finish_tests {
    use super::*;
    use crate::doc::LayerExpression as E;

    /// The picker reads the draft back to find its name, so identical
    /// triples would hide a preset; and the default finish must still be
    /// the run the export did before presets existed.
    #[test]
    fn print_presets_are_distinct_and_the_default_is_todays_run() {
        for (i, a) in PRINT_PRESETS.iter().enumerate() {
            for b in PRINT_PRESETS.iter().skip(i + 1) {
                assert_ne!(
                    a.finish, b.finish,
                    "{} and {} would collide in the picker",
                    a.name, b.name
                );
            }
            assert_eq!(matching_preset(a.finish), Some(i), "{} is findable", a.name);
        }
        let d = ExportFinish::default();
        assert_eq!(d.dpi, 0, "no resample");
        assert_eq!(d.colour, E::Colour);
        assert!(!d.split_spreads);
        assert_eq!(matching_preset(d), None, "the default reads as Custom");
    }

    /// The u8 finish and the fix15 layer PREVIEW are one definition of
    /// "grey" and "mono": drift here means the preview lies about what
    /// the exported file will look like.
    #[test]
    fn u8_reduce_agrees_with_the_fix15_preview() {
        for p in [
            [0u8, 0, 0, 255],
            [255, 255, 255, 255],
            [200, 30, 30, 255],
            [130, 130, 130, 255],
            [120, 120, 120, 255],
            [10, 240, 90, 255],
        ] {
            for e in [E::Grey, E::Mono] {
                // Straight u8 -> premultiplied fix15 (alpha 255 here, so
                // premultiplication is the identity scale).
                let fix = |c: u8| (c as u32 * 32768 / 255) as u16;
                let got = reduce_u8(p, e);
                let want = crate::blend::expression_reduce(
                    [fix(p[0]), fix(p[1]), fix(p[2]), fix(p[3])],
                    e,
                );
                let back = |v: u16| ((v as u32 * 255 + 16384) / 32768) as u8;
                for c in 0..3 {
                    assert!(
                        (got[c] as i32 - back(want[c]) as i32).abs() <= 1,
                        "{p:?} {e:?} channel {c}: u8 {} vs fix15 {}",
                        got[c],
                        back(want[c])
                    );
                }
                assert_eq!(got[3], back(want[3]), "{p:?} {e:?} alpha");
            }
        }
    }

    /// Downscale THEN threshold: a mono finish must land 1-bit at the
    /// output size. Thresholding first and resampling after would hand
    /// the printer a greyscale file that claims to be 1-bit.
    #[test]
    fn mono_finish_is_one_bit_after_the_resample() {
        let mut img = image::RgbaImage::new(64, 64);
        for (x, _y, px) in img.enumerate_pixels_mut() {
            let v = (x * 4) as u8; // a soft ramp across the threshold
            px.0 = [v, v, v, 255];
        }
        let out = finish_image(img.clone(), 0.5, E::Mono);
        assert_eq!(out.dimensions(), (32, 32));
        for px in out.pixels() {
            assert!(
                px.0[0] == 0 || px.0[0] == 255,
                "a resampled grey survived the threshold: {:?}",
                px.0
            );
        }
        // Grey keeps its ramp; colour and an up-scale are both no-ops.
        let grey = finish_image(img.clone(), 1.0, E::Grey);
        assert!(grey.pixels().any(|p| p.0[0] != 0 && p.0[0] != 255));
        assert_eq!(
            finish_image(img.clone(), 2.0, E::Colour).dimensions(),
            (64, 64)
        );
    }

    /// The scale never exceeds 1.0, and a work with no dpi has nothing to
    /// scale relative to.
    #[test]
    fn finish_scale_never_upsamples() {
        assert_eq!(finish_scale(350, Some(600)), 350.0 / 600.0);
        assert_eq!(finish_scale(600, Some(350)), 1.0, "no invented detail");
        assert_eq!(finish_scale(0, Some(600)), 1.0, "0 = the work's own");
        assert_eq!(finish_scale(350, None), 1.0, "a pixel canvas has no dpi");
    }
}

#[cfg(test)]
mod through_tests {
    use super::*;
    use crate::tile::{FIX15_ONE, TileIdx};

    /// LF-002, the row-18 scenario verbatim: a Multiply layer inside a
    /// NORMAL folder multiplies onto its folder-mates only — the art below
    /// the folder is untouched (the CSP complaint: "your shadow does
    /// nothing"). Set the folder to THROUGH and the seal is removed: the
    /// same child multiplies onto the page below.
    #[test]
    fn through_folder_removes_the_seal() {
        let build = |through: bool| {
            let mut doc = Document::new(64, 64);
            let art = doc.add_layer("art");
            doc.layers[art].tile_mut(TileIdx::new(0, 0)).set_pixel(
                5,
                5,
                [30000, 0, 0, FIX15_ONE as u16],
            );
            let f = doc.add_layer("F");
            doc.layers[f].folder = true;
            doc.layers[f].through = through;
            let m = doc.add_layer("mult");
            doc.layers[m].depth = 1;
            doc.layers[m].blend = crate::doc::Blend::Multiply;
            doc.layers[m].tile_mut(TileIdx::new(0, 0)).set_pixel(
                5,
                5,
                [16384, 16384, 16384, FIX15_ONE as u16],
            );
            doc
        };

        let px = |doc: &Document| {
            let img = composite(doc, Background::White);
            let p = img.get_pixel(5, 5);
            p.0[0]
        };
        let sealed = px(&build(false));
        let loose = px(&build(true));
        assert!(
            sealed > 200,
            "a normal folder seals: the multiply must not reach the red ({sealed})"
        );
        assert!(
            loose < sealed - 50,
            "a through folder removes the seal: the multiply darkens the page ({loose} vs {sealed})"
        );
    }

    /// Through composites EXACTLY as if the folder were not there and the
    /// child were loose at root depth (the definition, verbatim).
    #[test]
    fn through_equals_loose_layers() {
        let through_doc = {
            let mut doc = Document::new(64, 64);
            let art = doc.add_layer("art");
            doc.layers[art].tile_mut(TileIdx::new(0, 0)).set_pixel(
                5,
                5,
                [30000, 0, 0, FIX15_ONE as u16],
            );
            let f = doc.add_layer("F");
            doc.layers[f].folder = true;
            doc.layers[f].through = true;
            let m = doc.add_layer("mult");
            doc.layers[m].depth = 1;
            doc.layers[m].blend = crate::doc::Blend::Multiply;
            doc.layers[m].opacity = 0.8;
            doc.layers[m].tile_mut(TileIdx::new(0, 0)).set_pixel(
                5,
                5,
                [16384, 16384, 16384, FIX15_ONE as u16],
            );
            doc
        };
        let mut loose = Document::new(64, 64);
        let art = loose.add_layer("art");
        loose.layers[art].tile_mut(TileIdx::new(0, 0)).set_pixel(
            5,
            5,
            [30000, 0, 0, FIX15_ONE as u16],
        );
        let m = loose.add_layer("mult");
        loose.layers[m].blend = crate::doc::Blend::Multiply;
        loose.layers[m].opacity = 0.8;
        loose.layers[m].tile_mut(TileIdx::new(0, 0)).set_pixel(
            5,
            5,
            [16384, 16384, 16384, FIX15_ONE as u16],
        );

        let a = composite(&through_doc, Background::White);
        let b = composite(&loose, Background::White);
        for (p, q) in a.pixels().zip(b.pixels()) {
            assert_eq!(p.0, q.0, "through must equal loose layers pixel-for-pixel");
        }
    }
}

#[cfg(test)]
mod through_ora_tests {
    use super::*;
    use crate::tile::{FIX15_ONE, TileIdx};

    /// LF-002: the through flag survives an ORA save/load round trip (a
    /// folder keeps it; a plain layer never carries it).
    #[test]
    fn through_survives_an_ora_round_trip() {
        let mut doc = Document::new(64, 64);
        let art = doc.add_layer("art");
        doc.layers[art].tile_mut(TileIdx::new(0, 0)).set_pixel(
            5,
            5,
            [30000, 0, 0, FIX15_ONE as u16],
        );
        let f = doc.add_layer("F");
        doc.layers[f].folder = true;
        doc.layers[f].through = true;
        let m = doc.add_layer("mult");
        doc.layers[m].depth = 1;

        let mut buf = std::io::Cursor::new(Vec::new());
        crate::ora::save_to(&doc, &mut buf).unwrap();
        let back = crate::ora::load_from(buf).unwrap();
        assert!(back.layers[f].folder);
        assert!(back.layers[f].through, "the flag round-trips");
        assert!(!back.layers[m].through, "plain layers never carry it");
    }
}

#[cfg(test)]
mod crop_tests {
    use super::*;

    fn setup() -> crate::page::PageSetup {
        // The 投稿 manuscript preset: 257×364 paper, 220×310 trim, 3mm bleed.
        crate::page::PageSetup::presets()
            .into_iter()
            .find(|p| p.name.contains("投稿"))
            .expect("manuscript preset")
    }

    /// Trim and trim+bleed rects land where the setup's own derived rects
    /// say; Paper is the identity; a spread keeps the outer insets and
    /// spans the fold.
    #[test]
    fn crop_rects_follow_the_setup_and_span_spreads() {
        let s = setup();
        let (pw, ph) = s.paper_px();
        assert_eq!(
            crop_rect_px(&s, (pw, ph), ExportCrop::Paper),
            [0, 0, pw, ph]
        );

        let t = s.trim_rect_px();
        let got = crop_rect_px(&s, (pw, ph), ExportCrop::Trim);
        assert_eq!(
            got,
            [
                t[0] as u32,
                t[1] as u32,
                t[2].round() as u32,
                t[3].round() as u32
            ]
        );

        let b = s.bleed_rect_px();
        let gb = crop_rect_px(&s, (pw, ph), ExportCrop::TrimBleed);
        assert!(gb[0] < got[0] && gb[2] > got[2], "bleed extends past trim");
        assert_eq!(gb[0], b[0] as u32);

        // A spread (double-width doc): left inset = single page's, right
        // inset mirrored at the far edge.
        let gs = crop_rect_px(&s, (pw * 2, ph), ExportCrop::Trim);
        assert_eq!(gs[0], got[0], "left inset unchanged");
        assert_eq!(
            gs[2],
            (pw * 2) - (pw - got[2]),
            "right inset mirrored on the spread's far edge"
        );

        // Degenerate (zero trim — pixel canvases): full paper, no empty file.
        let mut px = s.clone();
        px.trim_mm = (0.0, 0.0);
        assert_eq!(
            crop_rect_px(&px, (pw, ph), ExportCrop::Trim),
            [0, 0, pw, ph]
        );
    }

    /// The cropped finish: crop precedes resample; exact px_height wins
    /// over dpi scale and never upsamples.
    #[test]
    fn cropped_finish_crops_then_fits_height() {
        let img = image::RgbaImage::from_pixel(400, 600, image::Rgba([255, 255, 255, 255]));
        let out = finish_image_cropped(
            img.clone(),
            [100, 100, 300, 500],
            1.0,
            0,
            crate::doc::LayerExpression::Colour,
        );
        assert_eq!((out.width(), out.height()), (200, 400));

        let out = finish_image_cropped(
            img.clone(),
            [100, 100, 300, 500],
            1.0,
            200,
            crate::doc::LayerExpression::Colour,
        );
        assert_eq!(
            (out.width(), out.height()),
            (100, 200),
            "exact height, ratio kept"
        );

        let out = finish_image_cropped(
            img,
            [0, 0, 400, 600],
            1.0,
            4096,
            crate::doc::LayerExpression::Colour,
        );
        assert_eq!((out.width(), out.height()), (400, 600), "never upsamples");
    }
}
