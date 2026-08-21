//! Tonal correction (CSP 色調補正): the pure per-pixel maps, and the one
//! whole-layer applier that both the live preview and the undoable commit
//! go through.
//!
//! Two halves, deliberately:
//!
//! * [`Adjust`] is a pure function of a **straight** (unpremultiplied) RGB
//!   triple in 0..1. It never sees alpha, never sees a tile, and is where
//!   the maths is tested.
//! * [`correct_tile`] is the only place pixels move. Preview and commit call
//!   the same function with the same arguments, which is what makes the
//!   preview honest — there is no second code path that could disagree.
//!
//! **Alpha is never touched by any correction here.** That is a load-bearing
//! property, not an accident: it is why the transparent-pixel lock
//! (`Document::mask_op_to_alpha`) is deliberately NOT called on this path.
//! That helper assumes a src-over paint and, given an unchanged alpha, damps
//! the colour change on every semi-transparent pixel — it would silently
//! halve a correction on a 50%-alpha pixel. Nothing to clamp, so nothing is
//! clamped.
//!
//! **CPU only, and that is a deferral, not a design.** The standing product
//! rule is GPU-first for pixel processing, but `mn-gpu` today exposes the
//! compositor and the dab compute path and nothing a whole-layer filter
//! could hang off; every existing whole-layer pixel op in this tree
//! (`Document::convert_brightness_to_opacity`, the gradient, the fills) is
//! CPU too. Landing this in `mn-core` keeps it portable and testable with
//! plain `cargo test` per the crate contract. The GPU path is the follow-up
//! and wants a general "run this kernel over a layer's tiles" seam, which
//! the blur family needs as well — one seam for both, not one each.

use std::sync::Arc;

use crate::blend::{FIX15_ONE_F, f32_to_fix15};
use crate::doc::Document;
use crate::tile::{TILE_PIXELS, Tile, TileIdx};

/// Rec.709 luma, the same coefficients `convert_brightness_to_opacity`
/// already uses (written over 32768 so the shared origin stays visible).
const LUMA: [f32; 3] = [6967.0 / 32768.0, 23435.0 / 32768.0, 2366.0 / 32768.0];

/// How many control points a [`Adjust::ToneCurve`] can carry.
///
/// A fixed capacity, not a `Vec`: [`Adjust`] is `Copy` (the preview copies it
/// every frame and the command queue moves it around), and CSP's curve dialog
/// is a handful of handles anyway.
pub const TONE_CURVE_MAX: usize = 8;

/// One tonal correction and its parameters.
///
/// Slider ranges are normalised: CSP shows −100..100, we carry −1..1, and
/// the dialog does the ×100 for display only.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Adjust {
    /// TC-004. `brightness` is an offset on 0..1; `contrast` pivots around
    /// mid grey. Both 0 = no change.
    BrightnessContrast { brightness: f32, contrast: f32 },
    /// TC-005. `hue` in degrees; `saturation` and `luminosity` scale HSV's
    /// S and V by `1 + v`, so −1 flattens and +1 doubles. All 0 = no change.
    HueSaturation {
        hue: f32,
        saturation: f32,
        luminosity: f32,
    },
    /// TC-006. Output levels per channel; CSP's range is 2..=20.
    Posterize { levels: u32 },
    /// TC-007, CSP's "Reverse gradient": RGB invert. No parameters, so it
    /// runs straight off the menu with no dialog — as it does in CSP.
    Invert,
    /// TC-011. Luma at or above `threshold` goes white, below goes black.
    /// The print-prep operation: bitonal lineart in one step.
    Binarize { threshold: f32 },
    /// TC-002 (CSP レベル補正). The scanner operation: pull the input black
    /// and white points in onto the ink and the paper, bend the midtones with
    /// `gamma`, then re-spread onto the output range. All five are 0..1
    /// except `gamma` (0.1..10, 1.0 = no bend); CSP shows them 0..255.
    ///
    /// Rest is `in_black = 0`, `in_white = 1`, `gamma = 1`, `out_black = 0`,
    /// `out_white = 1`.
    Levels {
        in_black: f32,
        in_white: f32,
        gamma: f32,
        out_black: f32,
        out_white: f32,
    },
    /// TC-003 (CSP トーンカーブ). `n` control points on the unit square, in
    /// `pts[..n]`, sorted by x with the ends pinned at x = 0 and x = 1; the
    /// slots past `n` are dead and must be left at their default so `==` and
    /// [`Self::is_identity`] stay exact.
    ///
    /// The interpolation is monotone cubic (Fritsch–Carlson): a plain
    /// Catmull-Rom through hand-placed points overshoots, and an overshoot on
    /// a tone curve is a visible dark halo in a gradient that the user never
    /// asked for.
    ToneCurve {
        pts: [[f32; 2]; TONE_CURVE_MAX],
        n: u8,
    },
}

