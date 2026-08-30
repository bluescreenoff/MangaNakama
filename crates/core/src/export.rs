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
                // FB-overflow: escaped layers re-seat above their anchor —
                // `order` is the shared walk, `step.depth` the effective
                // depth (the anchor's own, for an escapee) and `step.part`
                // which half of a mask-capped spill this step draws.
                for &step in &order {
                    let li = step.layer;
                    let layer = &doc.layers[li];
                    if !eff[li] {
                        continue;
                    }
                    let d = step.depth as usize;
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
                    // Breakout mask cap: the IN half is the exact complement
                    // of the mask, and an absent tile is full coverage — so
                    // it holds nothing back and the whole tile stays with the
                    // OUT half at the escaped seat. Skipping the tile outright
                    // keeps that free.
                    if step.part == crate::doc::SpillPart::In && mask_cov.is_none() {
                        continue;
                    }
                    let hold_in = step.part == crate::doc::SpillPart::In;
                    // Blend If. The gate itself is resolved once per layer;
                    // what has to be per-pixel is the VALUE READ, because the
                    // gate asks about the destination accumulator —
                    // everything composited below this layer so far — or
                    // about this layer's own finished ink. `None` (the
                    // overwhelmingly common case) costs one Option test per
                    // layer and nothing per pixel.
                    let gate = layer.gate();
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
                            let m = if hold_in { 1.0 - m } else { m };
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
                        // Blend If, LAST: the gate is about what this layer
                        // is landing ON, so it weighs the finished source —
                        // after the expression reduce, the tint, the layer
                        // opacity, the mask and the clip — exactly as if the
                        // artist had turned the opacity down at this one
                        // pixel. `blend2.wgsl` scales `s` at the same point.
                        if let Some(g) = gate {
                            // `weight_for` and not a luma read: WHICH pixel
                            // (underlying composite or this layer's own ink)
                            // and WHICH channel are the gate's business, not
                            // the compositor's — one answer, shared with the
                            // shader.
                            let w = g.weight_for(src, *slot);
                            for c in src.iter_mut() {
                                *c *= w;
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

/// Which kernel a downscaling export resamples with — CSP's 処理方法
/// (イラスト向き / コミック向き), and it is not cosmetic.
///
/// `Photo` is a general-purpose photographic filter (Lanczos): correct for
/// colour and greyscale, and *wrong* for 1-bit line art, because a hairline
/// it averages into grey is then killed outright by the 50 % mono
/// threshold. `Comic` area-averages the page's INK COVERAGE and re-thresholds
/// at a biased level ([`COMIC_INK_BIAS`]) so a thin dark line survives the
/// shrink instead of dissolving.
///
/// **Only a MONO finish resamples differently.** Comic is a decision about
/// where to put a threshold, and grey/colour output has no threshold to
/// bias — so `Comic` on a non-mono finish is `Photo`, which is why `Comic`
/// can be the default without changing a single colour or grey export.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Resample {
    /// Threshold-aware downscale for 1-bit manga line art. The default:
    /// this application's whole identity is mono pages, and the old
    /// behaviour dissolved their hairlines silently.
    #[default]
    Comic,
    /// The pre-2026-08-29 kernel, unchanged, byte for byte — and the right
    /// answer for anything continuous-tone.
    Photo,
}

impl Resample {
    /// Does this policy actually take the comic path for `colour`?
    ///
    /// The one place the "mono only" rule is written down; the dialog's
    /// enable/disable and the pixels agree because they ask this.
    pub fn is_comic(self, colour: crate::doc::LayerExpression) -> bool {
        self == Resample::Comic && colour == crate::doc::LayerExpression::Mono
    }
}

/// What an Export All run writes to disk.
///
/// PNG is 入稿 (the printer's copy, lossless, big). JPEG is 提出 — the light
/// copy you hand an editor twice a chapter: small enough to mail, openable
/// on a phone. They are different jobs, not different tastes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ExportFormat {
    #[default]
    Png,
    Jpeg,
}

impl ExportFormat {
    /// The filename extension, without the dot. The dialog's name preview
    /// and the writer read the SAME function — a preview that could
    /// disagree with the files is worse than no preview.
    pub fn ext(self) -> &'static str {
        match self {
            ExportFormat::Png => "png",
            ExportFormat::Jpeg => "jpg",
        }
    }
}

