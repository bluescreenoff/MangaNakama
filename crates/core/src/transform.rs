//! Edit ▸ Transform: scale/rotate resampling of raster content.
//!
//! # Model
//!
//! A transform is previewed as an overlay (the app draws the source region's
//! pixels through the pending affine) and committed ONCE, as a single undo
//! step, when the user presses Enter. Nothing here touches the GPU compositor
//! or the mip chain — the committed pixels land in ordinary tiles through
//! `Layer::tile_mut`, which the existing upload path sees as normal dirty
//! tiles.
//!
//! # Sampling
//!
//! The committed resample walks DESTINATION pixels and inverse-maps into the
//! source (no holes, no double-writes). Bilinear filtering on premultiplied
//! fix15 — premultiplied means the filter is correct at alpha edges without a
//! divide. A GPU resample path does not exist yet — the `resampled`
//! parameter below is its designed seam; `Affine2` here is the shared math
//! either path would use.
//!
//! Only raster layers transform. Vector layers (frame/balloon/text) keep
//! their geometry editable through their own Object-tool paths instead —
//! resampling their derived rasters would bake them.

use std::collections::HashMap;
use std::sync::Arc;

use crate::doc::{Document, Layer};
use crate::selection::Selection;
use crate::tile::{FIX15_ONE, TILE_SIZE, Tile, TileIdx};

/// A 2D affine transform: `dst = m * src + t` (row-major 2x2 + translation).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine2 {
    pub m: [[f32; 2]; 2],
    pub t: [f32; 2],
}

impl Affine2 {
    pub const IDENTITY: Self = Self {
        m: [[1.0, 0.0], [0.0, 1.0]],
        t: [0.0, 0.0],
    };

    /// Scale + rotate around a pivot, then translate: the Transform tool's
    /// parameter set. `rad` is clockwise in y-down canvas space.
    pub fn scale_rotate_around(
        pivot: [f32; 2],
        sx: f32,
        sy: f32,
        rad: f32,
        translate: [f32; 2],
    ) -> Self {
        let (sin, cos) = rad.sin_cos();
        // R * S
        let m = [[cos * sx, -sin * sy], [sin * sx, cos * sy]];
        // dst = R*S*(p - pivot) + pivot + translate
        let t = [
            pivot[0] + translate[0] - (m[0][0] * pivot[0] + m[0][1] * pivot[1]),
            pivot[1] + translate[1] - (m[1][0] * pivot[0] + m[1][1] * pivot[1]),
        ];
        Self { m, t }
    }

    pub fn apply(&self, p: [f32; 2]) -> [f32; 2] {
        [
            self.m[0][0] * p[0] + self.m[0][1] * p[1] + self.t[0],
            self.m[1][0] * p[0] + self.m[1][1] * p[1] + self.t[1],
        ]
    }

    /// Inverse, or `None` when the transform is degenerate (zero scale).
    pub fn inverse(&self) -> Option<Self> {
        let det = self.m[0][0] * self.m[1][1] - self.m[0][1] * self.m[1][0];
        if det.abs() < 1e-12 {
            return None;
        }
        let inv_det = 1.0 / det;
        let m = [
            [self.m[1][1] * inv_det, -self.m[0][1] * inv_det],
            [-self.m[1][0] * inv_det, self.m[0][0] * inv_det],
        ];
        let t = [
            -(m[0][0] * self.t[0] + m[0][1] * self.t[1]),
            -(m[1][0] * self.t[0] + m[1][1] * self.t[1]),
        ];
        Some(Self { m, t })
    }

    /// Transform an axis-aligned rect, returning the axis-aligned bounds of
    /// the result: [x0, y0, x1, y1].
    pub fn map_rect(&self, r: [f32; 4]) -> [f32; 4] {
        let corners = [
            self.apply([r[0], r[1]]),
            self.apply([r[2], r[1]]),
            self.apply([r[0], r[3]]),
            self.apply([r[2], r[3]]),
        ];
        let mut out = [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ];
        for c in corners {
            out[0] = out[0].min(c[0]);
            out[1] = out[1].min(c[1]);
            out[2] = out[2].max(c[0]);
            out[3] = out[3].max(c[1]);
        }
        out
    }
}