impl Adjust {
    /// Menu defaults, so a menu item stays one line.
    pub const BRIGHTNESS_CONTRAST: Self = Self::BrightnessContrast {
        brightness: 0.0,
        contrast: 0.0,
    };
    pub const HUE_SATURATION: Self = Self::HueSaturation {
        hue: 0.0,
        saturation: 0.0,
        luminosity: 0.0,
    };
    pub const POSTERIZE: Self = Self::Posterize { levels: 8 };
    pub const BINARIZE: Self = Self::Binarize { threshold: 0.5 };
    pub const LEVELS: Self = Self::Levels {
        in_black: 0.0,
        in_white: 1.0,
        gamma: 1.0,
        out_black: 0.0,
        out_white: 1.0,
    };
    /// The straight line, as two points — the identity curve the dialog opens
    /// on and the one [`Self::is_identity`] recognises.
    pub const TONE_CURVE: Self = Self::ToneCurve {
        pts: Self::TONE_CURVE_REST,
        n: 2,
    };
    /// The default point array. Dead slots are `[0, 0]`, and every editor must
    /// put them back that way — a stale value in `pts[5]` compares unequal and
    /// would make an identity curve push an undo step.
    pub const TONE_CURVE_REST: [[f32; 2]; TONE_CURVE_MAX] = {
        let mut p = [[0.0; 2]; TONE_CURVE_MAX];
        p[1] = [1.0, 1.0];
        p
    };

    /// The name the History palette shows, and the dialog's title.
    pub fn label(&self) -> &'static str {
        match self {
            Adjust::BrightnessContrast { .. } => "Brightness/Contrast",
            Adjust::HueSaturation { .. } => "Hue/Saturation/Luminosity",
            Adjust::Posterize { .. } => "Posterization",
            Adjust::Invert => "Reverse gradient",
            Adjust::Binarize { .. } => "Binarization",
            Adjust::Levels { .. } => "Levels",
            Adjust::ToneCurve { .. } => "Tone curve",
        }
    }

    /// True when this correction provably cannot change a pixel. Apply
    /// refuses in that case, so a dialog dismissed with every slider at
    /// rest never pushes an empty undo step.
    ///
    /// Posterize, Invert and Binarize quantize unconditionally and are never
    /// identity — a layer that happens to already be posterized still gets
    /// its undo step, which is the honest answer (we do not scan the pixels
    /// to find out).
    pub fn is_identity(&self) -> bool {
        match *self {
            Adjust::BrightnessContrast {
                brightness,
                contrast,
            } => brightness == 0.0 && contrast == 0.0,
            Adjust::HueSaturation {
                hue,
                saturation,
                luminosity,
            } => hue == 0.0 && saturation == 0.0 && luminosity == 0.0,
            Adjust::Levels {
                in_black,
                in_white,
                gamma,
                out_black,
                out_white,
            } => {
                in_black == 0.0
                    && in_white == 1.0
                    && gamma == 1.0
                    && out_black == 0.0
                    && out_white == 1.0
            }
            // Every control point on the diagonal: the Fritsch–Carlson
            // tangents are then all 1 and the curve IS the line, so this is a
            // provable no-op and not a guess. The common case is the untouched
            // two-point default.
            Adjust::ToneCurve { pts, n } => {
                let n = n as usize;
                n >= 2
                    && pts[0] == [0.0, 0.0]
                    && pts[n - 1] == [1.0, 1.0]
                    && pts[..n].iter().all(|p| p[0] == p[1])
            }
            _ => false,
        }
    }

    /// The correction itself: straight RGB in 0..1 → straight RGB in 0..1.
    pub fn map(&self, rgb: [f32; 3]) -> [f32; 3] {
        match *self {
            Adjust::BrightnessContrast {
                brightness,
                contrast,
            } => {
                // Contrast around mid grey first, then the brightness
                // offset (GIMP's order). k → ∞ as contrast → 1, so the
                // slider stops short of 1 — a true infinite contrast is
                // Binarize's job and it has its own row.
                let c = contrast.clamp(-1.0, 0.99);
                let k = (1.0 + c) / (1.0 - c);
                let mut out = [0.0f32; 3];
                for i in 0..3 {
                    out[i] = ((rgb[i] - 0.5) * k + 0.5 + brightness).clamp(0.0, 1.0);
                }
                out
            }
            Adjust::HueSaturation {
                hue,
                saturation,
                luminosity,
            } => {
                let mut hsv = rgb_to_hsv(rgb);
                hsv[0] = (hsv[0] + hue).rem_euclid(360.0);
                hsv[1] = (hsv[1] * (1.0 + saturation)).clamp(0.0, 1.0);
                hsv[2] = (hsv[2] * (1.0 + luminosity)).clamp(0.0, 1.0);
                hsv_to_rgb(hsv)
            }
            Adjust::Posterize { levels } => {
                // n buckets, and the ends stay pinned: 0 → 0, 1 → 1.
                let n = levels.clamp(2, 256) as f32;
                let mut out = [0.0f32; 3];
                for i in 0..3 {
                    let q = (rgb[i].clamp(0.0, 1.0) * n).floor().min(n - 1.0);
                    out[i] = q / (n - 1.0);
                }
                out
            }
            Adjust::Invert => [1.0 - rgb[0], 1.0 - rgb[1], 1.0 - rgb[2]],
            Adjust::Binarize { threshold } => {
                let luma = LUMA[0] * rgb[0] + LUMA[1] * rgb[1] + LUMA[2] * rgb[2];
                if luma >= threshold {
                    [1.0; 3]
                } else {
                    [0.0; 3]
                }
            }
            Adjust::Levels {
                in_black,
                in_white,
                gamma,
                out_black,
                out_white,
            } => {
                // A degenerate or inverted input range would divide by zero
                // or flip the image; the floor turns it into the hard
                // threshold the user is asking for by dragging them together.
                let span = (in_white - in_black).max(1e-6);
                let inv_g = 1.0 / gamma.clamp(0.1, 10.0);
                let mut out = [0.0f32; 3];
                for i in 0..3 {
                    let t = ((rgb[i] - in_black) / span).clamp(0.0, 1.0).powf(inv_g);
                    out[i] = (out_black + t * (out_white - out_black)).clamp(0.0, 1.0);
                }
                out
            }
            Adjust::ToneCurve { pts, n } => {
                let p = &pts[..(n as usize).min(TONE_CURVE_MAX)];
                [
                    curve_eval(p, rgb[0]),
                    curve_eval(p, rgb[1]),
                    curve_eval(p, rgb[2]),
                ]
            }
        }
    }
}

