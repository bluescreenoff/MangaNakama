//! Row 55 (CSP 液化 Liquify, TR-spec in the crawl notes): direct
//! canvas warping with the pen — not a filter, a TOOL that edits the
//! raster layer as you drag.
//!
//! Modes (CSP's seven): **Push** (drag displaces along the stroke),
//! **Expand** / **Pinch** (radial out/in; press-and-hold accumulates),
//! **Push left** / **Push right** (displace perpendicular to the
//! stroke), **Twirl clockwise** / **anti-clockwise** (rotate about the
//! cursor; hold accumulates). **Alt inverts** the effect (Expand↔Pinch,
//! Push reverses, twirl reverses).
//!
//! Rendering: one STEP applies a displacement field over the brush
//! disc by INVERSE mapping — each destination pixel samples the
//! pre-step snapshot at `p − D(p)` with bilinear interpolation
//! (premultiplied fix15, so soft edges stay correct). Steps are small
//! by design (one per pointer move, or one per held frame for the
//! accumulating modes), which is what makes the inverse map exact
//! enough: the error is second-order in the step size.
//!
//! Callers bracket a whole gesture in `begin_op`/`end_op` — every step
//! writes through `tile_mut`, so the entire drag is ONE undo.

use crate::doc::Document;
use crate::tile::{Tile, TileIdx, TILE_SIZE};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiquifyMode {
    Push,
    Expand,
    Pinch,
    PushLeft,
    PushRight,
    TwirlCw,
    TwirlCcw,
}

impl LiquifyMode {
    pub const ALL: [LiquifyMode; 7] = [
        LiquifyMode::Push,
        LiquifyMode::Expand,
        LiquifyMode::Pinch,
        LiquifyMode::PushLeft,
        LiquifyMode::PushRight,
        LiquifyMode::TwirlCw,
        LiquifyMode::TwirlCcw,
    ];
    pub fn label(&self) -> &'static str {
        match self {
            LiquifyMode::Push => "Push",
            LiquifyMode::Expand => "Expand",
            LiquifyMode::Pinch => "Pinch",
            LiquifyMode::PushLeft => "Push left",
            LiquifyMode::PushRight => "Push right",
            LiquifyMode::TwirlCw => "Twirl clockwise",
            LiquifyMode::TwirlCcw => "Twirl anti-clockwise",
        }
    }
    /// The modes where holding the pen still keeps working (CSP):
    /// Expand/Pinch bulge/shrink around the point, Twirl keeps turning.
    pub fn accumulates(self) -> bool {
        matches!(
            self,
            LiquifyMode::Expand | LiquifyMode::Pinch | LiquifyMode::TwirlCw | LiquifyMode::TwirlCcw
        )
    }
}