/// The pixels lifted off a layer for transforming: a sparse tile snapshot plus
/// the source bounds (canvas px). The app holds one while the deferred bbox
/// is being dragged; commit consumes it.
#[derive(Clone)]
pub struct FloatSource {
    /// Source tiles (only those intersecting the source rect, alpha outside
    /// the rect/selection zeroed).
    pub tiles: HashMap<TileIdx, Arc<Tile>>,
    /// Tight source bounds in canvas px: [x0, y0, x1, y1] (x1/y1 exclusive).
    pub rect: [i32; 4],
}

impl FloatSource {
    /// One premultiplied fix15 pixel at a canvas position (transparent when
    /// the position has no lifted tile).
    pub fn pixel(&self, x: i32, y: i32) -> [u16; 4] {
        let ti = TileIdx::of_pixel(x, y);
        let Some(tile) = self.tiles.get(&ti) else {
            return [0, 0, 0, 0];
        };
        tile.pixel(
            (x - ti.x * TILE_SIZE as i32) as usize,
            (y - ti.y * TILE_SIZE as i32) as usize,
        )
    }
}

/// Read one premultiplied fix15 pixel out of a sparse tile map (missing tile =
/// transparent).
#[inline]
fn sample_px(tiles: &HashMap<TileIdx, Arc<Tile>>, x: i32, y: i32) -> [f32; 4] {
    let ti = TileIdx::of_pixel(x, y);
    let Some(tile) = tiles.get(&ti) else {
        return [0.0; 4];
    };
    let lx = (x - ti.x * TILE_SIZE as i32) as usize;
    let ly = (y - ti.y * TILE_SIZE as i32) as usize;
    let p = tile.pixel(lx, ly);
    [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]
}

/// Bilinear sample at a fractional source position (premultiplied fix15, so
/// interpolation at alpha edges is correct without unpremultiplying).
fn sample_bilinear(
    tiles: &HashMap<TileIdx, Arc<Tile>>,
    rect: [i32; 4],
    x: f32,
    y: f32,
) -> [f32; 4] {
    // Outside the source rect (with a 1px filter apron) contributes nothing.
    if x < rect[0] as f32 - 1.0
        || y < rect[1] as f32 - 1.0
        || x >= rect[2] as f32
        || y >= rect[3] as f32
    {
        return [0.0; 4];
    }
    let x0 = x.floor();
    let y0 = y.floor();
    let fx = x - x0;
    let fy = y - y0;
    let (x0, y0) = (x0 as i32, y0 as i32);
    let clamp = |px: i32, py: i32| -> [f32; 4] {
        // The rect edge acts as transparent, not clamped — content ends there.
        if px < rect[0] || py < rect[1] || px >= rect[2] || py >= rect[3] {
            [0.0; 4]
        } else {
            sample_px(tiles, px, py)
        }
    };
    let p00 = clamp(x0, y0);
    let p10 = clamp(x0 + 1, y0);
    let p01 = clamp(x0, y0 + 1);
    let p11 = clamp(x0 + 1, y0 + 1);
    let mut out = [0.0f32; 4];
    for c in 0..4 {
        let top = p00[c] * (1.0 - fx) + p10[c] * fx;
        let bot = p01[c] * (1.0 - fx) + p11[c] * fx;
        out[c] = top * (1.0 - fy) + bot * fy;
    }
    out
}

/// The selection-weighted fraction of one premultiplied fix15 pixel:
/// `taken = (v·mv + 127)/255`, the exact split `move_selected` cuts with.
/// Shared by lift and clear so the pair is mass-conserving by construction
/// (taken + remainder == v — one rounding, applied consistently).
#[inline]
fn taken(p: [u16; 4], mv: u32) -> [u16; 4] {
    [
        ((p[0] as u32 * mv + 127) / 255) as u16,
        ((p[1] as u32 * mv + 127) / 255) as u16,
        ((p[2] as u32 * mv + 127) / 255) as u16,
        ((p[3] as u32 * mv + 127) / 255) as u16,
    ]
}