/// Monotone cubic (Fritsch–Carlson) through `pts`, evaluated at `x`.
///
/// The limiter is the whole point: it clips the Hermite tangents so a segment
/// between two monotone points can never leave the box those points bound.
/// Straight Catmull-Rom does leave it, and the ringing lands in the midtones
/// of a gradient where it reads as a band the artist did not draw.
///
/// Fewer than two points degenerates gracefully: none = identity, one = the
/// constant that point names.
fn curve_eval(pts: &[[f32; 2]], x: f32) -> f32 {
    let n = pts.len();
    if n == 0 {
        return x.clamp(0.0, 1.0);
    }
    if n == 1 {
        return pts[0][1].clamp(0.0, 1.0);
    }
    // Outside the control range the end value stands — the ends are pinned to
    // x = 0 and x = 1 by the editor, so this is the boundary guard, not a
    // routine path.
    if x <= pts[0][0] {
        return pts[0][1].clamp(0.0, 1.0);
    }
    if x >= pts[n - 1][0] {
        return pts[n - 1][1].clamp(0.0, 1.0);
    }

    let mut d = [0.0f32; TONE_CURVE_MAX]; // secant slopes, d[..n-1]
    for i in 0..n - 1 {
        let h = (pts[i + 1][0] - pts[i][0]).max(1e-6);
        d[i] = (pts[i + 1][1] - pts[i][1]) / h;
    }
    let mut m = [0.0f32; TONE_CURVE_MAX]; // tangents, m[..n]
    m[0] = d[0];
    m[n - 1] = d[n - 2];
    for i in 1..n - 1 {
        m[i] = 0.5 * (d[i - 1] + d[i]);
    }
    for i in 0..n - 1 {
        if d[i] == 0.0 {
            // A flat run must stay flat, or the cubic bulges through it.
            m[i] = 0.0;
            m[i + 1] = 0.0;
            continue;
        }
        let mut a = m[i] / d[i];
        let mut b = m[i + 1] / d[i];
        // A tangent that disagrees in sign with its secant is an overshoot
        // waiting to happen; zero it, then keep (a, b) inside the circle of
        // radius 3 that Fritsch–Carlson proves monotone.
        if a < 0.0 {
            a = 0.0;
        }
        if b < 0.0 {
            b = 0.0;
        }
        let s = a * a + b * b;
        if s > 9.0 {
            let tau = 3.0 / s.sqrt();
            a *= tau;
            b *= tau;
        }
        m[i] = a * d[i];
        m[i + 1] = b * d[i];
    }

    let i = pts[..n - 1]
        .iter()
        .rposition(|p| p[0] <= x)
        .unwrap_or(0)
        .min(n - 2);
    let h = (pts[i + 1][0] - pts[i][0]).max(1e-6);
    let t = ((x - pts[i][0]) / h).clamp(0.0, 1.0);
    let (t2, t3) = (t * t, t * t * t);
    let y = (2.0 * t3 - 3.0 * t2 + 1.0) * pts[i][1]
        + (t3 - 2.0 * t2 + t) * h * m[i]
        + (-2.0 * t3 + 3.0 * t2) * pts[i + 1][1]
        + (t3 - t2) * h * m[i + 1];
    y.clamp(0.0, 1.0)
}

fn rgb_to_hsv(c: [f32; 3]) -> [f32; 3] {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let d = max - min;
    let h = if d <= 0.0 {
        0.0
    } else if max == c[0] {
        60.0 * (((c[1] - c[2]) / d) % 6.0)
    } else if max == c[1] {
        60.0 * ((c[2] - c[0]) / d + 2.0)
    } else {
        60.0 * ((c[0] - c[1]) / d + 4.0)
    };
    let s = if max > 0.0 { d / max } else { 0.0 };
    [h.rem_euclid(360.0), s, max]
}

fn hsv_to_rgb(hsv: [f32; 3]) -> [f32; 3] {
    let h = hsv[0].rem_euclid(360.0);
    let s = hsv[1].clamp(0.0, 1.0);
    let v = hsv[2].clamp(0.0, 1.0);
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r + m, g + m, b + m]
}