/// One warp step at `(x, y)` on `doc`'s layer `li`.
///
/// * `dx, dy` — the pointer delta since the last step (canvas px). The
///   push/perpendicular modes scale with it; the accumulating modes
///   take their energy from `amount` instead (the caller frames that
///   as `strength × elapsed`, so a held pen keeps working at the same
///   rate regardless of frame cadence).
/// * `strength` — 0..=1.
/// * `invert` — Alt.
pub fn step(
    doc: &mut Document,
    li: usize,
    mode: LiquifyMode,
    x: f32,
    y: f32,
    dx: f32,
    dy: f32,
    radius: f32,
    strength: f32,
    amount: f32,
    invert: bool,
) {
    let radius = radius.max(1.0);
    let s = strength.clamp(0.0, 1.0);
    if s <= 0.0 {
        return;
    }
    let (w, h) = (doc.size.0 as i32, doc.size.1 as i32);
    // How far any pixel can move this step: push-family scales with the
    // delta, radial/twirl with radius×amount. Destinations OUTSIDE the
    // brush rim can still receive ink (a push carries the disc's centre
    // a full delta), so the iteration region — and the snapshot it
    // reads from — is the disc EXPANDED by that reach.
    let push_len = dx.hypot(dy);
    let max_disp = match mode {
        LiquifyMode::Push | LiquifyMode::PushLeft | LiquifyMode::PushRight => push_len * s,
        _ => radius * amount * s,
    };
    let reach = radius as i32 + max_disp.ceil() as i32 + 1;
    let (cx, cy) = (x as i32, y as i32);
    let (x0, y0) = ((cx - reach).max(0), (cy - reach).max(0));
    let (x1, y1) = ((cx + reach).min(w - 1), (cy + reach).min(h - 1));
    if x1 < x0 || y1 < y0 {
        return;
    }
    // The pre-step snapshot, sampled OUTSIDE the disc too (displacement
    // can read up to |D| past the rim).
    let sw = (x1 - x0 + 1) as usize;
    let sh = (y1 - y0 + 1) as usize;
    let mut snap = vec![[0u16; 4]; sw * sh];
    for ty in 0..=y1 / TILE_SIZE as i32 {
        for tx in 0..=x1 / TILE_SIZE as i32 {
            let idx = TileIdx::new(tx, ty);
            let Some(t) = doc.layers.get(li).and_then(|l| l.tile_arc(idx)) else {
                continue;
            };
            let (ox, oy) = idx.origin();
            copy_tile_into(&t, &mut snap, ox - x0, oy - y0, sw, sh);
        }
    }
    let sample = |fx: f32, fy: f32| -> [f32; 4] {
        bilinear(&snap, fx - x0 as f32, fy - y0 as f32, sw, sh)
    };
    for py in y0..=y1 {
        for px in x0..=x1 {
            let (ux, uy) = (px as f32 - x, py as f32 - y);
            let d = ux.hypot(uy);
            if d >= radius {
                // Outside the brush proper the falloff is zero — but
                // pixels up to `reach` can still be DESTINATIONS, which
                // the displacement below handles by falling to ~0 there.
                continue;
            }
            // Smoothstep falloff: full at the centre, zero at the rim.
            let t = 1.0 - d / radius;
            let f = t * t * (3.0 - 2.0 * t) * s;
            let d_field = match mode {
                LiquifyMode::Push => [dx * f, dy * f],
                LiquifyMode::Expand | LiquifyMode::Pinch => {
                    let sign = match mode {
                        LiquifyMode::Pinch => -1.0,
                        _ => 1.0,
                    };
                    let inv_d = if d < 1e-3 { 0.0 } else { 1.0 / d };
                    [
                        ux * inv_d * f * radius * amount * sign,
                        uy * inv_d * f * radius * amount * sign,
                    ]
                }
                LiquifyMode::PushLeft | LiquifyMode::PushRight => {
                    if push_len < 1e-3 {
                        [0.0, 0.0]
                    } else {
                        let sign = if mode == LiquifyMode::PushRight {
                            -1.0
                        } else {
                            1.0
                        };
                        let n = push_len * f;
                        [
                            dy / push_len * n * sign,
                            -dx / push_len * n * sign,
                        ]
                    }
                }
                LiquifyMode::TwirlCw | LiquifyMode::TwirlCcw => {
                    let sign = if mode == LiquifyMode::TwirlCcw {
                        -1.0
                    } else {
                        1.0
                    };
                    let inv_d = if d < 1e-3 { 0.0 } else { 1.0 / d };
                    let n = f * radius * amount * sign;
                    [-uy * inv_d * n, ux * inv_d * n]
                }
            };
            let d_field = if invert {
                [-d_field[0], -d_field[1]]
            } else {
                d_field
            };
            let src = sample(px as f32 - d_field[0], py as f32 - d_field[1]);
            let idx = TileIdx::of_pixel(px, py);
            let (ox, oy) = idx.origin();
            let t = doc.layers[li].tile_mut(idx);
            let px16 = [
                src[0].round().clamp(0.0, u16::MAX as f32) as u16,
                src[1].round().clamp(0.0, u16::MAX as f32) as u16,
                src[2].round().clamp(0.0, u16::MAX as f32) as u16,
                src[3].round().clamp(0.0, u16::MAX as f32) as u16,
            ];
            t.set_pixel((px - ox) as usize, (py - oy) as usize, px16);
        }
    }
}

/// Bilinear sample of the snapshot; transparent outside it.
fn bilinear(snap: &[[u16; 4]], fx: f32, fy: f32, w: usize, h: usize) -> [f32; 4] {
    let (w, h) = (w as f32, h as f32);
    if fx < 0.0 || fy < 0.0 || fx > w - 1.0 || fy > h - 1.0 {
        return [0.0; 4];
    }
    let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w as usize - 1), (y0 + 1).min(h as usize - 1));
    let (ax, ay) = (fx - x0 as f32, fy - y0 as f32);
    let mut out = [0.0f32; 4];
    for c in 0..4 {
        let top = snap[y0 * w as usize + x0][c] as f32 * (1.0 - ax)
            + snap[y0 * w as usize + x1][c] as f32 * ax;
        let bot = snap[y1 * w as usize + x0][c] as f32 * (1.0 - ax)
            + snap[y1 * w as usize + x1][c] as f32 * ax;
        out[c] = top * (1.0 - ay) + bot * ay;
    }
    out
}