/// Lift the pixels inside `rect` (canvas px, [x0, y0, x1, y1] exclusive)
/// off a layer into a `FloatSource`, weighted by the selection when one
/// exists: each pixel contributes its `cov/255` fraction, so a feathered
/// edge (SE-007 blur) lifts PARTIAL pixels rather than quantizing at the
/// ≥-half line (DECISIONS 8.73 — the recorded r104 follow-up). The source
/// region's pixels are NOT cleared here — `clear_lifted` does that inside
/// the caller's undo op.
pub fn lift_region(layer: &Layer, rect: [i32; 4], selection: Option<&Selection>) -> FloatSource {
    let mut tiles: HashMap<TileIdx, Arc<Tile>> = HashMap::new();
    let t0 = TileIdx::of_pixel(rect[0], rect[1]);
    let t1 = TileIdx::of_pixel(rect[2] - 1, rect[3] - 1);
    for ty in t0.y..=t1.y {
        for tx in t0.x..=t1.x {
            let ti = TileIdx::new(tx, ty);
            let Some(src) = layer.tile(ti) else { continue };
            // Copy the tile, zeroing pixels outside the rect and taking the
            // selection-weighted fraction of the rest.
            let mut out = src.clone();
            let (ox, oy) = ti.origin();
            let mut any = false;
            for ly in 0..TILE_SIZE {
                for lx in 0..TILE_SIZE {
                    let cx = ox + lx as i32;
                    let cy = oy + ly as i32;
                    let inside = cx >= rect[0] && cy >= rect[1] && cx < rect[2] && cy < rect[3];
                    if !inside {
                        out.set_pixel(lx, ly, [0; 4]);
                        continue;
                    }
                    let mv = selection.map_or(255u32, |s| s.coverage(cx, cy) as u32);
                    if mv == 255 {
                        if out.pixel(lx, ly)[3] > 0 {
                            any = true;
                        }
                    } else {
                        let w = taken(out.pixel(lx, ly), mv);
                        out.set_pixel(lx, ly, w);
                        if w[3] > 0 {
                            any = true;
                        }
                    }
                }
            }
            if any {
                tiles.insert(ti, Arc::new(out));
            }
        }
    }
    FloatSource { tiles, rect }
}

/// Erase exactly the fraction `lift_region` took: every in-rect pixel keeps
/// its selection-weighted remainder `v − taken` (fully selected → zero,
/// zero coverage → untouched, feathered edge → partially erased). ONE
/// implementation shared by `commit_transform` and the clipboard Cut, so
/// the lift/clear pair cannot drift apart again. Writes through `tile_mut`
/// — call it inside the caller's undo op.
pub fn clear_lifted(layer: &mut Layer, rect: [i32; 4], selection: Option<&Selection>) {
    let t0 = TileIdx::of_pixel(rect[0], rect[1]);
    let t1 = TileIdx::of_pixel(rect[2] - 1, rect[3] - 1);
    for ty in t0.y..=t1.y {
        for tx in t0.x..=t1.x {
            let ti = TileIdx::new(tx, ty);
            if layer.tile(ti).is_none() {
                continue;
            }
            let (ox, oy) = ti.origin();
            let tile = layer.tile_mut(ti);
            for ly in 0..TILE_SIZE {
                for lx in 0..TILE_SIZE {
                    let cx = ox + lx as i32;
                    let cy = oy + ly as i32;
                    if !(cx >= rect[0] && cy >= rect[1] && cx < rect[2] && cy < rect[3]) {
                        continue;
                    }
                    let mv = selection.map_or(255u32, |s| s.coverage(cx, cy) as u32);
                    if mv == 0 {
                        continue;
                    }
                    let p = tile.pixel(lx, ly);
                    let w = taken(p, mv);
                    tile.set_pixel(lx, ly, [p[0] - w[0], p[1] - w[1], p[2] - w[2], p[3] - w[3]]);
                }
            }
        }
    }
}