/// THE pixel path, shared by the preview and the commit.
///
/// `src` is the untouched pre-image, `dst` the tile being written (they may
/// hold different data — the preview's `dst` still carries the *previous*
/// parameters, which is exactly why every pixel is written from `src` and
/// never from `dst`). `mask` is the selection's per-pixel coverage for this
/// tile, `None` meaning "no selection, correct everything".
///
/// Pixels are premultiplied fix15. The correction is defined on the colour
/// you SEE, so each pixel is unpremultiplied first and re-premultiplied
/// after — skipping that step is the single easiest way to get this wrong,
/// and it shows up only on semi-transparent pixels (a 50%-alpha mid grey
/// would binarize as if it were half as bright).
pub fn correct_tile(dst: &mut [u16], src: &[u16], adj: &Adjust, mask: Option<&[u8; TILE_PIXELS]>) {
    for p in 0..TILE_PIXELS {
        let i = p * 4;
        let cov = mask.map_or(255u32, |m| m[p] as u32);
        let a = src[i + 3] as u32;
        if a == 0 || cov == 0 {
            // Fully transparent (nothing to correct) or unselected: the
            // source stands. This is the branch that keeps pixels outside
            // the selection byte-identical.
            dst[i..i + 4].copy_from_slice(&src[i..i + 4]);
            continue;
        }
        // Unpremultiply. `src[i]` and `a` are BOTH fix15, so the scale
        // cancels and their plain ratio is already the straight 0..1 colour;
        // dividing by the alpha's *normalised* form instead (`FIX15_ONE / a`,
        // which is what this was) leaves the result 32768x too large, and the
        // clamp below then flattens every non-black pixel to full white. The
        // correction still ran — on white — so the preview looked alive and
        // the maths tests, which only ever call `Adjust::map` directly, saw
        // nothing.
        let inv = 1.0 / a as f32;
        let straight = [
            (src[i] as f32 * inv).min(1.0),
            (src[i + 1] as f32 * inv).min(1.0),
            (src[i + 2] as f32 * inv).min(1.0),
        ];
        let out = adj.map(straight);
        let af = a as f32 / FIX15_ONE_F;
        for c in 0..3 {
            // Re-premultiply, then never exceed alpha (rounding could).
            let new = f32_to_fix15(out[c].clamp(0.0, 1.0) * af).min(src[i + 3]) as u32;
            dst[i + c] = if cov == 255 {
                new as u16
            } else {
                // Partial selection coverage blends toward the source, the
                // same formula `mask_op_to_selection` uses.
                ((new * cov + src[i + c] as u32 * (255 - cov) + 127) / 255) as u16
            };
        }
        dst[i + 3] = src[i + 3];
    }
}

impl Document {
    /// The tiles a correction would touch on `index`: the layer's populated
    /// tiles, minus any the selection cannot reach. `None` = the layer
    /// refuses (folder / vector / locked).
    ///
    /// Only populated tiles, because every correction here preserves alpha
    /// and a fully transparent pixel therefore stays exactly transparent —
    /// an empty tile has no work in it and materializing one would only
    /// grow the file.
    fn adjust_tiles(&self, index: usize) -> Option<Vec<TileIdx>> {
        let l = self.layers.get(index)?;
        if !l.paintable() || l.lock {
            return None;
        }
        let sel = self.selection.as_ref();
        Some(
            l.tiles()
                .map(|(i, _)| i)
                .filter(|i| sel.is_none_or(|s| s.tile_mask(*i).is_some()))
                .collect(),
        )
    }

    /// Cheap pre-image of everything a correction would touch on `index`
    /// (Arc clones — no pixel is copied here). The live preview's restore
    /// point, and the commit's source of truth.
    pub fn adjust_snapshot(&self, index: usize) -> Vec<(TileIdx, Arc<Tile>)> {
        let Some(idxs) = self.adjust_tiles(index) else {
            return Vec::new();
        };
        idxs.into_iter()
            .filter_map(|i| self.layers[index].tile_arc(i).map(|t| (i, t.clone())))
            .collect()
    }

    /// Paint a live preview from `snap`, **outside** the undo bracket:
    /// `Some(adj)` shows the correction, `None` puts the pixels back.
    ///
    /// Every pixel is rewritten from the snapshot each call, so this is
    /// idempotent and dragging a slider never compounds. Nothing else in the
    /// app may observe these pixels — see `App::adjust_preview_revert`.
    pub fn preview_adjust(
        &mut self,
        index: usize,
        snap: &[(TileIdx, Arc<Tile>)],
        adj: Option<&Adjust>,
    ) {
        if index >= self.layers.len() {
            return;
        }
        let sel = self.selection.clone();
        for (idx, orig) in snap {
            let mask = sel.as_ref().and_then(|s| s.tile_mask(*idx));
            let src = orig.data().to_vec();
            let data = self.layers[index].tile_mut(*idx).data_mut();
            match adj {
                Some(a) => correct_tile(data, &src, a, mask),
                None => data.copy_from_slice(&src),
            }
        }
        self.touch();
    }

    /// Apply a correction to the **active** layer as ONE undo step. False
    /// when the layer refuses, has no pixels in reach, or the correction is
    /// a no-op. The single-layer face of [`Self::apply_adjust_many`].
    pub fn apply_adjust(&mut self, adj: &Adjust) -> bool {
        let li = self.active;
        self.apply_adjust_many(adj, &[li]) > 0
    }