/// `IO-030` — what a REDUCED export does to a screentone (CSP asks this at
/// export time, and the workflow audit's runner-up 13 is that it is a
/// CHOICE, not a bug to avoid).
///
/// # The two honest answers
///
/// A tone layer is derived at the work's dpi and then resampled with
/// everything else, so a 600 → 350 dpi finish shrinks the dots along with
/// the art. That keeps the printed screen at the layer's own frequency —
/// 60 lpi stays 60 lpi — and it is what this app has always done. The price
/// is moiré: a lattice whose period lands on a fractional number of output
/// pixels beats against the sample grid, and 60 lpi at a 0.583 scale is
/// exactly such a period.
///
/// The other answer is to screen for the EXPORT instead: derive the tone
/// against `work_dpi / scale`, so that after the reduction each cell is the
/// size it was in the work — a whole number of output pixels, no beat. The
/// printed screen coarsens to `lpi × scale` (60 lpi at half size prints as
/// 30 lpi), which is a real change to how the page reads and is why this is
/// the artist's call and not ours.
///
/// [`Self::Frequency`] is the default and is byte-for-byte the old
/// behaviour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ToneScale {
    /// Keep the layer's frequency; the dots resample with the art.
    #[default]
    Frequency,
    /// Re-screen for the export scale; the dots keep their pixel size and
    /// the printed frequency drops with the reduction.
    Dots,
}

impl ToneScale {
    pub fn label(self) -> &'static str {
        match self {
            ToneScale::Frequency => "Keep frequency (dots rescale)",
            ToneScale::Dots => "Re-screen for the export scale",
        }
    }
}

/// The dpi a page's tone screens should be DERIVED at for an export that
/// finishes at `scale` — [`ToneScale`]'s one line of consequence.
///
/// `Frequency` derives at the work's own dpi (the screen is then reduced
/// with the page). `Dots` derives at `work / scale`, so the reduction lands
/// the cell back at its work-pixel size. Both clamp to a sane dpi: a scale
/// of 0 or a missing work dpi means "no reduction is happening", and the
/// tone dpi is then just the tone dpi.
pub fn tone_export_dpi(tone_dpi: u32, scale: f32, mode: ToneScale) -> u32 {
    if mode == ToneScale::Frequency || !(scale > 0.0) || scale >= 1.0 {
        return tone_dpi;
    }
    ((tone_dpi as f32 / scale).round() as u32).clamp(tone_dpi, 20_000)
}

/// The finishing decisions a submission target fixes: output resolution,
/// expression colour, resample kernel and container. `dpi == 0` means "the
/// work's own resolution, no resample" — the same `0 = no dpi` convention
/// `PageSetup::dpi` uses.
///
/// Whether a spread leaves as two files lives on the export dialog
/// (`export_all_split`) and is NOT duplicated here: one value, one home.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExportFinish {
    pub dpi: u32,
    pub colour: crate::doc::LayerExpression,
    pub split_spreads: bool,
    pub resample: Resample,
    /// `IO-030`: what a REDUCED export does to a screentone. Part of the
    /// finish because it is a submission decision — the printer's screen
    /// frequency — not a rendering taste.
    pub tone: ToneScale,
    pub format: ExportFormat,
    /// JPEG quality, 1..=100. Carried even when the format is PNG so the
    /// knob keeps its value across a format flip; ignored by the PNG
    /// writer.
    pub quality: u8,
}

/// The JPEG quality a proof ships at. High enough that 1-bit edges do not
/// visibly mosquito-ring at reading size, low enough that a 20-page chapter
/// mails: the 85 every JP 書き出し guide names for 提出用.
pub const PROOF_JPEG_QUALITY: u8 = 85;

