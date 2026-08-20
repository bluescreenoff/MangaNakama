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
                (linear_to_srgb(acc[c] / n) * 255.0).round().clamp(0.0, 255.0) as u8
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
            if touched {
                for (li, (layer, vis)) in doc.layers.iter().zip(&eff).enumerate() {
                    if !*vis {
                        continue;
                    }
                    let d = layer.depth as usize;
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
                    let base_tile = bases[li].and_then(|b| doc.layers[b].display_tile(idx));
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
                            let m = base_tile
                                .map(|bt| bt.data()[o + 3] as f32 / 32768.0)
                                .unwrap_or(0.0);
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
        assert_eq!(composite_pixel_avg(&doc, 10, 10, 0), composite_pixel(&doc, 10, 10));
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