    /// Apply one correction to several layers as ONE undo step (TC-013:
    /// `UndoGroup::Compound` of per-layer `Tiles` groups — the CSP 5.0
    /// "correct the page, not a layer at a time" operation), clipped to the
    /// selection when there is one. Layers that refuse (folder / vector /
    /// locked / no pixels in reach) are skipped. Returns how many layers
    /// were corrected; zero pushes nothing.
    pub fn apply_adjust_many(&mut self, adj: &Adjust, indices: &[usize]) -> usize {
        if adj.is_identity() {
            return 0;
        }
        let sel = self.selection.clone();
        let mut members = Vec::new();
        for &li in indices {
            let snap = self.adjust_snapshot(li);
            if snap.is_empty() {
                continue;
            }
            // `begin_op_on`, not `begin_op`: only one of these layers is
            // active, and recording into "whichever layer happened to be
            // active" is the documented art-loss shape (CODE-MAP, undo).
            self.begin_op_on(li);
            for (idx, orig) in &snap {
                let mask = sel.as_ref().and_then(|s| s.tile_mask(*idx));
                let src = orig.data().to_vec();
                let data = self.layers[li].tile_mut(*idx).data_mut();
                correct_tile(data, &src, adj, mask);
            }
            // No `mask_op_to_selection`: the coverage blend is already in
            // `correct_tile` and unselected tiles were never in `snap`.
            // No `mask_op_to_alpha` either — see the module note.
            if let Some(g) = self.end_op_take() {
                members.push(g);
            }
        }
        let n = members.len();
        self.push_compound(adj.label(), members);
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection::Selection;
    use crate::tile::TILE_SIZE;

    /// Write one straight-colour pixel (premultiplied on the way in).
    fn put(doc: &mut Document, li: usize, x: i32, y: i32, rgba: [f32; 4]) {
        let idx = TileIdx::new(x / TILE_SIZE as i32, y / TILE_SIZE as i32);
        let (ox, oy) = idx.origin();
        let p = ((y - oy) as usize * TILE_SIZE + (x - ox) as usize) * 4;
        let a = rgba[3];
        let d = doc.layers[li].tile_mut(idx).data_mut();
        for c in 0..3 {
            d[p + c] = f32_to_fix15(rgba[c] * a);
        }
        d[p + 3] = f32_to_fix15(a);
    }

    /// Read a pixel back as straight colour + alpha.
    fn get(doc: &Document, li: usize, x: i32, y: i32) -> [f32; 4] {
        let idx = TileIdx::new(x / TILE_SIZE as i32, y / TILE_SIZE as i32);
        let (ox, oy) = idx.origin();
        let p = ((y - oy) as usize * TILE_SIZE + (x - ox) as usize) * 4;
        let Some(t) = doc.layers[li].tile_arc(idx) else {
            return [0.0; 4];
        };
        let d = t.data();
        let a = d[p + 3] as f32 / FIX15_ONE_F;
        if a <= 0.0 {
            return [0.0, 0.0, 0.0, 0.0];
        }
        [
            d[p] as f32 / FIX15_ONE_F / a,
            d[p + 1] as f32 / FIX15_ONE_F / a,
            d[p + 2] as f32 / FIX15_ONE_F / a,
            a,
        ]
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.002
    }

    // --- the pure maps ---------------------------------------------------

    #[test]
    fn invert_is_its_own_inverse() {
        let c = [0.2, 0.5, 0.9];
        let back = Adjust::Invert.map(Adjust::Invert.map(c));
        assert!((0..3).all(|i| near(back[i], c[i])), "{back:?}");
    }

    #[test]
    fn binarize_splits_at_the_threshold_on_luma() {
        // Rec.709: pure green is bright (0.715), pure blue is dark (0.072),
        // so at a 0.5 threshold green goes white and blue goes black even
        // though blue's own channel is at full.
        let b = Adjust::Binarize { threshold: 0.5 };
        assert_eq!(b.map([0.0, 1.0, 0.0]), [1.0; 3]);
        assert_eq!(b.map([0.0, 0.0, 1.0]), [0.0; 3]);
        // And the threshold is a real knob, not a fixed midpoint.
        let low = Adjust::Binarize { threshold: 0.05 };
        assert_eq!(low.map([0.0, 0.0, 1.0]), [1.0; 3]);
    }

    #[test]
    fn posterize_pins_both_ends_and_gives_n_levels() {
        let p = Adjust::Posterize { levels: 4 };
        assert_eq!(p.map([0.0; 3])[0], 0.0);
        assert_eq!(p.map([1.0; 3])[0], 1.0);
        let steps: std::collections::BTreeSet<u32> = (0..=100)
            .map(|i| (p.map([i as f32 / 100.0; 3])[0] * 1000.0).round() as u32)
            .collect();
        assert_eq!(steps.len(), 4, "four buckets, got {steps:?}");
    }

    #[test]
    fn brightness_and_contrast_rest_at_zero() {
        let c = [0.2, 0.5, 0.9];
        let out = Adjust::BRIGHTNESS_CONTRAST.map(c);
        assert!((0..3).all(|i| near(out[i], c[i])), "{out:?}");
        assert!(Adjust::BRIGHTNESS_CONTRAST.is_identity());
        // Contrast pivots on mid grey: 0.5 does not move, the ends spread.
        let hi = Adjust::BrightnessContrast {
            brightness: 0.0,
            contrast: 0.5,
        };
        assert!(near(hi.map([0.5; 3])[0], 0.5));
        assert!(hi.map([0.7; 3])[0] > 0.7);
        assert!(hi.map([0.3; 3])[0] < 0.3);
    }

    #[test]
    fn hue_saturation_rests_at_zero_and_desaturates_to_luma_free_grey() {
        let c = [0.2, 0.6, 0.4];
        let out = Adjust::HUE_SATURATION.map(c);
        assert!((0..3).all(|i| near(out[i], c[i])), "{out:?}");
        assert!(Adjust::HUE_SATURATION.is_identity());
        // −1 saturation flattens to grey; HSV grey is the max channel.
        let grey = Adjust::HueSaturation {
            hue: 0.0,
            saturation: -1.0,
            luminosity: 0.0,
        }
        .map(c);
        assert!(near(grey[0], grey[1]) && near(grey[1], grey[2]), "{grey:?}");
        assert!(near(grey[0], 0.6), "{grey:?}");
        // A 360° hue rotation is a round trip.
        let spun = Adjust::HueSaturation {
            hue: 360.0,
            saturation: 0.0,
            luminosity: 0.0,
        }
        .map(c);
        assert!((0..3).all(|i| near(spun[i], c[i])), "{spun:?}");
    }

    /// A tone curve from a point list, with the dead slots left at rest.
    fn curve(points: &[[f32; 2]]) -> Adjust {
        let mut pts = Adjust::TONE_CURVE_REST;
        pts[..points.len()].copy_from_slice(points);
        for p in pts.iter_mut().skip(points.len()) {
            *p = [0.0, 0.0];
        }
        Adjust::ToneCurve {
            pts,
            n: points.len() as u8,
        }
    }

    #[test]
    fn levels_rest_is_identity() {
        let c = [0.0, 0.37, 1.0];
        let out = Adjust::LEVELS.map(c);
        assert!((0..3).all(|i| near(out[i], c[i])), "{out:?}");
        assert!(Adjust::LEVELS.is_identity());
        // And any single knob off rest is NOT identity — an empty undo step
        // is the failure this guards.
        let mut moved = Adjust::LEVELS;
        if let Adjust::Levels { gamma, .. } = &mut moved {
            *gamma = 1.2;
        }
        assert!(!moved.is_identity());
    }

    /// Levels with everything at rest but the named knobs.
    fn levels(in_black: f32, in_white: f32, gamma: f32, out_black: f32, out_white: f32) -> Adjust {
        Adjust::Levels {
            in_black,
            in_white,
            gamma,
            out_black,
            out_white,
        }
    }

    #[test]
    fn levels_gamma_brightens_midtones_without_moving_the_ends() {
        let g = levels(0.0, 1.0, 2.0, 0.0, 1.0);
        assert!(near(g.map([0.0; 3])[0], 0.0), "black stays black");
        assert!(near(g.map([1.0; 3])[0], 1.0), "white stays white");
        let mid = g.map([0.5; 3])[0];
        assert!(mid > 0.5, "gamma 2 must brighten mid grey, got {mid}");
        assert!(near(mid, 0.5f32.sqrt()), "gamma is x^(1/g): {mid}");
        // The other direction darkens.
        assert!(levels(0.0, 1.0, 0.5, 0.0, 1.0).map([0.5; 3])[0] < 0.5);
    }

    #[test]
    fn levels_input_range_clips_and_output_range_remaps() {
        // Input: everything at or below 0.25 goes to black, at or above 0.75
        // to white — the scanner move.
        let clip = levels(0.25, 0.75, 1.0, 0.0, 1.0);
        assert_eq!(clip.map([0.1; 3])[0], 0.0);
        assert_eq!(clip.map([0.9; 3])[0], 1.0);
        assert!(near(clip.map([0.5; 3])[0], 0.5), "the middle stays middle");
        // Output: the whole image is squeezed into 0.2..0.8.
        let out = levels(0.0, 1.0, 1.0, 0.2, 0.8);
        assert!(near(out.map([0.0; 3])[0], 0.2));
        assert!(near(out.map([1.0; 3])[0], 0.8));
        assert!(near(out.map([0.5; 3])[0], 0.5));
    }

    #[test]
    fn tone_curve_identity_is_identity_and_pins_the_ends() {
        assert!(Adjust::TONE_CURVE.is_identity());
        for i in 0..=20 {
            let x = i as f32 / 20.0;
            let y = Adjust::TONE_CURVE.map([x; 3])[0];
            assert!(near(y, x), "identity curve moved {x} to {y}");
        }
        // A raised midpoint still pins both ends.
        let up = curve(&[[0.0, 0.0], [0.5, 0.75], [1.0, 1.0]]);
        assert!(near(up.map([0.0; 3])[0], 0.0), "black end");
        assert!(near(up.map([1.0; 3])[0], 1.0), "white end");
        assert!(near(up.map([0.5; 3])[0], 0.75), "the point itself");
        assert!(up.map([0.25; 3])[0] > 0.25, "and it lifts its neighbourhood");
        assert!(!up.is_identity());
        // Points that all sit ON the diagonal really are a no-op, extra
        // handles or not.
        assert!(curve(&[[0.0, 0.0], [0.4, 0.4], [1.0, 1.0]]).is_identity());
    }

    #[test]
    fn tone_curve_never_overshoots_monotone_points() {
        // The Catmull-Rom failure: a flat run followed by a steep rise rings
        // BELOW zero (a dark halo) before it climbs. Fritsch–Carlson must
        // stay inside the box its neighbours bound, and stay monotone.
        let c = curve(&[[0.0, 0.1], [0.3, 0.1], [0.35, 0.9], [1.0, 1.0]]);
        let mut prev = -1.0f32;
        for i in 0..=200 {
            let x = i as f32 / 200.0;
            let y = c.map([x; 3])[0];
            assert!(
                (0.1 - 0.0005..=1.0).contains(&y),
                "overshoot at {x}: {y} left the data's range"
            );
            assert!(y >= prev - 0.0005, "not monotone at {x}: {prev} -> {y}");
            prev = y;
        }
        // The flat run is actually flat, not a bulge.
        assert!(near(c.map([0.15; 3])[0], 0.1));
    }

    #[test]
    fn tone_curve_applies_per_channel() {
        // One curve, three channels, each read independently — a curve that
        // reduced to luma would flatten colour.
        let c = curve(&[[0.0, 0.0], [0.5, 0.75], [1.0, 1.0]]);
        let out = c.map([0.5, 0.0, 1.0]);
        assert!(near(out[0], 0.75) && near(out[1], 0.0) && near(out[2], 1.0), "{out:?}");
    }

    // --- the document applier --------------------------------------------

    fn doc_with_pixels() -> Document {
        let mut doc = Document::new(128, 128);
        // Two pixels, both mid grey, one inside a future selection and one
        // outside it (different tiles as well as different pixels).
        put(&mut doc, 0, 10, 10, [0.6, 0.6, 0.6, 1.0]);
        put(&mut doc, 0, 100, 100, [0.6, 0.6, 0.6, 1.0]);
        doc
    }

    #[test]
    fn correction_is_one_undo_step_and_undo_restores_exactly() {
        let mut doc = doc_with_pixels();
        let before = get(&doc, 0, 10, 10);
        let depth = doc.undo_labels().len();
        doc.set_op_label(Adjust::Invert.label());
        assert!(doc.apply_adjust(&Adjust::Invert));
        assert_eq!(doc.undo_labels().len(), depth + 1, "exactly one step");
        assert_eq!(doc.undo_labels().last().unwrap(), "Reverse gradient");
        let after = get(&doc, 0, 10, 10);
        assert!(near(after[0], 0.4), "inverted: {after:?}");
        assert!(doc.undo());
        let back = get(&doc, 0, 10, 10);
        assert!((0..4).all(|i| near(back[i], before[i])), "{back:?}");
    }

    #[test]
    fn a_selection_bounds_the_correction() {
        // The classic silent-damage bug: a correction that ignores the
        // selection quietly repaints the whole layer.
        let mut doc = doc_with_pixels();
        let outside_before = get(&doc, 0, 100, 100);
        doc.selection = Some(Selection::from_rect(&doc, 0.0, 0.0, 64.0, 64.0));
        assert!(doc.apply_adjust(&Adjust::Invert));
        let inside = get(&doc, 0, 10, 10);
        assert!(near(inside[0], 0.4), "inside the selection: {inside:?}");
        let outside = get(&doc, 0, 100, 100);
        assert!(
            (0..4).all(|i| outside[i] == outside_before[i]),
            "outside the selection must be byte-identical: {outside:?}"
        );
    }

    #[test]
    fn correction_preserves_alpha_and_uses_straight_colour() {
        // A half-transparent mid grey. Premultiplied storage holds 0.3;
        // a correction that forgot to unpremultiply would binarize on 0.3
        // and come out black. The right answer is white (0.6 ≥ 0.5), and
        // the alpha must be untouched.
        let mut doc = Document::new(128, 128);
        put(&mut doc, 0, 5, 5, [0.6, 0.6, 0.6, 0.5]);
        // The negative control, and it is the load-bearing half: a DARK
        // half-transparent pixel must come out black. White-on-white is what
        // an unpremultiply that OVERSHOOTS also produces, so the assertion
        // above cannot fail on its own — it passed against the shipped
        // `correct_tile` that scaled every colour past 1.0 and therefore
        // binarized pure white no matter what was under the pen.
        put(&mut doc, 0, 6, 5, [0.4, 0.4, 0.4, 0.5]);
        assert!(doc.apply_adjust(&Adjust::Binarize { threshold: 0.5 }));
        let p = get(&doc, 0, 5, 5);
        assert!(near(p[0], 1.0) && near(p[1], 1.0) && near(p[2], 1.0), "{p:?}");
        assert!(near(p[3], 0.5), "alpha must survive a correction: {p:?}");
        let d = get(&doc, 0, 6, 5);
        assert!(near(d[0], 0.0) && near(d[1], 0.0) && near(d[2], 0.0), "{d:?}");
        assert!(near(d[3], 0.5), "alpha must survive a correction: {d:?}");
    }

    #[test]
    fn transparent_pixels_stay_transparent() {
        let mut doc = Document::new(128, 128);
        put(&mut doc, 0, 5, 5, [0.0, 0.0, 0.0, 1.0]);
        assert!(doc.apply_adjust(&Adjust::Invert));
        // Its neighbour was never written and must not have been inverted
        // into opaque white.
        assert_eq!(get(&doc, 0, 6, 5), [0.0; 4]);
    }

    #[test]
    fn an_identity_correction_pushes_no_undo_step() {
        let mut doc = doc_with_pixels();
        let depth = doc.undo_labels().len();
        assert!(!doc.apply_adjust(&Adjust::BRIGHTNESS_CONTRAST));
        assert_eq!(doc.undo_labels().len(), depth, "no empty step");
    }

    #[test]
    fn a_locked_layer_refuses() {
        let mut doc = doc_with_pixels();
        doc.layers[0].lock = true;
        assert!(!doc.apply_adjust(&Adjust::Invert));
    }

    #[test]
    fn preview_is_idempotent_and_reverts_exactly() {
        // Dragging a slider re-previews many times; the pixels must come
        // from the snapshot every time, never compound, and `None` must put
        // the layer back bit-for-bit.
        let mut doc = doc_with_pixels();
        let before: Vec<u16> = doc.layers[0]
            .tile_arc(TileIdx::new(0, 0))
            .unwrap()
            .data()
            .to_vec();
        let snap = doc.adjust_snapshot(0);
        doc.preview_adjust(0, &snap, Some(&Adjust::Invert));
        let once = get(&doc, 0, 10, 10);
        doc.preview_adjust(0, &snap, Some(&Adjust::Invert));
        let twice = get(&doc, 0, 10, 10);
        assert_eq!(once, twice, "preview compounded");
        assert!(near(once[0], 0.4), "{once:?}");
        // And a preview writes NO undo step.
        assert!(doc.undo_labels().is_empty());
        doc.preview_adjust(0, &snap, None);
        let after: Vec<u16> = doc.layers[0]
            .tile_arc(TileIdx::new(0, 0))
            .unwrap()
            .data()
            .to_vec();
        assert_eq!(before, after, "revert must be bit-for-bit");
    }

    #[test]
    fn preview_and_commit_agree_pixel_for_pixel() {
        // The whole point of sharing `correct_tile`: what you saw is what
        // you got. Pin it, because a second code path is how previews start
        // lying.
        let adj = Adjust::Posterize { levels: 3 };
        let mut a = doc_with_pixels();
        a.selection = Some(Selection::from_rect(&a, 0.0, 0.0, 64.0, 64.0));
        let snap = a.adjust_snapshot(0);
        a.preview_adjust(0, &snap, Some(&adj));
        let previewed: Vec<u16> = a.layers[0]
            .tile_arc(TileIdx::new(0, 0))
            .unwrap()
            .data()
            .to_vec();

        let mut b = doc_with_pixels();
        b.selection = Some(Selection::from_rect(&b, 0.0, 0.0, 64.0, 64.0));
        assert!(b.apply_adjust(&adj));
        let committed: Vec<u16> = b.layers[0]
            .tile_arc(TileIdx::new(0, 0))
            .unwrap()
            .data()
            .to_vec();
        assert_eq!(previewed, committed);
    }

    // --- TC-013: several layers, one step --------------------------------

    #[test]
    fn many_corrects_selected_layers_as_one_compound_step() {
        let mut doc = Document::new(128, 128);
        put(&mut doc, 0, 10, 10, [0.6, 0.6, 0.6, 1.0]);
        let l2 = doc.add_layer("L2");
        put(&mut doc, l2, 10, 10, [0.2, 0.2, 0.2, 1.0]);
        let locked = doc.add_layer("locked");
        put(&mut doc, locked, 10, 10, [0.8, 0.8, 0.8, 1.0]);
        doc.layers[locked].lock = true;
        // A refusing layer (locked) and a bare index are skipped, not fatal.
        let n = doc.apply_adjust_many(&Adjust::Invert, &[0, l2, locked, 99]);
        assert_eq!(n, 2, "two layers corrected, locked + bogus skipped");
        assert!(near(get(&doc, 0, 10, 10)[0], 0.4));
        assert!(near(get(&doc, l2, 10, 10)[0], 0.8));
        assert!(near(get(&doc, locked, 10, 10)[0], 0.8), "locked untouched");
        assert_eq!(doc.undo_labels().len(), 1, "ONE step for the whole set");
        assert!(doc.undo());
        assert!(near(get(&doc, 0, 10, 10)[0], 0.6), "undo restores layer 0");
        assert!(near(get(&doc, l2, 10, 10)[0], 0.2), "and layer 2");
        assert!(doc.redo());
        assert!(near(get(&doc, 0, 10, 10)[0], 0.4), "redo replays layer 0");
        assert!(near(get(&doc, l2, 10, 10)[0], 0.8), "and layer 2");
    }

    #[test]
    fn multi_selection_gestures_and_the_structural_door() {
        let mut doc = Document::new(64, 64);
        let l1 = doc.add_layer("b");
        let l2 = doc.add_layer("c");
        assert_eq!(doc.active, l2);
        // Ctrl+click toggles and hands over the pen; Shift+click ranges.
        assert!(doc.toggle_multi(0));
        assert_eq!((doc.active, doc.layer_multi.clone()), (0, vec![l2]));
        assert!(doc.range_multi(l2));
        assert_eq!(doc.layer_multi, vec![l1, l2], "range spans, pen stays");
        assert_eq!(doc.multi_targets(), vec![0, l1, l2]);
        // Toggling the ACTIVE row off moves the pen to a remaining row.
        assert!(doc.toggle_multi(0));
        assert!(doc.active != 0 && !doc.layer_multi.contains(&doc.active));
        // A plain selection collapses the set.
        assert!(doc.set_active(l1));
        assert!(doc.layer_multi.is_empty());
        assert_eq!(doc.multi_targets(), vec![l1]);
        // The structural door: index-shifting ops clear the selection with
        // the history (the invariant Compound leans on covers both).
        doc.toggle_multi(l2);
        assert!(!doc.layer_multi.is_empty());
        doc.add_layer("d");
        assert!(doc.layer_multi.is_empty(), "structural op cleared it");
    }
}