/// Commit a transform of `src` through `xf` onto the document's active layer
/// as ONE undo step: when `clear_source` is set, clears the lifted fraction
/// of the source region (`clear_lifted` — selection-weighted), then scatters
/// the resampled destination. Returns `false` (and leaves the document
/// untouched) when the transform is degenerate.
///
/// `clear_source` is TRUE only for floats that were lifted off this layer
/// (Edit ▸ Transform, Flip): moving them must erase where they came from.
/// It is FALSE for every paste — clipboard, OS DIB, material: those pixels
/// were never on the layer, and clearing `src.rect` there erased a
/// rectangle of unrelated art (Copy behaved as Cut the moment the float was
/// dragged; a cross-page paste punched a hole at the source page's
/// coordinates). `selection` should be the LIFT-TIME selection: the clear
/// must mirror what `lift_region` took, not whatever the selection has
/// become while the float was open.
///
/// `mask_to_selection` (owner 2026-08-21, paste into a selection): clamp what
/// this commit WROTE back to `doc.selection`'s coverage, so a paste lands
/// masked to the marching ants instead of splashing over the whole canvas.
/// It reads the LIVE selection (not the lift-time one): the clamp is about
/// where the pixels may land now, not about mirroring an earlier lift. The
/// CALLER decides — the app passes `true` only for paste floats that stamp
/// an existing layer; a paste that creates its own layer gets a
/// non-destructive layer mask instead, and must NOT also clamp here or the
/// coverage would apply twice (a feathered edge would square).
///
/// `resampled`: the seam for a future GPU resample — a full destination
/// pixel buffer with its bounds. No such path exists yet (nothing named
/// `transform_region` is implemented anywhere); every caller passes `None`
/// and resamples here on the CPU, under the inverse-map + bilinear
/// contract a GPU port would have to match.
pub fn commit_transform(
    doc: &mut Document,
    src: &FloatSource,
    xf: &Affine2,
    selection: Option<&Selection>,
    clear_source: bool,
    mask_to_selection: bool,
    resampled: Option<(&[u16], [i32; 4])>,
) -> bool {
    let Some(inv) = xf.inverse() else {
        return false;
    };
    let li = doc.active;
    if !doc.layers[li].paintable() {
        return false;
    }

    // Destination bounds: the transformed source rect, clipped to the canvas.
    let r = xf.map_rect([
        src.rect[0] as f32,
        src.rect[1] as f32,
        src.rect[2] as f32,
        src.rect[3] as f32,
    ]);
    let dst = [
        (r[0].floor() as i32).max(0),
        (r[1].floor() as i32).max(0),
        (r[2].ceil() as i32).min(doc.size.0 as i32),
        (r[3].ceil() as i32).min(doc.size.1 as i32),
    ];
    if dst[0] >= dst[2] || dst[1] >= dst[3] {
        return false;
    }

    doc.begin_op();

    // 1. Clear the lifted fraction of the source region (selection-weighted)
    //    on the layer — lifted floats only; pasted floats never took
    //    anything off this layer (see the doc comment).
    if clear_source {
        clear_lifted(&mut doc.layers[li], src.rect, selection);
    }

    // 2. Scatter the resampled destination (src-over, one pass over dst px).
    {
        let layer = &mut doc.layers[li];
        for cy in dst[1]..dst[3] {
            for cx in dst[0]..dst[2] {
                let px = match resampled {
                    Some((buf, b)) => {
                        // GPU path: read the precomputed destination buffer.
                        if cx < b[0] || cy < b[1] || cx >= b[2] || cy >= b[3] {
                            continue;
                        }
                        let w = (b[2] - b[0]) as usize;
                        let o = ((cy - b[1]) as usize * w + (cx - b[0]) as usize) * 4;
                        [
                            buf[o] as f32,
                            buf[o + 1] as f32,
                            buf[o + 2] as f32,
                            buf[o + 3] as f32,
                        ]
                    }
                    None => {
                        // CPU path: inverse-map the pixel centre into source
                        // space and bilinear-sample.
                        let sp = inv.apply([cx as f32 + 0.5, cy as f32 + 0.5]);
                        sample_bilinear(&src.tiles, src.rect, sp[0] - 0.5, sp[1] - 0.5)
                    }
                };
                if px[3] < 0.5 {
                    continue; // fully transparent — don't touch the tile
                }
                let ti = TileIdx::of_pixel(cx, cy);
                let lx = (cx - ti.x * TILE_SIZE as i32) as usize;
                let ly = (cy - ti.y * TILE_SIZE as i32) as usize;
                let tile = layer.tile_mut(ti);
                let d = tile.pixel(lx, ly);
                // src-over in premultiplied fix15.
                let sa = px[3] / FIX15_ONE as f32;
                let out = [
                    (px[0] + d[0] as f32 * (1.0 - sa))
                        .round()
                        .min(FIX15_ONE as f32) as u16,
                    (px[1] + d[1] as f32 * (1.0 - sa))
                        .round()
                        .min(FIX15_ONE as f32) as u16,
                    (px[2] + d[2] as f32 * (1.0 - sa))
                        .round()
                        .min(FIX15_ONE as f32) as u16,
                    (px[3] + d[3] as f32 * (1.0 - sa))
                        .round()
                        .min(FIX15_ONE as f32) as u16,
                ];
                tile.set_pixel(lx, ly, out);
            }
        }
    }

    // 3. Paste into a selection: clamp the op's own writes back to the
    //    selection's weighted coverage. The op is still open, so the
    //    pre-images this needs are still recorded — outside the ants the
    //    layer goes back to exactly what it held, inside a feathered band
    //    the pasted pixels blend by coverage.
    if mask_to_selection {
        doc.mask_op_to_selection();
    }

    doc.end_op()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Audit M2 (rounds 50-68): is a flip an EXACT pixel permutation? One
    /// dot, one horizontal flip — the inverse-map + bilinear convention
    /// should degenerate to weights 1/0 here (2·pivot is an integer, so
    /// every sampled position is integral), landing the dot byte-identical
    /// at the mirrored coordinate with no bilinear halo. The app-level
    /// `flip_layer_content_mirrors_and_undoes` explains its 3×3
    /// neighbourhood tolerance with a half-pixel claim this test checks.
    #[test]
    fn flip_is_an_exact_pixel_permutation() {
        let mut doc = Document::new(256, 256);
        doc.begin_op();
        {
            let ti = TileIdx::of_pixel(150, 90);
            let t = doc.layers[0].tile_mut(ti);
            t.set_pixel(
                (150 - ti.origin().0) as usize,
                (90 - ti.origin().1) as usize,
                [12345, 23456, 32768, 32768],
            );
        }
        doc.end_op();
        // The populated-tile rect, as the app's standalone flip lifts it.
        let rect = [128, 64, 192, 128];
        let src = lift_region(&doc.layers[0], rect, None);
        let pivot = [
            (rect[0] + rect[2]) as f32 * 0.5,
            (rect[1] + rect[3]) as f32 * 0.5,
        ];
        let xf = Affine2::scale_rotate_around(pivot, -1.0, 1.0, 0.0, [0.0, 0.0]);
        assert!(commit_transform(
            &mut doc, &src, &xf, None, true, false, None
        ));

        let read = |x: i32, y: i32| -> [u16; 4] {
            let ti = TileIdx::of_pixel(x, y);
            doc.layers[0]
                .tile(ti)
                .map(|t| t.pixel((x - ti.origin().0) as usize, (y - ti.origin().1) as usize))
                .unwrap_or([0; 4])
        };
        // Pixel [150,151) reflects about x=160 onto [169,170): dest cx
        // satisfies 319 − cx = 150 (the sampled source index).
        let mx = 319 - 150;
        assert_eq!(
            read(mx, 90),
            [12345, 23456, 32768, 32768],
            "the dot lands byte-identical at the mirrored coordinate"
        );
        assert_eq!(read(150, 90), [0; 4], "the source position is cleared");
        for dy in -1..=1i32 {
            for dx in -1..=1i32 {
                if (dx, dy) != (0, 0) {
                    assert_eq!(
                        read(mx + dx, 90 + dy),
                        [0; 4],
                        "no bilinear halo around the landing site"
                    );
                }
            }
        }
    }

    fn doc_with_box(x0: i32, y0: i32, x1: i32, y1: i32) -> Document {
        let mut doc = Document::new(256, 256);
        for cy in y0..y1 {
            for cx in x0..x1 {
                let ti = TileIdx::of_pixel(cx, cy);
                let lx = (cx - ti.x * TILE_SIZE as i32) as usize;
                let ly = (cy - ti.y * TILE_SIZE as i32) as usize;
                doc.layers[0]
                    .tile_mut(ti)
                    .set_pixel(lx, ly, [0, 0, 0, FIX15_ONE as u16]);
            }
        }
        doc
    }

    fn alpha_at(doc: &Document, x: i32, y: i32) -> u16 {
        let ti = TileIdx::of_pixel(x, y);
        doc.layers[0]
            .tile(ti)
            .map(|t| {
                t.pixel(
                    (x - ti.x * TILE_SIZE as i32) as usize,
                    (y - ti.y * TILE_SIZE as i32) as usize,
                )[3]
            })
            .unwrap_or(0)
    }

    #[test]
    fn affine_inverse_roundtrips() {
        let xf = Affine2::scale_rotate_around([50.0, 60.0], 1.5, 0.75, 0.3, [12.0, -4.0]);
        let inv = xf.inverse().unwrap();
        let p = [123.0, 45.0];
        let q = inv.apply(xf.apply(p));
        assert!((q[0] - p[0]).abs() < 1e-3 && (q[1] - p[1]).abs() < 1e-3);
    }

    #[test]
    fn degenerate_scale_refuses() {
        let xf = Affine2::scale_rotate_around([0.0, 0.0], 0.0, 1.0, 0.0, [0.0, 0.0]);
        assert!(xf.inverse().is_none());
        let mut doc = doc_with_box(10, 10, 20, 20);
        let src = lift_region(&doc.layers[0], [10, 10, 20, 20], None);
        assert!(!commit_transform(
            &mut doc, &src, &xf, None, true, false, None
        ));
        assert!(!doc.can_undo(), "refused transform pushes no undo");
    }

    #[test]
    fn translate_moves_the_box_one_undo_step() {
        let mut doc = doc_with_box(10, 10, 20, 20);
        let src = lift_region(&doc.layers[0], [10, 10, 20, 20], None);
        let xf = Affine2::scale_rotate_around([15.0, 15.0], 1.0, 1.0, 0.0, [100.0, 50.0]);
        assert!(commit_transform(
            &mut doc, &src, &xf, None, true, false, None
        ));

        assert_eq!(alpha_at(&doc, 15, 15), 0, "source cleared");
        assert_eq!(
            alpha_at(&doc, 115, 65),
            FIX15_ONE as u16,
            "landed at +100,+50"
        );

        // ONE undo step restores both the clear and the scatter.
        assert!(doc.undo());
        assert_eq!(
            alpha_at(&doc, 15, 15),
            FIX15_ONE as u16,
            "undo restored source"
        );
        assert_eq!(alpha_at(&doc, 115, 65), 0, "undo removed destination");
        assert!(!doc.can_undo(), "exactly one undo group");
    }

    #[test]
    fn double_scale_covers_double_extent() {
        let mut doc = doc_with_box(64, 64, 96, 96); // 32px box
        let src = lift_region(&doc.layers[0], [64, 64, 96, 96], None);
        let xf = Affine2::scale_rotate_around([64.0, 64.0], 2.0, 2.0, 0.0, [0.0, 0.0]);
        assert!(commit_transform(
            &mut doc, &src, &xf, None, true, false, None
        ));
        // Pivot corner stays; the far corner is now ~64px out.
        assert_eq!(alpha_at(&doc, 65, 65), FIX15_ONE as u16);
        assert_eq!(
            alpha_at(&doc, 120, 120),
            FIX15_ONE as u16,
            "scaled area covered"
        );
        assert_eq!(alpha_at(&doc, 130, 130), 0, "beyond the scaled box");
    }

    #[test]
    fn rotate_90_lands_where_expected() {
        // Asymmetric box so rotation is observable: 20 wide, 10 tall.
        let mut doc = doc_with_box(100, 100, 120, 110);
        let src = lift_region(&doc.layers[0], [100, 100, 120, 110], None);
        let xf = Affine2::scale_rotate_around(
            [110.0, 105.0],
            1.0,
            1.0,
            std::f32::consts::FRAC_PI_2,
            [0.0, 0.0],
        );
        assert!(commit_transform(
            &mut doc, &src, &xf, None, true, false, None
        ));
        // After 90° cw around (110,105): the box is 10 wide, 20 tall.
        assert_eq!(alpha_at(&doc, 110, 105), FIX15_ONE as u16, "centre stays");
        assert_eq!(alpha_at(&doc, 112, 112), FIX15_ONE as u16, "now taller");
        assert_eq!(alpha_at(&doc, 118, 105), 0, "no longer wide");
    }

    #[test]
    fn lift_respects_rect_bounds() {
        let doc = doc_with_box(0, 0, 64, 64);
        let src = lift_region(&doc.layers[0], [10, 10, 20, 20], None);
        // Only pixels inside the rect were lifted.
        assert_eq!(sample_px(&src.tiles, 15, 15)[3], FIX15_ONE as f32);
        assert_eq!(sample_px(&src.tiles, 5, 5)[3], 0.0);
        assert_eq!(sample_px(&src.tiles, 25, 25)[3], 0.0);
    }

    #[test]
    fn float_source_pixel_reads_lifted_tiles() {
        let doc = doc_with_box(10, 10, 20, 20);
        let src = lift_region(&doc.layers[0], [10, 10, 20, 20], None);
        assert_eq!(src.pixel(15, 15), [0, 0, 0, FIX15_ONE as u16]);
        assert_eq!(
            src.pixel(5, 5),
            [0, 0, 0, 0],
            "outside the lift reads transparent"
        );
        assert_eq!(src.pixel(25, 25), [0, 0, 0, 0]);
    }

    fn px_at(doc: &Document, x: i32, y: i32) -> [u16; 4] {
        let ti = TileIdx::of_pixel(x, y);
        doc.layers[0]
            .tile(ti)
            .map(|t| {
                t.pixel(
                    (x - ti.x * TILE_SIZE as i32) as usize,
                    (y - ti.y * TILE_SIZE as i32) as usize,
                )
            })
            .unwrap_or([0; 4])
    }

    /// DECISIONS 8.73 (r107): lift/clear read the selection as a WEIGHT —
    /// each pixel contributes its cov/255 fraction — instead of the ≥-half
    /// predicate, which quantized a feathered edge (SE-007 blur) to a hard
    /// cut and left the sub-threshold ring behind at the source. The split
    /// is `move_selected`'s exact arithmetic, so lift + clear conserves
    /// pixel mass by construction (taken + remainder == v, one rounding).
    #[test]
    fn lift_and_clear_split_by_selection_weight() {
        let mut doc = doc_with_box(30, 30, 90, 90);
        let mut sel = Selection::from_rect(&doc, 40.0, 40.0, 58.0, 80.0);
        sel = sel.blur(&doc, 6);
        assert!(
            (40..80).any(|x| (1..255).contains(&sel.coverage(x, 60))),
            "the blur produced a feather to exercise"
        );

        // Snapshot the region before anything touches it.
        let mut before = vec![[0u16; 4]; 60 * 60];
        for y in 30..90 {
            for x in 30..90 {
                before[(y - 30) as usize * 60 + (x - 30) as usize] = px_at(&doc, x, y);
            }
        }

        let src = lift_region(&doc.layers[0], [40, 40, 80, 80], Some(&sel));
        let mut partial = 0;
        for y in 40..80 {
            for x in 40..80 {
                let v = before[(y - 30) as usize * 60 + (x - 30) as usize];
                let mv = sel.coverage(x, y) as u32;
                let w = src.pixel(x, y);
                for c in 0..4 {
                    assert_eq!(
                        w[c],
                        ((v[c] as u32 * mv + 127) / 255) as u16,
                        "lifted channel {c} at ({x},{y}) is the cov/255 fraction"
                    );
                }
                if mv > 0 && mv < crate::selection::SEL_ON as u32 {
                    partial += 1;
                }
            }
        }
        assert!(partial > 0, "the sub-threshold ring was exercised");

        clear_lifted(&mut doc.layers[0], [40, 40, 80, 80], Some(&sel));
        for y in 30..90 {
            for x in 30..90 {
                let v0 = before[(y - 30) as usize * 60 + (x - 30) as usize];
                let now = px_at(&doc, x, y);
                if !(x >= 40 && y >= 40 && x < 80 && y < 80) {
                    assert_eq!(now, v0, "outside the rect nothing changes ({x},{y})");
                    continue;
                }
                let mv = sel.coverage(x, y) as u32;
                for c in 0..4 {
                    let taken = ((v0[c] as u32 * mv + 127) / 255) as u16;
                    assert_eq!(
                        now[c],
                        v0[c] - taken,
                        "cleared channel {c} at ({x},{y}) keeps exactly the complement"
                    );
                }
            }
        }
    }

    /// The r104 recorded gap, closed end to end: blur a selection, transform
    /// through it — every pixel now moves by exactly its coverage fraction.
    /// On the old ≥-half predicate the sub-128 ring STAYED at the source,
    /// fully opaque, while the rest moved with a hard edge.
    #[test]
    fn feathered_transform_translates_partial_edges() {
        let mut doc = doc_with_box(40, 40, 80, 80);
        let mut sel = Selection::from_rect(&doc, 40.0, 40.0, 58.0, 80.0);
        sel = sel.blur(&doc, 6);
        let src = lift_region(&doc.layers[0], [40, 40, 80, 80], Some(&sel));
        // Integer translate: the inverse map lands on exact pixel centres
        // (the flip-is-exact proof), so expectations are closed-form.
        let xf = Affine2::scale_rotate_around([60.0, 60.0], 1.0, 1.0, 0.0, [64.0, 0.0]);
        assert!(commit_transform(
            &mut doc,
            &src,
            &xf,
            Some(&sel),
            true,
            false,
            None
        ));

        let mut partial = 0;
        for x in 40..80 {
            let mv = sel.coverage(x, 60) as u32;
            let taken_a = ((FIX15_ONE as u32 * mv + 127) / 255) as u16;
            let rem_a = FIX15_ONE as u16 - taken_a;
            assert_eq!(
                alpha_at(&doc, x, 60),
                rem_a,
                "source keeps its unlifted fraction at cov {mv}"
            );
            assert_eq!(
                alpha_at(&doc, x + 64, 60),
                taken_a,
                "destination receives exactly the lifted fraction at cov {mv}"
            );
            if mv > 0 && mv < crate::selection::SEL_ON as u32 {
                assert!(rem_a > 0 && taken_a > 0, "the ring is partial, not cut");
                partial += 1;
            }
        }
        assert!(partial > 0, "the sub-threshold ring was exercised");

        // One undo restores the whole feathered transaction.
        assert!(doc.undo());
        for x in 40..80 {
            assert_eq!(alpha_at(&doc, x, 60), FIX15_ONE as u16);
            assert_eq!(alpha_at(&doc, x + 64, 60), 0);
        }
    }
}