fn copy_tile_into(
    t: &Tile,
    snap: &mut [[u16; 4]],
    off_x: i32,
    off_y: i32,
    sw: usize,
    sh: usize,
) {
    let d = t.data();
    for py in 0..TILE_SIZE {
        let sy = off_y + py as i32;
        if sy < 0 || sy >= sh as i32 {
            continue;
        }
        for px in 0..TILE_SIZE {
            let sx = off_x + px as i32;
            if sx < 0 || sx >= sw as i32 {
                continue;
            }
            let o = (py * TILE_SIZE + px) * 4;
            snap[sy as usize * sw + sx as usize] =
                [d[o], d[o + 1], d[o + 2], d[o + 3]];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::f32_to_fix15;
    use crate::doc::Document;

    /// Opaque black square, premultiplied.
    fn ink(doc: &mut Document, li: usize, x0: i32, y0: i32, x1: i32, y1: i32) {
        for y in y0..y1 {
            for x in x0..x1 {
                let idx = TileIdx::of_pixel(x, y);
                let (ox, oy) = idx.origin();
                let t = doc.layers[li].tile_mut(idx);
                let o = ((y - oy) as usize * TILE_SIZE + (x - ox) as usize) * 4;
                let f = f32_to_fix15(0.0);
                let d = t.data_mut();
                d[o] = f;
                d[o + 1] = f;
                d[o + 2] = f;
                d[o + 3] = f32_to_fix15(1.0);
            }
        }
    }

    fn alpha(doc: &Document, li: usize, x: i32, y: i32) -> u16 {
        let idx = TileIdx::of_pixel(x, y);
        let (ox, oy) = idx.origin();
        doc.layers[li]
            .tile_arc(idx)
            .map(|t| t.data()[((y - oy) as usize * TILE_SIZE + (x - ox) as usize) * 4 + 3])
            .unwrap_or(0)
    }

    #[test]
    fn push_moves_ink_along_the_delta() {
        let mut doc = Document::new(128, 128);
        let li = doc.add_layer("l");
        ink(&mut doc, li, 30, 60, 46, 68); // a horizontal bar
        // Ten small steps, the way a real drag arrives — a 20 px jump
        // in one step is not a stroke.
        for _ in 0..10 {
            step(&mut doc, li, LiquifyMode::Push, 38.0, 64.0, 2.0, 0.0, 16.0, 1.0, 0.0, false);
        }
        assert!(alpha(&doc, li, 48, 64) > 8000, "ink arrived past the old edge");
        assert!(alpha(&doc, li, 30, 64) < 4000, "the old left edge drained");
    }

    #[test]
    fn alt_inverts_push() {
        let mut doc = Document::new(128, 128);
        let li = doc.add_layer("l");
        ink(&mut doc, li, 30, 60, 46, 68);
        for _ in 0..10 {
            step(&mut doc, li, LiquifyMode::Push, 38.0, 64.0, 2.0, 0.0, 16.0, 1.0, 0.0, true);
        }
        assert!(alpha(&doc, li, 28, 64) > 8000, "inverted push moved LEFT");
        assert!(alpha(&doc, li, 44, 64) < 4000, "and drained the right");
    }

    #[test]
    fn expand_bulges_and_pinch_pulls_back() {
        let mut doc = Document::new(128, 128);
        let li = doc.add_layer("l");
        for (dx, dy) in [(10, 0), (-10, 0), (0, 10), (0, -10)] {
            ink(&mut doc, li, 64 + dx, 64 + dy, 66 + dx, 66 + dy);
        }
        ink(&mut doc, li, 63, 63, 65, 65);
        let (cx, cy) = (64.0f32, 64.0f32);
        // Hold-style accumulation: small amounts, many steps.
        for _ in 0..10 {
            step(&mut doc, li, LiquifyMode::Expand, cx, cy, 0.0, 0.0, 24.0, 1.0, 0.15, false);
        }
        assert!(alpha(&doc, li, 78, 64) > 4000, "the ring dot drifted outward");
        // Accumulated expand leaves a TRAIL: the centre dot's ink flows
        // outward through the near-centre ring on its way to the rim.
        assert!(alpha(&doc, li, 70, 64) > 1000, "outward trail between centre and ring");
        let after_expand = alpha(&doc, li, 78, 64);
        for _ in 0..20 {
            step(&mut doc, li, LiquifyMode::Pinch, cx, cy, 0.0, 0.0, 24.0, 1.0, 0.15, false);
        }
        assert!(
            alpha(&doc, li, 78, 64) < after_expand,
            "pinch pulled the bulge back in"
        );
    }

    #[test]
    fn twirl_rotates_ink_clockwise_about_the_centre() {
        let mut doc = Document::new(128, 128);
        let li = doc.add_layer("l");
        ink(&mut doc, li, 84, 63, 86, 65); // one dot east of centre
        let (cx, cy) = (64.0f32, 64.0f32);
        for _ in 0..14 {
            step(&mut doc, li, LiquifyMode::TwirlCw, cx, cy, 0.0, 0.0, 30.0, 1.0, 0.2, false);
        }
        assert!(alpha(&doc, li, 84, 64) < 4000, "the dot left the east");
        // ~63° of clockwise turn on a y-down canvas carries east toward
        // SOUTH: the ink lands below-right of the centre…
        assert!(alpha(&doc, li, 72, 81) > 3000, "…clockwise, toward the south");
        assert_eq!(alpha(&doc, li, 72, 47), 0, "and not toward the north");
    }

    #[test]
    fn a_whole_gesture_is_one_undo() {
        let mut doc = Document::new(128, 128);
        let li = doc.add_layer("l");
        ink(&mut doc, li, 30, 60, 46, 68);
        doc.begin_op();
        for k in 0..5 {
            step(&mut doc, li, LiquifyMode::Push, 38.0 + k as f32, 64.0, 4.0, 0.0, 16.0, 1.0, 0.0, false);
        }
        doc.end_op();
        assert!(doc.undo());
        assert_eq!(alpha(&doc, li, 30, 64), f32_to_fix15(1.0), "undo restored the ink");
        assert_eq!(alpha(&doc, li, 52, 64), 0, "…all of it");
    }
}