impl Default for ExportFinish {
    /// Today's untouched Export All Pages run, byte for byte. (The
    /// resample default is `Comic`, which is a no-op here: the default
    /// colour is full colour, and comic only bites on mono.)
    fn default() -> Self {
        Self {
            dpi: 0,
            colour: crate::doc::LayerExpression::Colour,
            split_spreads: false,
            resample: Resample::Comic,
            tone: ToneScale::Frequency,
            format: ExportFormat::Png,
            quality: PROOF_JPEG_QUALITY,
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
            resample: Resample::Comic,
            tone: ToneScale::Frequency,
            format: ExportFormat::Png,
            quality: PROOF_JPEG_QUALITY,
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
            resample: Resample::Comic,
            tone: ToneScale::Frequency,
            format: ExportFormat::Png,
            quality: PROOF_JPEG_QUALITY,
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
            resample: Resample::Comic,
            tone: ToneScale::Frequency,
            format: ExportFormat::Png,
            quality: PROOF_JPEG_QUALITY,
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
            resample: Resample::Comic,
            tone: ToneScale::Frequency,
            format: ExportFormat::Png,
            quality: PROOF_JPEG_QUALITY,
        },
    },
    ExportPreset {
        name: "Proof JPEG 150 dpi (提出用)",
        note: "the light copy an editor gets twice a chapter: small JPEGs a \
               phone opens, spreads kept whole so the reading order survives \
               — never the file you send a printer",
        finish: ExportFinish {
            dpi: 150,
            colour: crate::doc::LayerExpression::Colour,
            split_spreads: false,
            resample: Resample::Comic,
            tone: ToneScale::Frequency,
            format: ExportFormat::Jpeg,
            quality: PROOF_JPEG_QUALITY,
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

/// How much of an output pixel must be covered by INK before the comic
/// downscale calls it black.
///
/// Picked by test, not by feel (`a_hairline_survives_the_comic_shrink`).
/// The number that matters: a 1 px line shrunk by 0.5 lands as exactly 50 %
/// coverage, so anything at or under 0.5 keeps it — but 0.5 is the knife
/// edge that float rounding decides, so the bias sits clear of it. 0.35
/// also keeps a 1 px line down to a 0.35× shrink, while a light 25 % tone
/// still drops to paper instead of blotting solid. Going much lower fattens
/// the whole page: every stray dark pixel would print.
pub const COMIC_INK_BIAS: f32 = 0.35;

/// One straight-RGBA8 pixel as `(alpha, ink)` in 0..1, where ink is
/// alpha-weighted darkness.
///
/// Darkness is `1 - mean(rgb)`, the SAME mean-of-channels rule `reduce_u8`
/// thresholds on — if the downscale and the reduction disagreed about what
/// "dark" means, the comic path would be biasing against the wrong axis.
fn ink_of(p: &image::Rgba<u8>) -> (f32, f32) {
    let a = p.0[3] as f32 / 255.0;
    let v = (p.0[0] as f32 + p.0[1] as f32 + p.0[2] as f32) / (3.0 * 255.0);
    (a, a * (1.0 - v))
}

/// CSP's コミック向き: area-average the ink, then re-threshold at
/// [`COMIC_INK_BIAS`]. Output is already 1-bit black-on-white (or fully
/// transparent), so the `reduce_u8` pass that follows is a no-op on it.
///
/// Exact box weights, computed per output pixel straight off the source —
/// no full-canvas float buffer. A 600 dpi B4 page is ~23 M pixels and this
/// module's whole memory discipline (see the file header) is about not
/// materialising one of those as f32.
fn comic_downscale(img: &image::RgbaImage, ow: u32, oh: u32) -> image::RgbaImage {
    let (iw, ih) = img.dimensions();
    let mut out = image::RgbaImage::new(ow, oh);
    let sx = iw as f64 / ow as f64;
    let sy = ih as f64 / oh as f64;
    for oy in 0..oh {
        let y0 = oy as f64 * sy;
        let y1 = ((oy + 1) as f64 * sy).min(ih as f64);
        for ox in 0..ow {
            let x0 = ox as f64 * sx;
            let x1 = ((ox + 1) as f64 * sx).min(iw as f64);
            let (mut area, mut acov, mut ink) = (0.0f32, 0.0f32, 0.0f32);
            for y in (y0.floor() as u32)..(y1.ceil() as u32).min(ih) {
                let wy = (((y + 1) as f64).min(y1) - (y as f64).max(y0)).max(0.0) as f32;
                if wy <= 0.0 {
                    continue;
                }
                for x in (x0.floor() as u32)..(x1.ceil() as u32).min(iw) {
                    let wx = (((x + 1) as f64).min(x1) - (x as f64).max(x0)).max(0.0) as f32;
                    if wx <= 0.0 {
                        continue;
                    }
                    let (a, i) = ink_of(img.get_pixel(x, y));
                    let w = wx * wy;
                    area += w;
                    acov += w * a;
                    ink += w * i;
                }
            }
            // Alpha keeps `reduce_u8`'s own 50 % rule — the bias is a
            // statement about ink, not about coverage of the page.
            let px = if area <= 0.0 || acov / area < 0.5 {
                [0, 0, 0, 0]
            } else if ink / acov >= COMIC_INK_BIAS {
                [0, 0, 0, 255]
            } else {
                [255, 255, 255, 255]
            };
            out.put_pixel(ox, oy, image::Rgba(px));
        }
    }
    out
}

/// Downscale by the policy: comic where it applies, else the untouched
/// Lanczos path. `(w, h)` are already the output size.
fn resample_to(
    img: &image::RgbaImage,
    w: u32,
    h: u32,
    colour: crate::doc::LayerExpression,
    resample: Resample,
) -> image::RgbaImage {
    if resample.is_comic(colour) {
        comic_downscale(img, w, h)
    } else {
        image::imageops::resize(img, w, h, image::imageops::FilterType::Lanczos3)
    }
}

/// Write a finished page.
///
/// **Mono + JPEG (decision, 2026-08-29).** JPEG cannot hold 1 bit, and
/// refusing the combination would break the one workflow it exists for —
/// showing an editor the real, thresholded page in a file he can open on a
/// phone. So a mono (or grey) finish writes an 8-bit GREYSCALE JPEG of the
/// already-thresholded pixels: the page you see is the page that prints,
/// minus the container's promise of exactness. The PNG path is untouched
/// and stays the one you send a printer.
pub fn save_finished(
    img: &image::RgbaImage,
    path: &Path,
    format: ExportFormat,
    quality: u8,
    colour: crate::doc::LayerExpression,
) -> image::ImageResult<()> {
    match format {
        ExportFormat::Png => img.save(path),
        ExportFormat::Jpeg => {
            let mut w = std::io::BufWriter::new(std::fs::File::create(path)?);
            let mut enc =
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut w, quality.clamp(1, 100));
            // JPEG has no alpha: a transparent-background export is
            // composited onto paper white here rather than having its
            // alpha silently dropped (which would print the ink's
            // premultiplied black over black).
            if colour == crate::doc::LayerExpression::Colour {
                let mut rgb = image::RgbImage::new(img.width(), img.height());
                for (o, p) in rgb.pixels_mut().zip(img.pixels()) {
                    let a = p.0[3] as u32;
                    o.0 = std::array::from_fn(|c| {
                        ((p.0[c] as u32 * a + 255 * (255 - a) + 127) / 255).min(255) as u8
                    });
                }
                enc.encode_image(&rgb)
            } else {
                let mut grey = image::GrayImage::new(img.width(), img.height());
                for (o, p) in grey.pixels_mut().zip(img.pixels()) {
                    let a = p.0[3] as u32;
                    let v = (p.0[0] as u32 + p.0[1] as u32 + p.0[2] as u32 + 1) / 3;
                    o.0[0] = ((v * a + 255 * (255 - a) + 127) / 255).min(255) as u8;
                }
                enc.encode_image(&grey)
            }
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
    resample: Resample,
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
        let img = resample_to(&img, w, px_height, colour, resample);
        return finish_image(img, 1.0, colour, resample);
    }
    finish_image(img, scale, colour, resample)
}

pub fn finish_image(
    img: image::RgbaImage,
    scale: f32,
    colour: crate::doc::LayerExpression,
    resample: Resample,
) -> image::RgbaImage {
    let mut img = if scale < 1.0 {
        let w = ((img.width() as f32 * scale).round() as u32).max(1);
        let h = ((img.height() as f32 * scale).round() as u32).max(1);
        resample_to(&img, w, h, colour, resample)
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

    /// A layer mask with flat per-TILE coverage. 32768 = "let this out",
    /// 0 = "hold it inside the panel"; an index absent from `cov` has no
    /// mask tile at all, which is full coverage by the unmasked rule.
    fn cap_mask(doc: &mut Document, layer: usize, cov: &[(TileIdx, u16)]) {
        let mut m = crate::doc::LayerMask {
            tiles: std::collections::HashMap::new(),
            enabled: true,
            revision: crate::tile::next_revision(),
        };
        for &(idx, c) in cov {
            let mut t = crate::tile::Tile::new_transparent();
            for y in 0..TILE_SIZE {
                for x in 0..TILE_SIZE {
                    t.set_pixel(x, y, [c, c, c, c]);
                }
            }
            m.tiles.insert(idx, std::sync::Arc::new(t));
        }
        doc.layers[layer].mask = Some(m);
    }

    /// FB-overflow part 2, item 1: a breakout layer's OWN mask caps the
    /// spill. Inside the mask the art gets out over the border; outside it
    /// the art stays exactly where it was, clipped by the panel — the two
    /// halves are complements, so nothing is drawn twice and nothing is
    /// lost.
    #[test]
    fn a_layer_mask_caps_the_spill_to_the_masked_region() {
        use crate::frame::FrameSet;
        // 128² = four 64² tiles; the panel is the middle square, so every
        // tile has both an inside-the-panel and an outside-the-panel part.
        let mut doc = Document::new(128, 128);
        let fs = FrameSet::single_rect([32.0, 32.0, 96.0, 96.0], 4.0);
        let hi = doc.add_frame_folder("F", fs);
        let draw = hi - 1;
        for ty in 0..2 {
            for tx in 0..2 {
                fill_tile(&mut doc, draw, TileIdx::new(tx, ty), [0.0, 1.0, 0.0, 1.0]);
            }
        }
        assert!(doc.set_layer_escape(draw, true));
        // Out through the top-left tile only; the other three are held.
        cap_mask(
            &mut doc,
            draw,
            &[
                (TileIdx::new(0, 0), 32768),
                (TileIdx::new(1, 0), 0),
                (TileIdx::new(0, 1), 0),
                (TileIdx::new(1, 1), 0),
            ],
        );

        let img = composite(&doc, Background::White);
        assert_eq!(
            img.get_pixel(8, 8).0,
            [0, 255, 0, 255],
            "inside the mask: the art is out over the paper"
        );
        assert_eq!(
            img.get_pixel(48, 32).0,
            [0, 255, 0, 255],
            "inside the mask: over the border ink too"
        );
        assert_eq!(
            img.get_pixel(8, 120).0,
            [255, 255, 255, 255],
            "outside the mask: still clipped by the panel"
        );
        let border = img.get_pixel(64, 32).0;
        assert!(
            border[0] < 40 && border[1] < 40,
            "outside the mask: the border ink is untouched, got {border:?}"
        );
        assert_eq!(
            img.get_pixel(80, 80).0,
            [0, 255, 0, 255],
            "outside the mask: inside the panel the art still draws"
        );

        // The complement is exact: at half opacity the two halves must land
        // on the SAME colour inside the panel. A seam here would mean the
        // spilled half is blending over the held half.
        doc.set_layer_opacity(draw, 0.5);
        let img = composite(&doc, Background::White);
        assert_eq!(
            img.get_pixel(40, 40).0,
            img.get_pixel(80, 80).0,
            "no double blend where the mask changes hands"
        );
        assert_eq!(img.get_pixel(40, 40).0, [128, 255, 128, 255]);

        // Disabling the mask puts the all-or-nothing spill back.
        doc.set_layer_opacity(draw, 1.0);
        doc.layers[draw].mask.as_mut().unwrap().enabled = false;
        let img = composite(&doc, Background::White);
        assert_eq!(
            img.get_pixel(8, 120).0,
            [0, 255, 0, 255],
            "mask off: every side spills again"
        );
    }

    /// Item 2: the draws-over set moves WHERE the breakout composites. By
    /// default it lands just above its own frame folder, so a panel stacked
    /// above still covers it; naming that panel puts it on top.
    #[test]
    fn draws_over_seats_the_breakout_above_the_named_panel() {
        use crate::frame::FrameSet;
        let mut doc = Document::new(128, 128);
        // Lower panel + its escapee (green, whole canvas).
        let lo = doc.add_frame_folder("lower", FrameSet::single_rect([8.0, 64.0, 120.0, 120.0], 4.0));
        let burst = lo - 1;
        for ty in 0..2 {
            for tx in 0..2 {
                fill_tile(&mut doc, burst, TileIdx::new(tx, ty), [0.0, 1.0, 0.0, 1.0]);
            }
        }
        assert!(doc.set_layer_escape(burst, true));
        // Upper panel, ABOVE it in the stack, with opaque red art inside.
        let up = doc.add_frame_folder("upper", FrameSet::single_rect([8.0, 8.0, 120.0, 56.0], 4.0));
        let upart = up - 1;
        for tx in 0..2 {
            fill_tile(&mut doc, upart, TileIdx::new(tx, 0), [1.0, 0.0, 0.0, 1.0]);
        }

        // Default: over its own frame folder only — the upper panel wins.
        let img = composite(&doc, Background::White);
        assert_eq!(
            img.get_pixel(64, 30).0,
            [255, 0, 0, 255],
            "default seat: the upper panel still covers the burst"
        );

        // Name the upper panel's ART: the seat lifts out of that sealed
        // folder to the header, so the burst clears the border too.
        let id = doc.layers[upart].id();
        assert!(doc.set_layer_spill_seat(burst, Some(upart)));
        assert!(doc.layers[burst].draws_over.contains(&id));
        let img = composite(&doc, Background::White);
        assert_eq!(
            img.get_pixel(64, 30).0,
            [0, 255, 0, 255],
            "draws over: the burst is on top of the upper panel's art"
        );
        assert_eq!(
            img.get_pixel(64, 8).0,
            [0, 255, 0, 255],
            "…and over that panel's border ink, not inside its mask"
        );

        // Back to the default seat.
        assert!(doc.set_layer_spill_seat(burst, None));
        let img = composite(&doc, Background::White);
        assert_eq!(img.get_pixel(64, 30).0, [255, 0, 0, 255]);
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
        let out = finish_image(img.clone(), 0.5, E::Mono, Resample::Photo);
        assert_eq!(out.dimensions(), (32, 32));
        for px in out.pixels() {
            assert!(
                px.0[0] == 0 || px.0[0] == 255,
                "a resampled grey survived the threshold: {:?}",
                px.0
            );
        }
        // Grey keeps its ramp; colour and an up-scale are both no-ops.
        let grey = finish_image(img.clone(), 1.0, E::Grey, Resample::Photo);
        assert!(grey.pixels().any(|p| p.0[0] != 0 && p.0[0] != 255));
        assert_eq!(
            finish_image(img.clone(), 2.0, E::Colour, Resample::Photo).dimensions(),
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

    /// A page of 1 px hairlines, white paper, `n` lines five pixels apart.
    /// The worst case a mono export meets: the ink is one pixel wide, so
    /// any kernel that averages it into grey hands it straight to the 50 %
    /// threshold to be killed.
    fn hairline_page(w: u32, h: u32, step: u32) -> (image::RgbaImage, Vec<u32>) {
        let mut img = image::RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]));
        let xs: Vec<u32> = (1..w).step_by(step as usize).collect();
        for &x in &xs {
            for y in 0..h {
                img.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
            }
        }
        (img, xs)
    }

    /// Which of `xs` still have ink in the shrunk page, columns mapped by
    /// the same scale.
    fn surviving(out: &image::RgbaImage, xs: &[u32], scale: f32) -> usize {
        xs.iter()
            .filter(|&&x| {
                let ox = ((x as f32 * scale) as u32).min(out.width() - 1);
                // A one-pixel slop either side: which side of a box the
                // line lands on is arithmetic, not survival.
                (ox.saturating_sub(1)..=(ox + 1).min(out.width() - 1))
                    .any(|c| out.get_pixel(c, out.height() / 2).0[0] == 0)
            })
            .count()
    }

    /// FINDING 7 (audit `IO-029`, CSP 処理方法 コミック向き). Shrink a page
    /// of 1 px hairlines by half: under `Comic` every line SURVIVES, under
    /// `Photo` — the kernel every mono export used before 2026-08-29 — most
    /// of them dissolve to grey and die at the threshold.
    ///
    /// The differential is the point. A test that only asserted "comic
    /// keeps the line" would pass on a kernel that keeps everything (a
    /// dilate), and one that only asserted "photo loses it" would pass on a
    /// blank page.
    #[test]
    fn a_hairline_survives_the_comic_shrink() {
        let (img, xs) = hairline_page(120, 32, 5);
        let n = xs.len();

        let comic = finish_image(img.clone(), 0.5, E::Mono, Resample::Comic);
        assert_eq!(comic.dimensions(), (60, 16));
        let kept = surviving(&comic, &xs, 0.5);
        assert_eq!(kept, n, "comic dropped {} of {n} hairlines", n - kept);

        let photo = finish_image(img, 0.5, E::Mono, Resample::Photo);
        let photo_kept = surviving(&photo, &xs, 0.5);
        // Measured 2026-08-29: comic 24/24, photo 0/24. The old export
        // did not thin these lines, it deleted every one of them.
        assert!(
            photo_kept < kept,
            "the photo kernel kept {photo_kept}/{n} hairlines — if it no \
             longer loses them, this test has stopped proving anything"
        );
    }

    /// The other half of the policy: `Comic` is a decision about where to
    /// put a THRESHOLD, so it must be inert wherever there is no threshold.
    /// This is what lets `Comic` be the default without touching a single
    /// colour or greyscale export.
    #[test]
    fn comic_is_a_no_op_for_anything_but_mono() {
        let (img, _) = hairline_page(120, 32, 5);
        for e in [E::Colour, E::Grey] {
            assert!(!Resample::Comic.is_comic(e), "{e:?}");
            assert_eq!(
                finish_image(img.clone(), 0.5, e, Resample::Comic),
                finish_image(img.clone(), 0.5, e, Resample::Photo),
                "{e:?} must not notice the resample policy"
            );
        }
        assert!(Resample::Comic.is_comic(E::Mono));
        assert!(!Resample::Photo.is_comic(E::Mono), "the old path stays reachable");
    }

    /// REGRESSION GUARD for finding 7: `Photo` is the pre-change kernel,
    /// byte for byte, on a non-trivial page. The right-hand side restates
    /// the old `finish_image` body verbatim — if someone "improves" the
    /// shared resample path, the exports that were never broken say so.
    #[test]
    fn photo_output_is_byte_identical_to_the_old_kernel() {
        // Ink, tone-ish dither, a soft ramp and a transparent corner: the
        // four things a real page mixes.
        let mut img = image::RgbaImage::new(97, 61);
        for (x, y, px) in img.enumerate_pixels_mut() {
            px.0 = match (x, y) {
                _ if x < 8 && y < 8 => [0, 0, 0, 0],
                _ if x % 11 == 0 || y % 13 == 0 => [0, 0, 0, 255],
                _ if (x + y) % 3 == 0 => [17, 34, 51, 255],
                _ => [(x * 2) as u8, (y * 4) as u8, 200, 255],
            };
        }
        for e in [E::Colour, E::Grey, E::Mono] {
            for scale in [0.5f32, 350.0 / 600.0, 0.17] {
                let w = ((img.width() as f32 * scale).round() as u32).max(1);
                let h = ((img.height() as f32 * scale).round() as u32).max(1);
                let mut old = image::imageops::resize(
                    &img,
                    w,
                    h,
                    image::imageops::FilterType::Lanczos3,
                );
                if e != E::Colour {
                    for px in old.pixels_mut() {
                        px.0 = reduce_u8(px.0, e);
                    }
                }
                assert_eq!(
                    finish_image(img.clone(), scale, e, Resample::Photo),
                    old,
                    "{e:?} @{scale}"
                );
            }
        }
    }

    /// FINDING 9. The 提出 preset exists, says JPEG, and is DISTINCT from
    /// every other entry (the picker derives its selection by equality —
    /// a duplicate triple makes one preset unreachable).
    #[test]
    fn the_proof_jpeg_preset_is_reachable_and_light() {
        let proof = PRINT_PRESETS
            .iter()
            .find(|p| p.finish.format == ExportFormat::Jpeg)
            .expect("a submission preset that is not a print file");
        assert_eq!(proof.finish.dpi, 150, "phone-openable, not 入稿");
        assert!(!proof.finish.split_spreads, "a spread stays one image");
        assert_eq!(matching_preset(proof.finish), Some(4));
        for (i, p) in PRINT_PRESETS.iter().enumerate() {
            assert_eq!(matching_preset(p.finish), Some(i), "{}", p.name);
        }
        assert_eq!(ExportFormat::Png.ext(), "png");
        assert_eq!(ExportFormat::Jpeg.ext(), "jpg");
        // The default finish is still the untouched old run.
        assert_eq!(ExportFinish::default().format, ExportFormat::Png);
    }

    /// The recorded mono+JPEG decision: a 1-bit finish written as JPEG
    /// becomes a GREY jpeg of the thresholded page — it decodes to one
    /// channel, and the black/white structure is still there.
    #[test]
    fn a_mono_finish_written_as_jpeg_is_a_grey_proof() {
        let (img, _) = hairline_page(64, 64, 5);
        let page = finish_image(img, 0.5, E::Mono, Resample::Comic);
        let dir = std::env::temp_dir().join("mn-export-jpeg-decision");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("proof.jpg");
        save_finished(&page, &path, ExportFormat::Jpeg, PROOF_JPEG_QUALITY, E::Mono).unwrap();
        let back = image::open(&path).unwrap();
        assert!(
            matches!(back, image::DynamicImage::ImageLuma8(_)),
            "a mono proof is a one-channel JPEG, not an RGB one"
        );
        let back = back.to_luma8();
        assert_eq!(back.dimensions(), page.dimensions());
        assert!(
            back.pixels().any(|p| p.0[0] < 64) && back.pixels().any(|p| p.0[0] > 192),
            "the thresholded structure survived the encode"
        );
        // PNG is byte-for-byte what it always was.
        let png = dir.join("proof.png");
        save_finished(&page, &png, ExportFormat::Png, PROOF_JPEG_QUALITY, E::Mono).unwrap();
        assert_eq!(image::open(&png).unwrap().to_rgba8(), page);
        let _ = std::fs::remove_dir_all(&dir);
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
            Resample::Photo,
        );
        assert_eq!((out.width(), out.height()), (200, 400));

        let out = finish_image_cropped(
            img.clone(),
            [100, 100, 300, 500],
            1.0,
            200,
            crate::doc::LayerExpression::Colour,
            Resample::Photo,
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
            Resample::Photo,
        );
        assert_eq!((out.width(), out.height()), (400, 600), "never upsamples");
    }
}
