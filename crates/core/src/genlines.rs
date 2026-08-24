//! Speed/focus line GENERATION (TRIAGE 140 v1, SF-family): parametric,
//! seeded, deterministic black ink for manga effect lines.
//!
//! v1 is dialog-driven — parameters in, one new layer of hard-edged ink
//! out. CSP's two-driver-curve on-canvas editing (SF-004/005: the blue
//! reference line and the red shape line, editable alone) needs
//! Object-tool curve editing on generator layers and is deferred with
//! reason; the params here are exactly what those two curves will drive.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::tile::{FIX15_ONE, TILE_SIZE, Tile, TileIdx};

/// 集中線 — focus lines: `count` rays converging toward `center`, drawn
/// from `r_in` to `r_out` (jittered per line), each a segment from a
/// jittered angle. Width jitters by `width_jitter` (0..1 of `width`).
#[derive(Clone, Debug)]
pub struct FocusLinesParams {
    pub center: [f32; 2],
    pub r_in: f32,
    pub r_out: f32,
    pub count: u32,
    pub width: f32,
    /// 0..1 — per-line angle jitter as a fraction of the angular gap.
    pub angle_jitter: f32,
    /// 0..1 — per-line width jitter fraction.
    pub width_jitter: f32,
    /// 0..1 — per-line length jitter fraction of (r_out − r_in).
    pub length_jitter: f32,
    /// 0..1 — rays thin toward the CENTRE (a printed 集中線 needles at the
    /// convergence and carries its weight at the rim). 0 = the legacy
    /// constant-width ray, bit-stable.
    pub taper: f32,
    pub seed: u64,
}

/// 流線 — speed lines: `count` parallel segments along `angle` degrees,
/// lengths in [len_min, len_max], scattered across the canvas perpendic.
///
/// `Default` exists for the `..Default::default()` shorthand and zeroes
/// every knob, which for the density fields is exactly the legacy meaning
/// (uniform-random scatter, no bundling, no split jitters).
#[derive(Clone, Debug, Default)]
pub struct SpeedLinesParams {
    pub angle_deg: f32,
    pub count: u32,
    pub len_min: f32,
    pub len_max: f32,
    pub width: f32,
    /// 0..1 — how far each run thins toward its TAIL (the end it travels
    /// to). 0 is the pre-2026-08-22 look, bit for bit; 1 ends in a needle
    /// point, which is what a printed 流線 block actually does (the
    /// pro-page audit's "flat noise field" complaint).
    pub taper: f32,
    /// Aim every run at this canvas point instead of running pure
    /// parallel — a far point gives the subtle fan a perspective panel
    /// wants; `None` is parallel. A NEAR point turns the block into
    /// focus lines, which is the other tool's job.
    pub converge: Option<[f32; 2]>,
    /// >0 — WALK the normal extent in steps of `gap_px` instead of
    /// scattering `count` runs at uniform-random offsets. The scatter is
    /// what makes a generated 流線 block read as noise: uniform-random
    /// positions CLUMP (three runs a pixel apart, then a bald strip),
    /// which is precisely what a hand-ruled block never does. 0 keeps the
    /// scatter, bit for bit, for every file saved before this existed.
    pub gap_px: f32,
    /// まとまり — bundle `group` runs at `gap_px`, then leave a hole of
    /// `group_gap` × `gap_px` before the next bundle. 0/1 = no bundling.
    /// Only read on the walk (`gap_px` > 0).
    pub group: u32,
    /// The hole between bundles, in multiples of `gap_px` (see `group`).
    pub group_gap: f32,
    /// 0..1 — positional wobble as a fraction of `gap_px`, so the walk is
    /// even without being mechanical. Walk only; 0 = dead even.
    pub jit_gap: f32,
    /// 0..1 — per-run length wobble, a fraction pulled off the drawn
    /// length. Walk only; 0 = the `len_min`..`len_max` spread alone.
    pub jit_len: f32,
    /// 0..1 — per-run width wobble, a fraction pulled off `width`. Walk
    /// only; 0 = every run at the nominal width.
    pub jit_width: f32,
    pub seed: u64,
}

/// The walk's hard ceiling on runs. `gap_px` comes from a UI field and a
/// sub-pixel gap over a 600 dpi B4's ~10 000 px normal extent is an
/// unbounded rasterization, not a slow one (the same class of hang the
/// [`segment`] bbox clip fixed).
const MAX_RUNS: u32 = 20_000;

/// ウニフラッシュ — sea-urchin flash: `count` FILLED triangular spikes
/// around `center`, needle-pointed at `r_in` and `width` px wide at
/// `r_out`. That is the classic flash mat: the shape a segment-based
/// generator cannot make, and the pro-page audit's #1 IMPOSSIBLE.
///
/// `solid` flips the POLARITY to the ベタフラッシュ variant — the ring
/// area between `r_in` and `r_out` inks solid and the same teeth are cut
/// OUT of it, so the ink pools at the hole and breaks into outward
/// spikes at the rim. The hole stays empty either way: it is where the
/// art goes.
#[derive(Clone, Debug)]
pub struct UrchinParams {
    pub center: [f32; 2],
    pub r_in: f32,
    pub r_out: f32,
    pub count: u32,
    /// Spike base width in px at `r_out`, CLAMPED to 90% of the gap
    /// between neighbours: a wider value merges the teeth into a plain
    /// ring (and, inverted, erases the solid variant entirely), so the
    /// tool would silently stop drawing the shape it exists for.
    pub width: f32,
    /// 0..1 — per-spike angle jitter as a fraction of the angular gap.
    /// Clamped to half a gap by the renderer, and that clamp is load-
    /// bearing: the solid variant finds a pixel's teeth by SECTOR index,
    /// and a tooth that wandered a whole sector over would be missed —
    /// a black tooth straddling a white gap.
    pub angle_jitter: f32,
    /// 0..1 — per-spike length jitter; each tip pulls in from `r_out`.
    pub length_jitter: f32,
    pub solid: bool,
    pub seed: u64,
}

/// One flash tooth as a filled isoceles triangle: apex on the ray at
/// `r_apex`, `hw` half-width at `r_base`. The sides are STRAIGHT — the
/// test is the triangle's, not an angular wedge's, because a constant-
/// angle wedge bows outward and reads as a petal instead of a spike.
struct Tooth {
    c: f32,
    s: f32,
    r_apex: f32,
    r_base: f32,
    hw: f32,
}

impl Tooth {
    /// Is `(dx, dy)` — a pixel relative to the flash centre — inside?
    fn hit(&self, dx: f32, dy: f32) -> bool {
        let along = dx * self.c + dy * self.s;
        if along < self.r_apex || along > self.r_base {
            return false;
        }
        let span = self.r_base - self.r_apex;
        if span <= f32::EPSILON {
            return false;
        }
        let perp = -dx * self.s + dy * self.c;
        perp.abs() <= self.hw * ((along - self.r_apex) / span)
    }
}

/// xorshift64* — small, deterministic, no deps.
fn rand(seed: &mut u64) -> f32 {
    // splitmix64 — full-range, no low-bit correlation (the first xorshift
    // attempt biased the top 24 bits to [0.5, 1) for small seeds).
    *seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *seed;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 40) as f32 / (1u64 << 24) as f32
}

/// Ink one pixel opaque BLACK, premultiplied fix15 — `[0, 0, 0, ONE]`.
///
/// This wrote `[ONE; 4]` from the first commit, which is opaque WHITE (the
/// same four words `Layer::fill_white` writes), so every generator in this
/// module drew white-on-white: a layer in the palette and nothing on the
/// page. It survived because every test here reads channel 3 only —
/// coverage — and the fingerprint pin counts inked PIXELS, not their
/// colour, so the whole suite agreed with a bug it never looked at. Owner
/// repro 2026-08-22, Figure ▸ Saturated line. Black is the documented
/// contract of this module (three doc comments say so) and the print
/// reality of 集中線/流線; the geometry is untouched, so the bit-stability
/// pin still matches.
fn put(map: &mut HashMap<TileIdx, Tile>, x: i32, y: i32) {
    if x < 0 || y < 0 {
        return;
    }
    let idx = TileIdx::of_pixel(x, y);
    let (ox, oy) = idx.origin();
    let tile = map.entry(idx).or_insert_with(Tile::new_transparent);
    let lx = (x - ox) as usize;
    let ly = (y - oy) as usize;
    if lx < TILE_SIZE && ly < TILE_SIZE {
        let o = Tile::offset(lx, ly);
        let d = tile.data_mut();
        d[o] = 0;
        d[o + 1] = 0;
        d[o + 2] = 0;
        d[o + 3] = FIX15_ONE as u16;
    }
}

/// Rasterize one thick segment (a x b, half-width hw) by scanning its
/// bbox and testing point-to-segment distance. Hard edges — speed lines
/// are print black; AA lives in the resample on export.
///
/// `taper` (0..1) ramps the half-width down along a→b, so `b` is the
/// needle end; 0 leaves the constant-width behaviour untouched (the
/// ramp evaluates to `hw * 1.0`, the same float, so every effect-line
/// layer saved before tapering existed regenerates bit for bit).
///
/// The bbox is CLIPPED to the canvas here, not after: the dialog's own
/// maximums (count 512, outer radius 2×width) put a segment's unclipped
/// bbox at ~10^7 pixels on a 600 dpi page — unclipped, the scan was
/// quadratic in the radius and allocated unbounded off-canvas tiles that
/// `retain` only discarded after building (a multi-minute UI hang and a
/// commit spike, from three slider drags).
fn segment(
    map: &mut HashMap<TileIdx, Tile>,
    a: [f32; 2],
    b: [f32; 2],
    hw: f32,
    taper: f32,
    size: (u32, u32),
) {
    let d = [b[0] - a[0], b[1] - a[1]];
    let dd = d[0] * d[0] + d[1] * d[1];
    if dd <= f32::EPSILON {
        return;
    }
    let x0 = (a[0].min(b[0]) - hw - 1.0).max(0.0);
    let x1 = (a[0].max(b[0]) + hw + 1.0).min(size.0 as f32);
    let y0 = (a[1].min(b[1]) - hw - 1.0).max(0.0);
    let y1 = (a[1].max(b[1]) + hw + 1.0).min(size.1 as f32);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    for y in y0.floor() as i32..=y1.ceil() as i32 {
        for x in x0.floor() as i32..=x1.ceil() as i32 {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let t = (((px - a[0]) * d[0] + (py - a[1]) * d[1]) / dd).clamp(0.0, 1.0);
            let qx = a[0] + t * d[0];
            let qy = a[1] + t * d[1];
            let ex = px - qx;
            let ey = py - qy;
            let hwt = hw * (1.0 - taper * t);
            if ex * ex + ey * ey <= hwt * hwt {
                put(map, x, y);
            }
        }
    }
}

/// Fill one tooth by scanning ITS bbox — the same clip-first rule as
/// [`segment`], and for the same reason: an off-canvas flash centre with
/// a page-sized outer radius is an unbounded scan and an unbounded tile
/// allocation, not a slow one.
fn fill_tooth(map: &mut HashMap<TileIdx, Tile>, c: [f32; 2], t: &Tooth, size: (u32, u32)) {
    let apex = [c[0] + t.c * t.r_apex, c[1] + t.s * t.r_apex];
    let base = [c[0] + t.c * t.r_base, c[1] + t.s * t.r_base];
    let off = [-t.s * t.hw, t.c * t.hw];
    let xs = [apex[0], base[0] + off[0], base[0] - off[0]];
    let ys = [apex[1], base[1] + off[1], base[1] - off[1]];
    let x0 = (xs.iter().copied().fold(f32::INFINITY, f32::min) - 1.0).max(0.0);
    let x1 = (xs.iter().copied().fold(f32::NEG_INFINITY, f32::max) + 1.0).min(size.0 as f32);
    let y0 = (ys.iter().copied().fold(f32::INFINITY, f32::min) - 1.0).max(0.0);
    let y1 = (ys.iter().copied().fold(f32::NEG_INFINITY, f32::max) + 1.0).min(size.1 as f32);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    for y in y0.floor() as i32..=y1.ceil() as i32 {
        for x in x0.floor() as i32..=x1.ceil() as i32 {
            if t.hit(x as f32 + 0.5 - c[0], y as f32 + 0.5 - c[1]) {
                put(map, x, y);
            }
        }
    }
}

/// Render focus lines into sparse tiles (opaque black premul fix15).
pub fn render_focus(p: &FocusLinesParams, size: (u32, u32)) -> HashMap<TileIdx, Arc<Tile>> {
    let mut map: HashMap<TileIdx, Tile> = HashMap::new();
    let mut seed = p.seed | 1;
    let span = (p.r_out - p.r_in).max(1.0);
    for i in 0..p.count.max(1) {
        let base = i as f32 * std::f32::consts::TAU / p.count.max(1) as f32;
        let ang = base
            + (rand(&mut seed) - 0.5)
                * p.angle_jitter
                * (std::f32::consts::TAU / p.count.max(1) as f32);
        let r1 = p.r_in + rand(&mut seed) * p.length_jitter * span * 0.5;
        let r2 = p.r_out - rand(&mut seed) * p.length_jitter * span * 0.5;
        let w = p.width * (1.0 - rand(&mut seed) * p.width_jitter);
        let (s, c) = ang.sin_cos();
        // OUTER first: segment() tapers toward `b`, and a focus ray thins
        // toward the convergence (the inner end). With taper 0 the order
        // is invisible — the distance test is symmetric — so legacy
        // renders stay bit-stable (pinned test).
        segment(
            &mut map,
            [p.center[0] + c * r2, p.center[1] + s * r2],
            [p.center[0] + c * r1, p.center[1] + s * r1],
            (w * 0.5).max(0.5),
            p.taper.clamp(0.0, 1.0),
            size,
        );
    }
    // Clip: drop fully-off-canvas tiles; per-pixel clipping happened in put.
    let (w, h) = (size.0 as i32, size.1 as i32);
    map.retain(|idx, _| {
        let (ox, oy) = idx.origin();
        ox < w && oy < h && ox + TILE_SIZE as i32 > 0 && oy + TILE_SIZE as i32 > 0
    });
    map.into_iter().map(|(k, v)| (k, Arc::new(v))).collect()
}

/// Render speed lines: parallel runs scattered across the canvas along
/// `angle_deg`, each starting within the canvas's perpendicular extent.
pub fn render_speed(p: &SpeedLinesParams, size: (u32, u32)) -> HashMap<TileIdx, Arc<Tile>> {
    let mut map: HashMap<TileIdx, Tile> = HashMap::new();
    let mut seed = p.seed | 1;
    let (w, h) = (size.0 as f32, size.1 as f32);
    let rad = p.angle_deg.to_radians();
    let dir = [rad.cos(), rad.sin()];
    let nrm = [-rad.sin(), rad.cos()];
    // The canvas extent along the normal — lines scatter across it.
    let corners = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]];
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for c in corners {
        let t = c[0] * nrm[0] + c[1] * nrm[1];
        lo = lo.min(t);
        hi = hi.max(t);
    }
    // Where the runs sit along the normal. The WALK (gap_px > 0) steps a
    // fixed gap with optional bundling; the legacy path scatters `count`
    // of them at uniform-random offsets and is kept bit for bit, so no
    // saved layer redraws (`legacy_renders_are_bit_stable`). Every new
    // `rand` call lives behind a `> 0.0` guard for the same reason: the
    // draw sequence has to be untouched when the new knobs are absent.
    let mut offsets: Vec<f32> = Vec::new();
    if p.gap_px > 0.0 {
        let gap = p.gap_px.max(0.25);
        let ggap = if p.group > 1 {
            p.group_gap.max(1.0)
        } else {
            1.0
        };
        let mut t = lo;
        let mut i = 0u32;
        while t <= hi && (offsets.len() as u32) < MAX_RUNS {
            offsets.push(t);
            let bundle_end = p.group > 1 && (i + 1) % p.group == 0;
            t += if bundle_end { gap * ggap } else { gap };
            i += 1;
        }
    }
    let n = if offsets.is_empty() {
        p.count.max(1)
    } else {
        offsets.len() as u32
    };
    for i in 0..n {
        let mut t = match offsets.get(i as usize) {
            Some(t) => *t,
            None => lo + rand(&mut seed) * (hi - lo),
        };
        if p.jit_gap > 0.0 {
            t += (rand(&mut seed) - 0.5) * p.jit_gap.clamp(0.0, 1.0) * p.gap_px.max(0.25);
        }
        let mut len = p.len_min + rand(&mut seed) * (p.len_max - p.len_min).max(0.0);
        if p.jit_len > 0.0 {
            len *= 1.0 - rand(&mut seed) * p.jit_len.clamp(0.0, 0.9);
        }
        // Start offset along the direction so the run crosses the canvas.
        // `t` is already the ABSOLUTE normal coordinate (corner
        // projection) — no canvas-centre offset.
        let along = rand(&mut seed) * (w.max(h) + len) - len;
        let base = [
            nrm[0] * t + dir[0] * (along - len * 0.5),
            nrm[1] * t + dir[1] * (along - len * 0.5),
        ];
        // Convergence aims the run at a far point instead of along the
        // shared direction; the SCATTER stays the parallel layout's, so
        // the fan is a lean on the block rather than a second tool.
        let run = match p.converge {
            Some(v) => {
                let (vx, vy) = (v[0] - base[0], v[1] - base[1]);
                let l = vx.hypot(vy);
                if l > 1e-3 { [vx / l, vy / l] } else { dir }
            }
            None => dir,
        };
        let tip = [base[0] + run[0] * len, base[1] + run[1] * len];
        let mut hw = p.width * 0.5;
        if p.jit_width > 0.0 {
            hw *= 1.0 - rand(&mut seed) * p.jit_width.clamp(0.0, 0.9);
        }
        segment(
            &mut map,
            base,
            tip,
            hw.max(0.5),
            p.taper.clamp(0.0, 1.0),
            size,
        );
    }
    let (wi, hi_) = (size.0 as i32, size.1 as i32);
    map.retain(|idx, _| {
        let (ox, oy) = idx.origin();
        ox < wi && oy < hi_ && ox + TILE_SIZE as i32 > 0 && oy + TILE_SIZE as i32 > 0
    });
    map.into_iter().map(|(k, v)| (k, Arc::new(v))).collect()
}

/// Render a sea-urchin flash (or, with `solid`, its inverse) into sparse
/// tiles. Deterministic under `seed` like the other two.
pub fn render_urchin(p: &UrchinParams, size: (u32, u32)) -> HashMap<TileIdx, Arc<Tile>> {
    let mut map: HashMap<TileIdx, Tile> = HashMap::new();
    let mut seed = p.seed | 1;
    let n = p.count.max(1);
    let step = std::f32::consts::TAU / n as f32;
    let r_out = p.r_out.max(1.0);
    let r_in = p.r_in.clamp(0.0, r_out - 1.0);
    let span = r_out - r_in;
    // See UrchinParams::width — 90% of the gap, never more. The cap is
    // floored before the clamp because f32::clamp PANICS on min > max
    // and a tiny flash would otherwise abort through wndproc (audit B).
    let hw = (p.width * 0.5).clamp(0.5, (step * r_out * 0.45).max(0.5));
    let aj = p.angle_jitter.clamp(0.0, 0.5);
    let lj = p.length_jitter.clamp(0.0, 1.0);
    let teeth: Vec<Tooth> = (0..n)
        .map(|i| {
            let ang = i as f32 * step + (rand(&mut seed) - 0.5) * aj * step;
            let r_tip = r_out - rand(&mut seed) * lj * span * 0.5;
            let (s, c) = ang.sin_cos();
            Tooth {
                c,
                s,
                r_apex: r_in,
                r_base: r_tip,
                hw,
            }
        })
        .collect();

    if p.solid {
        // One scan over the ring, inking everything the teeth do NOT
        // cover. Only the NEIGHBOURING sectors' teeth are tested per
        // pixel — a tooth cannot wander further (angle_jitter is capped
        // at half a gap); testing all `count` teeth per pixel is a
        // hundred-million-test scan on a full-page burst.
        let c = p.center;
        let x0 = (c[0] - r_out - 1.0).max(0.0);
        let x1 = (c[0] + r_out + 1.0).min(size.0 as f32);
        let y0 = (c[1] - r_out - 1.0).max(0.0);
        let y1 = (c[1] + r_out + 1.0).min(size.1 as f32);
        if x0 < x1 && y0 < y1 {
            let (ri2, ro2) = (r_in * r_in, r_out * r_out);
            for y in y0.floor() as i32..=y1.ceil() as i32 {
                for x in x0.floor() as i32..=x1.ceil() as i32 {
                    let dx = x as f32 + 0.5 - c[0];
                    let dy = y as f32 + 0.5 - c[1];
                    let r2 = dx * dx + dy * dy;
                    if r2 < ri2 || r2 > ro2 {
                        continue;
                    }
                    let k = (dy.atan2(dx).rem_euclid(std::f32::consts::TAU) / step) as i32;
                    let cut =
                        (-1..=1).any(|o| teeth[(k + o).rem_euclid(n as i32) as usize].hit(dx, dy));
                    if !cut {
                        put(&mut map, x, y);
                    }
                }
            }
        }
    } else {
        for t in &teeth {
            fill_tooth(&mut map, p.center, t, size);
        }
    }

    let (wi, hi_) = (size.0 as i32, size.1 as i32);
    map.retain(|idx, _| {
        let (ox, oy) = idx.origin();
        ox < wi && oy < hi_ && ox + TILE_SIZE as i32 > 0 && oy + TILE_SIZE as i32 > 0
    });
    map.into_iter().map(|(k, v)| (k, Arc::new(v))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ink_at(map: &HashMap<TileIdx, Arc<Tile>>, x: i32, y: i32) -> bool {
        let idx = TileIdx::of_pixel(x, y);
        map.get(&idx).is_some_and(|t| {
            t.pixel((x - idx.origin().0) as usize, (y - idx.origin().1) as usize)[3] > 0
        })
    }

    /// Focus lines: ink near the outer ring at many angles, none inside
    /// the inner radius, deterministic under a fixed seed.
    #[test]
    fn focus_lines_ring_and_hole() {
        let p = FocusLinesParams {
            center: [256.0, 256.0],
            r_in: 100.0,
            r_out: 240.0,
            count: 64,
            width: 6.0,
            angle_jitter: 0.5,
            width_jitter: 0.5,
            length_jitter: 0.2,
            taper: 0.0,
            seed: 7,
        };
        let m = render_focus(&p, (512, 512));
        // Sectors with ink at r ≈ 200 — each sector samples a short ARC
        // (7 points ±3°) because a single point can fall between two
        // jittered lines.
        let mut sectors = 0;
        for k in 0..32 {
            let base = k as f32 * std::f32::consts::TAU / 32.0;
            let hit = (-3..=3).map(|d| d as f32).any(|d| {
                let a = base + d.to_radians();
                let (s, c) = a.sin_cos();
                ink_at(&m, (256.0 + c * 200.0) as i32, (256.0 + s * 200.0) as i32)
            });
            if hit {
                sectors += 1;
            }
        }
        assert!(sectors >= 26, "most sectors carry ink ({sectors}/32)");
        assert!(!ink_at(&m, 256, 256), "the hole is empty");
        let m2 = render_focus(&p, (512, 512));
        assert_eq!(m.len(), m2.len(), "seeded = deterministic");
    }

    /// The COLOUR pin (owner repro 2026-08-22). Every generator here inked
    /// `[ONE; 4]` — opaque white — from the first commit, so a placed layer
    /// showed nothing on a white page. Every other test in this module reads
    /// channel 3 alone, which is exactly why nothing caught it; this one
    /// reads channels 0..3 for all four generators.
    #[test]
    fn every_generator_inks_black_not_white() {
        let focus = render_focus(
            &FocusLinesParams {
                center: [256.0, 256.0],
                r_in: 100.0,
                r_out: 240.0,
                count: 64,
                width: 6.0,
                angle_jitter: 0.5,
                width_jitter: 0.5,
                length_jitter: 0.2,
                taper: 0.0,
                seed: 7,
            },
            (512, 512),
        );
        let speed = render_speed(
            &SpeedLinesParams {
                angle_deg: 20.0,
                count: 80,
                len_min: 100.0,
                len_max: 300.0,
                width: 4.0,
                taper: 0.0,
                converge: None,
                seed: 3,
                ..Default::default()
            },
            (512, 512),
        );
        let urchin = |solid| {
            render_urchin(
                &UrchinParams {
                    center: [256.0, 256.0],
                    r_in: 80.0,
                    r_out: 240.0,
                    count: 24,
                    width: 18.0,
                    angle_jitter: 0.2,
                    length_jitter: 0.2,
                    solid,
                    seed: 5,
                },
                (512, 512),
            )
        };
        for (what, m) in [
            ("focus", &focus),
            ("speed", &speed),
            ("urchin", &urchin(false)),
            ("solid flash", &urchin(true)),
        ] {
            let mut inked = 0u32;
            for t in m.values() {
                for y in 0..TILE_SIZE {
                    for x in 0..TILE_SIZE {
                        let p = t.pixel(x, y);
                        if p[3] == 0 {
                            continue;
                        }
                        inked += 1;
                        assert_eq!(
                            [p[0], p[1], p[2]],
                            [0, 0, 0],
                            "{what}: inked pixel at ({x}, {y}) is not black"
                        );
                    }
                }
            }
            assert!(inked > 0, "{what}: drew nothing to check");
        }
    }

    /// A position-sensitive fingerprint of a rendered layer: tile count,
    /// inked pixels, and a checksum that moves if a single pixel moves.
    pub(super) fn fingerprint(m: &HashMap<TileIdx, Arc<Tile>>) -> (usize, u64, u64) {
        let mut keys: Vec<_> = m.keys().copied().collect();
        keys.sort_by_key(|i| (i.y, i.x));
        let (mut px, mut sum) = (0u64, 0u64);
        for k in keys {
            let (ox, oy) = k.origin();
            for y in 0..TILE_SIZE {
                for x in 0..TILE_SIZE {
                    if m[&k].pixel(x, y)[3] > 0 {
                        px += 1;
                        let gx = (ox + x as i32) as u64;
                        let gy = (oy + y as i32) as u64;
                        sum = sum
                            .wrapping_mul(0x0100_0000_01B3)
                            .wrapping_add(gx.wrapping_mul(65_537).wrapping_add(gy));
                    }
                }
            }
        }
        (m.len(), px, sum)
    }

    /// BIT-STABILITY PIN (flash round, 2026-08-22): the two original
    /// renderers must keep drawing exactly what they drew before `kind`,
    /// `taper` and `converge` existed — every effect-line layer in every
    /// saved file regenerates through them. The numbers were taken from
    /// the pre-round code; a change that moves them has silently redrawn
    /// the owner's archive.
    #[test]
    fn legacy_renders_are_bit_stable() {
        let f = FocusLinesParams {
            center: [256.0, 256.0],
            r_in: 100.0,
            r_out: 240.0,
            count: 64,
            width: 6.0,
            angle_jitter: 0.5,
            width_jitter: 0.5,
            length_jitter: 0.2,
            taper: 0.0,
            seed: 7,
        };
        assert_eq!(
            fingerprint(&render_focus(&f, (512, 512))),
            (52, 37446, 14_909_681_065_247_512_801)
        );
        let s = SpeedLinesParams {
            angle_deg: 20.0,
            count: 80,
            len_min: 100.0,
            len_max: 300.0,
            width: 4.0,
            taper: 0.0,
            converge: None,
            seed: 3,
            ..Default::default()
        };
        assert_eq!(
            fingerprint(&render_speed(&s, (512, 512))),
            (57, 25119, 6_096_450_357_538_070_854)
        );

        // Density round, 2026-08-23: the legacy speed set THROUGH THE
        // SPEC, with every new attribute at its serde default. `gap_px` 0
        // must still mean the uniform scatter, the split jitters 0 the
        // single `jitter`, `gap_deg` 0 "use `count`" and `color` black —
        // so a saved layer regenerates onto the pixels it was saved with.
        // (The focus half is pinned the same way by
        // `pre_flash_specs_load_with_the_old_meaning`, which compares the
        // spec's raster to explicit params rather than to a constant.)
        assert_eq!(
            fingerprint(
                &GenLinesSpec {
                    focus: false,
                    a: 20.0,
                    b: 100.0,
                    c: 300.0,
                    count: 80,
                    width: 4.0,
                    seed: 3,
                    ..Default::default()
                }
                .render((512, 512))
            ),
            (57, 25119, 6_096_450_357_538_070_854)
        );
    }

    /// `gap_deg` is the same fan expressed in CSP's unit: 360/gap rays,
    /// and setting it to the gap the count already implied draws the same
    /// set. 0 keeps the count (pinned above).
    #[test]
    fn gap_deg_derives_the_ray_count() {
        let by_count = GenLinesSpec {
            focus: true,
            a: 256.0,
            b: 256.0,
            c: 100.0,
            d: 240.0,
            count: 90,
            width: 6.0,
            jitter: 0.3,
            seed: 7,
            ..Default::default()
        };
        assert_eq!(by_count.ray_count(), 90);
        let by_gap = GenLinesSpec {
            count: 1,
            gap_deg: 4.0,
            ..by_count
        };
        assert_eq!(by_gap.ray_count(), 90, "360 / 4°");
        assert_eq!(
            fingerprint(&by_count.render((512, 512))),
            fingerprint(&by_gap.render((512, 512))),
            "the same fan, said the other way round"
        );
        // A silly gap is capped rather than allowed to hang the UI.
        assert_eq!(
            GenLinesSpec {
                gap_deg: 0.01,
                ..by_count
            }
            .ray_count(),
            4096
        );
    }

    /// Speed lines: horizontal runs at many heights (0° set).
    #[test]
    fn speed_lines_parallel_bands() {
        let p = SpeedLinesParams {
            angle_deg: 0.0,
            count: 80,
            len_min: 100.0,
            len_max: 300.0,
            width: 4.0,
            taper: 0.0,
            converge: None,
            seed: 3,
            ..Default::default()
        };
        let m = render_speed(&p, (512, 512));
        let mut rows = 0;
        for y in (0..512).step_by(2) {
            if ink_at(&m, 256, y) {
                rows += 1;
            }
        }
        // ~30% of runs cross x=256 at these lengths; each covers ~2 of
        // the 2-px samples → tens of hits.
        assert!(rows >= 20, "many horizontal bands ({rows})");
        // And the spread reaches BOTH halves of the canvas.
        let top = (0..256).step_by(2).any(|y| ink_at(&m, 256, y));
        let bot = (256..512).step_by(2).any(|y| ink_at(&m, 256, y));
        assert!(top && bot, "runs scatter over the full normal extent");
    }

    /// Taper thins a run toward its TAIL: sampled across the same run,
    /// the head still inks at the full half-width and the tail no longer
    /// does. 0 changes nothing (pinned separately by the bit-stability
    /// fingerprint).
    #[test]
    fn focus_lines_taper_needles_at_the_centre() {
        // Jitters off so ray 0 sits exactly on angle 0 (the +x axis): the
        // ray's cross-section near r_in must be materially thinner than
        // near r_out under taper, and identical without it.
        let p = |taper: f32| FocusLinesParams {
            center: [256.0, 256.0],
            r_in: 40.0,
            r_out: 220.0,
            count: 8,
            width: 12.0,
            angle_jitter: 0.0,
            width_jitter: 0.0,
            length_jitter: 0.0,
            taper,
            seed: 7,
        };
        let cross = |m: &HashMap<TileIdx, Arc<Tile>>, x: i32| {
            (0..40).filter(|dy| ink_at(m, x, 256 - 20 + dy)).count() as i32
        };
        let flat = render_focus(&p(0.0), (512, 512));
        assert_eq!(
            cross(&flat, 256 + 50),
            cross(&flat, 256 + 210),
            "taper 0: constant width end to end"
        );
        let tapered = render_focus(&p(0.9), (512, 512));
        let inner = cross(&tapered, 256 + 50);
        let outer = cross(&tapered, 256 + 210);
        assert!(
            inner * 2 < outer && outer >= 10,
            "needles at the convergence, weight at the rim ({inner} vs {outer})"
        );
    }

    #[test]
    fn speed_lines_taper_thins_the_tail() {
        // The ramp itself, measured on one PLACED run so no scatter is in
        // the way: how tall is the run's column at the head, and at the
        // far end?
        let col = |taper: f32, x: i32| {
            let mut m: HashMap<TileIdx, Tile> = HashMap::new();
            segment(
                &mut m,
                [50.0, 256.0],
                [450.0, 256.0],
                8.0,
                taper,
                (512, 512),
            );
            (0..512)
                .filter(|y| {
                    let idx = TileIdx::of_pixel(x, *y);
                    m.get(&idx).is_some_and(|t| {
                        let (ox, oy) = idx.origin();
                        t.pixel((x - ox) as usize, (*y - oy) as usize)[3] > 0
                    })
                })
                .count()
        };
        assert_eq!(col(0.0, 60), col(0.0, 400), "0 = constant width, as before");
        assert_eq!(col(1.0, 60), col(0.0, 60), "the head keeps its width");
        let tail = col(1.0, 400);
        assert!(
            tail * 3 < col(1.0, 60),
            "the tail thinned to a needle ({tail})"
        );
        assert!(tail >= 1, "but the run still reaches its end");

        // And the parameter is plumbed through the generator.
        let p = SpeedLinesParams {
            angle_deg: 0.0,
            count: 40,
            len_min: 300.0,
            len_max: 300.0,
            width: 10.0,
            taper: 0.0,
            converge: None,
            seed: 5,
            ..Default::default()
        };
        let flat = fingerprint(&render_speed(&p, (512, 512)));
        let tapered = fingerprint(&render_speed(
            &SpeedLinesParams {
                taper: 0.8,
                ..p.clone()
            },
            (512, 512),
        ));
        assert!(
            flat.1 > 0 && tapered.1 * 4 < flat.1 * 3,
            "less ink on the page"
        );
    }

    /// Convergence leans the runs at a point instead of leaving them
    /// parallel: with the vanishing point straight above the canvas the
    /// block fans, so the runs no longer share one direction.
    #[test]
    fn speed_lines_converge_on_a_point() {
        let p = SpeedLinesParams {
            angle_deg: 0.0,
            count: 24,
            len_min: 200.0,
            len_max: 200.0,
            width: 3.0,
            taper: 0.0,
            converge: Some([256.0, -4000.0]),
            seed: 9,
            ..Default::default()
        };
        let m = render_speed(&p, (512, 512));
        assert!(!m.is_empty(), "the fan landed on the canvas");
        let par = render_speed(
            &SpeedLinesParams {
                converge: None,
                ..p.clone()
            },
            (512, 512),
        );
        assert_ne!(
            fingerprint(&m),
            fingerprint(&par),
            "aiming at the point moved the runs"
        );
    }

    /// Sea-urchin flash: filled spikes, wide at the rim and pointed at
    /// the hole — so a ring of probes just inside `r_out` finds far more
    /// ink than the same ring just outside `r_in`, and the hole is empty.
    #[test]
    fn urchin_flash_spikes_are_filled_wedges() {
        let p = UrchinParams {
            center: [256.0, 256.0],
            r_in: 60.0,
            r_out: 240.0,
            count: 32,
            width: 26.0,
            angle_jitter: 0.2,
            length_jitter: 0.1,
            solid: false,
            seed: 11,
        };
        let m = render_urchin(&p, (512, 512));
        let ring = |r: f32| {
            (0..720)
                .filter(|k| {
                    let a = *k as f32 * std::f32::consts::TAU / 720.0;
                    let (s, c) = a.sin_cos();
                    ink_at(&m, (256.0 + c * r) as i32, (256.0 + s * r) as i32)
                })
                .count()
        };
        let rim = ring(230.0);
        let near = ring(70.0);
        assert!(rim > 150, "the rim is mostly ink ({rim}/720)");
        assert!(near * 3 < rim, "and the points are thin ({near} vs {rim})");
        assert!(!ink_at(&m, 256, 256), "the hole stays empty");
        // A wedge is FILLED, not an outline: walk a rim spoke inward and
        // it stays inked for a long unbroken run.
        let mut best = 0;
        let mut run = 0;
        for k in 0..720 {
            let a = k as f32 * std::f32::consts::TAU / 720.0;
            let (s, c) = a.sin_cos();
            run = if ink_at(&m, (256.0 + c * 230.0) as i32, (256.0 + s * 230.0) as i32) {
                run + 1
            } else {
                0
            };
            best = best.max(run);
        }
        assert!(best >= 8, "a spike is a solid band at the rim ({best})");
        assert_eq!(
            fingerprint(&m),
            fingerprint(&render_urchin(&p, (512, 512))),
            "seeded = deterministic"
        );
    }

    /// Solid flash is the SAME teeth cut out of a solid ring: the hole
    /// is still empty, the ring is mostly ink where the urchin is mostly
    /// gaps, and the two are complementary inside the annulus.
    #[test]
    fn solid_flash_inverts_the_ring() {
        let mut p = UrchinParams {
            center: [256.0, 256.0],
            r_in: 60.0,
            r_out: 240.0,
            count: 32,
            width: 26.0,
            angle_jitter: 0.2,
            length_jitter: 0.0,
            solid: false,
            seed: 11,
        };
        let spikes = render_urchin(&p, (512, 512));
        p.solid = true;
        let solid = render_urchin(&p, (512, 512));
        assert!(!ink_at(&solid, 256, 256), "the hole stays empty");
        // Just inside the hole's edge the solid variant is unbroken ink
        // (the teeth are needle-thin there).
        let ring = |m: &HashMap<TileIdx, Arc<Tile>>, r: f32| {
            (0..720)
                .filter(|k| {
                    let a = *k as f32 * std::f32::consts::TAU / 720.0;
                    let (s, c) = a.sin_cos();
                    ink_at(m, (256.0 + c * r) as i32, (256.0 + s * r) as i32)
                })
                .count()
        };
        // Not 720/720: the teeth are still ~1 px wide this close to the
        // apex, so they nick a couple of probes each.
        assert!(ring(&solid, 70.0) > 600, "solid at the hole's edge");
        assert!(
            ring(&solid, 70.0) > ring(&spikes, 70.0) * 8,
            "and the polarity really is the other way round"
        );
        assert!(
            ring(&solid, 230.0) < ring(&spikes, 230.0),
            "gaps at the rim"
        );
        // Complementary: no pixel of the annulus carries ink in both.
        let mut both = 0;
        for k in 0..2000 {
            let a = k as f32 * 0.031;
            let r = 65.0 + (k % 170) as f32;
            let (x, y) = ((256.0 + a.cos() * r) as i32, (256.0 + a.sin() * r) as i32);
            if ink_at(&spikes, x, y) && ink_at(&solid, x, y) {
                both += 1;
            }
        }
        // Edge pixels of a tooth can round into both scans; a handful is
        // the rasterizer's seam, a flood would mean the polarity is off.
        assert!(both < 40, "the two polarities barely overlap ({both})");
    }

    /// The rows a set of horizontal runs occupies at one column, as the
    /// gaps between consecutive bands. The measure the density round is
    /// actually about: a hand-ruled 流線 block has ONE gap repeated, the
    /// old uniform-random scatter has gaps from 1 px to a bald strip.
    fn band_gaps(m: &HashMap<TileIdx, Arc<Tile>>, w: i32, h: i32) -> Vec<i32> {
        let mut centres = Vec::new();
        let mut run: Option<(i32, i32)> = None;
        for y in 0..h {
            if (0..w).any(|x| ink_at(m, x, y)) {
                run = Some(match run {
                    Some((a, _)) => (a, y),
                    None => (y, y),
                });
            } else if let Some((a, b)) = run.take() {
                centres.push((a + b) / 2);
            }
        }
        if let Some((a, b)) = run {
            centres.push((a + b) / 2);
        }
        centres.windows(2).map(|w| w[1] - w[0]).collect()
    }

    /// `gap_px` walks the normal extent instead of scattering, so the
    /// spacing is EVEN — the clumping the owner called "dogshit".
    ///
    /// Short runs on purpose: the along-the-direction start offset can
    /// still drop a run clean off the canvas (that is the legacy scatter
    /// and stays), and its odds fall with the run length, so a stray
    /// double gap does not have to be tolerated by a loose bound.
    #[test]
    fn speed_lines_gap_spacing_is_even() {
        let p = SpeedLinesParams {
            angle_deg: 0.0,
            count: 0,
            len_min: 20.0,
            len_max: 20.0,
            width: 2.0,
            gap_px: 16.0,
            seed: 5,
            ..Default::default()
        };
        let m = render_speed(&p, (512, 512));
        let gaps = band_gaps(&m, 512, 512);
        assert!(
            gaps.len() > 20,
            "the walk filled the extent ({})",
            gaps.len()
        );
        let lo = *gaps.iter().min().unwrap();
        let hi = *gaps.iter().max().unwrap();
        let nominal = gaps.iter().filter(|g| (**g - 16).abs() <= 1).count();
        assert!(
            hi <= lo * 2 + 2 && nominal * 10 >= gaps.len() * 9,
            "one gap, repeated ({lo}..{hi}, {nominal}/{} at 16)",
            gaps.len()
        );

        // And the same number of runs through the SCATTER path clumps —
        // this is the before picture, and it is why the field exists.
        let sg = band_gaps(
            &render_speed(
                &SpeedLinesParams {
                    count: gaps.len() as u32 + 1,
                    gap_px: 0.0,
                    ..p.clone()
                },
                (512, 512),
            ),
            512,
            512,
        );
        let s_hi = *sg.iter().max().unwrap();
        let s_lo = *sg.iter().min().unwrap();
        assert!(
            s_hi > s_lo * 4,
            "the old scatter really is uneven ({s_lo}..{s_hi})"
        );
    }

    /// まとまり: bundles of `group` runs with a hole between them — so the
    /// gap histogram has two values, not one, and the hole is the bigger.
    #[test]
    fn speed_lines_grouping_leaves_holes() {
        let m = render_speed(
            &SpeedLinesParams {
                angle_deg: 0.0,
                count: 0,
                len_min: 20.0,
                len_max: 20.0,
                width: 2.0,
                gap_px: 12.0,
                group: 3,
                group_gap: 3.0,
                seed: 5,
                ..Default::default()
            },
            (512, 512),
        );
        let gaps = band_gaps(&m, 512, 512);
        assert!(gaps.len() > 10, "enough bands to see the pattern");
        let tight = *gaps.iter().min().unwrap();
        assert!((tight - 12).abs() <= 1, "the bundle's own gap ({tight})");
        // The hole is 3 × the gap, and there are two tight gaps (a bundle
        // of three) for every one of them. A run that the along-the-
        // direction offset dropped off the canvas merges two neighbours,
        // so the counts are compared loosely — the SHAPE is the claim.
        let holes = gaps.iter().filter(|g| (**g - 36).abs() <= 2).count();
        let tights = gaps.iter().filter(|g| (**g - 12).abs() <= 1).count();
        assert!(holes >= 5, "bundles stand apart ({gaps:?})");
        assert!(
            tights * 2 >= holes * 3,
            "and each bundle is three tight runs ({tights} tight, {holes} holes)"
        );
    }

    /// The colour field: a white run on a black page is the knockout the
    /// black-only generator could not draw. Alpha is untouched.
    #[test]
    fn spec_color_paints_the_ink() {
        let mut spec = GenLinesSpec {
            focus: true,
            a: 256.0,
            b: 256.0,
            c: 60.0,
            d: 240.0,
            count: 32,
            width: 6.0,
            jitter: 0.2,
            seed: 7,
            ..Default::default()
        };
        let black = spec.render((512, 512));
        spec.color = [255, 255, 255];
        let white = spec.render((512, 512));
        assert_eq!(
            fingerprint(&black),
            fingerprint(&white),
            "colour moves no pixel"
        );
        let mut seen = 0;
        for t in white.values() {
            for y in 0..TILE_SIZE {
                for x in 0..TILE_SIZE {
                    let p = t.pixel(x, y);
                    if p[3] > 0 {
                        seen += 1;
                        assert_eq!([p[0], p[1], p[2]], [FIX15_ONE as u16; 3], "white ink");
                    }
                }
            }
        }
        assert!(seen > 0, "something was inked to check");
    }

    /// A degenerate flash (radius smaller than one spike) must not panic:
    /// the half-width cap is floored before `clamp`, which PANICS on
    /// min > max and would abort through wndproc (audit B).
    #[test]
    fn tiny_flash_does_not_panic() {
        for solid in [false, true] {
            let p = UrchinParams {
                center: [10.0, 10.0],
                r_in: 0.0,
                r_out: 0.2,
                count: 512,
                width: 40.0,
                angle_jitter: 1.0,
                length_jitter: 1.0,
                solid,
                seed: 1,
            };
            let _ = render_urchin(&p, (64, 64));
        }
    }
}

// --- SF-004/005 (TRIAGE 140, r85): the generator's parameters persist on
// the layer, so effect lines stay EDITABLE — the dialog reopens with the
// layer's own values and re-applies in place ("a week later" is the
// point). The dialog's (focus, a..d, count, width, jitter, seed) tuple
// is the serialized form; the two render fns remain the raster source.

/// A generated effect-line layer's parameters, as the dialog holds them.
///
/// `kind` is the generator discriminant, added 2026-08-22 with the flash
/// round. EVERY file written before that date has no such attribute, so
/// `#[serde(default)]` = `0` MUST keep meaning exactly what those files
/// meant — pinned by `pre_flash_specs_load_with_the_old_meaning`:
///
/// - `0` — the original pair, chosen by `focus`: 集中線 focus lines
///   (`a`,`b` = centre, `c` = r_in, `d` = r_out) or 流線 speed lines
///   (`a` = angle°, `b` = len_min, `c` = len_max, `d` unused).
/// - `1` — ウニフラッシュ sea-urchin flash: filled triangular spikes,
///   focus geometry, `width` = the spike base width in px at `r_out`.
/// - `2` — solid flash: the same teeth cut out of a solid ring.
///
/// An UNKNOWN kind falls back to the kind-0 reading rather than
/// rendering nothing: a file from a future build should look wrong, not
/// vanish. Kinds 1/2 keep `focus = true` on purpose — the Object tool's
/// driver handles and their drag clamps key on that flag, and a flash is
/// aimed exactly like a focus-line burst.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GenLinesSpec {
    pub focus: bool,
    /// focus: center.x, center.y, r_in, r_out; speed: angle_deg, len_min, len_max (d unused).
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub count: u32,
    pub width: f32,
    pub jitter: f32,
    pub seed: u64,
    /// Generator discriminant — see the type doc. Absent = 0 = legacy.
    #[serde(default)]
    pub kind: u8,
    /// Speed lines only: [`SpeedLinesParams::taper`]. Absent = 0 = the
    /// constant-width runs every older file drew.
    #[serde(default)]
    pub taper: f32,
    /// Speed lines only: [`SpeedLinesParams::converge`]. Absent = None.
    #[serde(default)]
    pub converge: Option<[f32; 2]>,

    // --- density round, 2026-08-23. EVERY field below is
    // `#[serde(default)]` and 0 MUST keep meaning exactly what a file
    // written before them meant, same rule as `kind`: the bit-stability
    // pin and `pre_flash_specs_load_with_the_old_meaning` are the guards.
    /// Radial kinds: the angular gap in DEGREES between neighbouring rays
    /// — CSP's tutorials size a 集中線 by gap (≈3° dense, ≈10° sparse),
    /// not by a count that means something different on every page size.
    /// >0 derives `count`; 0 keeps the stored `count`.
    #[serde(default)]
    pub gap_deg: f32,
    /// Speed lines: [`SpeedLinesParams::gap_px`]. 0 = the old scatter.
    #[serde(default)]
    pub gap_px: f32,
    /// Speed lines: [`SpeedLinesParams::group`] (まとまり).
    #[serde(default)]
    pub group: u32,
    /// Speed lines: [`SpeedLinesParams::group_gap`].
    #[serde(default)]
    pub group_gap: f32,
    /// 0 = fall back to the single `jitter` (which is what every older
    /// file has). Split because a printed set wants a lot of length
    /// wobble and almost no angular wobble, and one knob cannot say that.
    #[serde(default)]
    pub jit_gap: f32,
    #[serde(default)]
    pub jit_len: f32,
    #[serde(default)]
    pub jit_width: f32,
    /// The ink colour, sRGB. Absent = `[0, 0, 0]` = the black every older
    /// file drew — which is also the only value that touches no pixel
    /// (see [`recolor`]), so the legacy raster is bit-identical.
    #[serde(default)]
    pub color: [u8; 3],

    // --- screen-side only: these drive the Object tool's handles and
    // never reach a renderer, so they cannot move a saved raster.
    /// Radial kinds: the angle (degrees) the r_in/r_out driver handles sit
    /// at — the direction the placing drag was made in, so the handles
    /// land where the gesture did instead of always due east (and off the
    /// page for a burst near the right edge). 0 = +x, the old placement.
    #[serde(default)]
    pub hand_deg: f32,
    /// Speed lines: where the blue reference line and its handles are
    /// anchored — the placing drag's midpoint. `None` = the canvas
    /// centre, which is where they used to be for every run on the page.
    #[serde(default)]
    pub anchor: Option<[f32; 2]>,
}

/// Repaint an already-rendered set from black to `color`, premultiplied.
///
/// A post-pass rather than a colour argument threaded through four
/// rasterizers: the generators ink FULL alpha only, so premultiplied
/// recolouring is exact, and `[0, 0, 0]` returns without touching a pixel
/// — which is what keeps every saved layer bit-identical.
fn recolor(map: &mut HashMap<TileIdx, Arc<Tile>>, color: [u8; 3]) {
    if color == [0, 0, 0] {
        return;
    }
    let c = color.map(|v| ((v as u32 * FIX15_ONE as u32) / 255) as u16);
    for tile in map.values_mut() {
        let t = Arc::make_mut(tile);
        let d = t.data_mut();
        for px in d.chunks_exact_mut(4) {
            if px[3] > 0 {
                px[0] = c[0];
                px[1] = c[1];
                px[2] = c[2];
            }
        }
    }
}

impl GenLinesSpec {
    /// The layer name a fresh generation gets — one place, so the app,
    /// the Materials bank and the dialog cannot disagree.
    pub fn layer_name(&self) -> &'static str {
        match self.kind {
            1 => "Urchin flash",
            2 => "Solid flash",
            _ if self.focus => "Focus lines",
            _ => "Speed lines",
        }
    }

    /// Does this generator converge on a point (focus lines and both
    /// flashes)? The aim-at-the-click paste rule keys on this — keying on
    /// `focus` alone left kind 1/2 materials placing at their stored
    /// centre instead of the cursor (M7 audit finding).
    pub fn radial(&self) -> bool {
        self.kind == 1 || self.kind == 2 || self.focus
    }

    /// How many rays/spikes a radial kind draws: gap-driven when
    /// `gap_deg` is set (CSP's own unit for a 集中線), else the stored
    /// count. Capped — a 0.05° gap is 7 200 rays and a UI hang.
    pub fn ray_count(&self) -> u32 {
        if self.gap_deg > 0.0 {
            ((360.0 / self.gap_deg).ceil() as u32).clamp(1, 4096)
        } else {
            self.count.max(1)
        }
    }

    /// One of the split jitters, falling back to the single legacy
    /// `jitter` while it is 0 — the whole back-compat rule in one place.
    fn jit(&self, v: f32) -> f32 {
        if v > 0.0 { v } else { self.jitter }
    }

    /// Rasterize the spec into tiles (the shared source with the dialog).
    pub fn render(&self, size: (u32, u32)) -> HashMap<TileIdx, Arc<Tile>> {
        let mut map = if self.kind == 1 || self.kind == 2 {
            render_urchin(
                &UrchinParams {
                    center: [self.a, self.b],
                    r_in: self.c,
                    r_out: self.d,
                    count: self.ray_count(),
                    width: self.width,
                    angle_jitter: self.jit(self.jit_gap),
                    length_jitter: self.jit(self.jit_len),
                    solid: self.kind == 2,
                    seed: self.seed,
                },
                size,
            )
        } else if self.focus {
            render_focus(
                &FocusLinesParams {
                    center: [self.a, self.b],
                    r_in: self.c,
                    r_out: self.d,
                    count: self.ray_count(),
                    width: self.width,
                    angle_jitter: self.jit(self.jit_gap),
                    width_jitter: self.jit(self.jit_width),
                    length_jitter: self.jit(self.jit_len),
                    taper: self.taper.clamp(0.0, 1.0),
                    seed: self.seed,
                },
                size,
            )
        } else {
            render_speed(
                &SpeedLinesParams {
                    angle_deg: self.a,
                    count: self.count.max(1),
                    len_min: self.b,
                    len_max: self.c,
                    width: self.width,
                    taper: self.taper,
                    converge: self.converge,
                    gap_px: self.gap_px,
                    group: self.group,
                    group_gap: self.group_gap,
                    jit_gap: self.jit_gap,
                    jit_len: self.jit_len,
                    jit_width: self.jit_width,
                    seed: self.seed,
                },
                size,
            )
        };
        recolor(&mut map, self.color);
        map
    }
}

#[cfg(test)]
mod spec_tests {
    use super::*;
    use crate::doc::Document;

    /// HARD REQUIREMENT (flash round, 2026-08-22): an .ora or a
    /// `.gen.json` material written before `kind`/`taper`/`converge`
    /// existed carries only the nine original attributes, and must
    /// deserialize into a spec that renders exactly what it rendered
    /// then — not "close", the same tiles.
    #[test]
    fn pre_flash_specs_load_with_the_old_meaning() {
        let legacy_focus = r#"{"focus":true,"a":256.0,"b":256.0,"c":100.0,"d":240.0,"count":64,"width":6.0,"jitter":0.5,"seed":7}"#;
        let s: GenLinesSpec = serde_json::from_str(legacy_focus).expect("old spec still loads");
        assert_eq!(s.kind, 0, "no attribute = the original pair");
        assert_eq!(s.taper, 0.0);
        assert_eq!(s.converge, None);
        assert_eq!(s.layer_name(), "Focus lines");
        let old = render_focus(
            &FocusLinesParams {
                center: [256.0, 256.0],
                r_in: 100.0,
                r_out: 240.0,
                count: 64,
                width: 6.0,
                angle_jitter: 0.5,
                width_jitter: 0.5,
                length_jitter: 0.5,
                taper: 0.0,
                seed: 7,
            },
            (512, 512),
        );
        let new = s.render((512, 512));
        assert_eq!(old.len(), new.len(), "same tiles");
        for (idx, t) in &old {
            assert_eq!(t.data(), new[idx].data(), "legacy focus raster moved");
        }

        let legacy_speed = r#"{"focus":false,"a":20.0,"b":100.0,"c":300.0,"d":0.0,"count":80,"width":4.0,"jitter":0.0,"seed":3}"#;
        let q: GenLinesSpec = serde_json::from_str(legacy_speed).unwrap();
        assert_eq!((q.kind, q.taper, q.converge), (0, 0.0, None));
        assert_eq!(q.layer_name(), "Speed lines");
        let speed = q.render((512, 512));
        assert_eq!(
            super::tests::fingerprint(&speed),
            (57, 25119, 6_096_450_357_538_070_854),
            "the pre-round speed raster, from the pre-round attributes"
        );

        // And the round trip out is still readable BY an old build: the
        // three new attributes are the only additions, and each of them
        // reads back as the value a missing one defaults to.
        let back: GenLinesSpec = serde_json::from_str(&serde_json::to_string(&q).unwrap()).unwrap();
        assert_eq!(back, q);
    }

    /// The flash kinds ride the same field set, survive ORA, and stay
    /// distinguishable — a saved urchin does not reload as focus lines.
    #[test]
    fn flash_kinds_round_trip_through_ora() {
        for (kind, name) in [(1u8, "Urchin flash"), (2, "Solid flash")] {
            let spec = GenLinesSpec {
                focus: true,
                a: 200.0,
                b: 200.0,
                c: 40.0,
                d: 180.0,
                count: 40,
                width: 20.0,
                jitter: 0.25,
                seed: 7,
                kind,
                ..Default::default()
            };
            assert_eq!(spec.layer_name(), name);
            let mut doc = Document::new(400, 400);
            let li = doc.add_layer(name);
            doc.layers[li].genlines = Some(spec);
            assert!(doc.regen_genlines(li, spec), "{name} inked");

            let mut buf = std::io::Cursor::new(Vec::new());
            crate::ora::save_to(&doc, &mut buf).unwrap();
            let re = crate::ora::load_from(std::io::Cursor::new(buf.into_inner())).unwrap();
            let g = re.layers[li].genlines.expect("spec survived");
            assert_eq!(g, spec, "{name}: kind survived the save");
        }
    }

    /// SF-004/005: the spec persists through ORA and regen renders from
    /// it — a re-applied layer keeps its stack position, the tiles follow
    /// the new params.
    #[test]
    fn spec_round_trips_and_regens_in_place() {
        let mut doc = Document::new(400, 400);
        doc.add_layer("Focus lines");
        let spec = GenLinesSpec {
            focus: true,
            a: 200.0,
            b: 200.0,
            c: 20.0,
            d: 180.0,
            count: 40,
            width: 2.0,
            jitter: 0.2,
            seed: 7,
            ..Default::default()
        };
        let li = doc.layers.len() - 1;
        doc.layers[li].genlines = Some(spec);
        assert!(doc.regen_genlines(li, spec));
        assert!(doc.layers[li].tiles().next().is_some(), "focus lines inked");

        let mut buf = std::io::Cursor::new(Vec::new());
        crate::ora::save_to(&doc, &mut buf).unwrap();
        let re = crate::ora::load_from(std::io::Cursor::new(buf.into_inner())).unwrap();
        let gl = re
            .layers
            .iter()
            .position(|l| l.name == "Focus lines")
            .unwrap();
        let g = re.layers[gl].genlines.expect("spec survived");
        assert_eq!(g, spec);

        // Change a param: regen follows, same layer, and the new spec is
        // stored BY the regen (it owns both halves now).
        let mut doc = re;
        let mut s2 = g;
        s2.count = 80;

        assert!(doc.regen_genlines(gl, s2));
        assert!(doc.layers[gl].tiles().next().is_some(), "regen inked");
        assert_eq!(doc.layers[gl].genlines, Some(s2), "the spec went on");
        assert!(
            !doc.regen_genlines(usize::MAX, s2),
            "out-of-bounds index, no regen"
        );
        // A real layer that carries NO spec also refuses (audit H: the
        // old test only exercised the out-of-bounds arm).
        let plain = doc.add_layer("plain");
        assert!(
            !doc.regen_genlines(plain, s2),
            "layer without spec, no regen"
        );
    }

    #[test]
    fn failed_regen_keeps_spec_and_tiles_agreeing() {
        // Audit F, 2026-08-19: a regen that renders nothing must move
        // NEITHER half — the stored spec still describes the pixels that
        // are on screen (the store now happens inside regen_genlines, so
        // this pins both halves rather than the app's old dance).
        let mut doc = Document::new(400, 400);
        doc.add_layer("Focus lines");
        let li = doc.layers.len() - 1;
        let spec = GenLinesSpec {
            focus: true,
            a: 200.0,
            b: 200.0,
            c: 20.0,
            d: 180.0,
            count: 40,
            width: 2.0,
            jitter: 0.2,
            seed: 7,
            ..Default::default()
        };
        doc.layers[li].genlines = Some(spec);
        assert!(doc.regen_genlines(li, spec));
        let tiles_before: Vec<_> = doc.layers[li]
            .tiles()
            .map(|(i, t)| (i, t.clone()))
            .collect();
        assert!(!tiles_before.is_empty());

        // A spec that renders nothing (convergence point far off the
        // canvas — the clip drops every tile): regen refuses, the inked
        // raster stays exactly as it was.
        let mut dead = spec;
        dead.a = -10000.0;
        dead.b = -10000.0;
        assert!(!doc.regen_genlines(li, dead), "nothing rendered, no regen");
        assert_eq!(
            doc.layers[li].genlines,
            Some(spec),
            "the dead spec was not stored"
        );
        let tiles_after: Vec<_> = doc.layers[li]
            .tiles()
            .map(|(i, t)| (i, t.clone()))
            .collect();
        assert_eq!(tiles_before.len(), tiles_after.len(), "tiles unchanged");
        for ((i0, t0), (i1, t1)) in tiles_before.iter().zip(tiles_after.iter()) {
            assert_eq!(i0, i1);
            assert_eq!(t0.data(), t1.data());
        }
    }

    #[test]
    fn regen_is_one_undo_step_and_keeps_the_layers_history() {
        // Audit F's old shape: replace_tiles swapped the raster wholesale,
        // past the copy-on-write recording, so the regen was not undoable
        // and had to purge the layer's pre-images to stay consistent. It
        // now writes through set_tile inside the op bracket — ONE step, and
        // the ink that was on the layer before the regen still undoes.
        let mut doc = Document::new(400, 400);
        let li = doc.add_layer("Focus lines");
        let spec = GenLinesSpec {
            focus: true,
            a: 200.0,
            b: 200.0,
            c: 20.0,
            d: 180.0,
            count: 40,
            width: 2.0,
            jitter: 0.2,
            seed: 7,
            ..Default::default()
        };
        // A first generation, then an ordinary tile write on top of it:
        // two steps for the regen under test to sit above.
        doc.layers[li].genlines = Some(spec);
        assert!(doc.regen_genlines(li, spec));
        doc.begin_op_on(li);
        doc.set_op_label("Stroke");
        doc.layers[li].set_tile(
            crate::tile::TileIdx::new(0, 0),
            Some(std::sync::Arc::new(crate::tile::Tile::default())),
        );
        doc.end_op();
        assert_eq!(
            doc.undo_labels(),
            ["New layer", "Regenerate lines", "Stroke"],
            "the setup's structural add records too"
        );
        let snap = |d: &Document| -> std::collections::BTreeMap<crate::tile::TileIdx, Vec<u16>> {
            d.layers[li]
                .tiles()
                .map(|(i, t)| (i, t.data().to_vec()))
                .collect()
        };
        let before = snap(&doc);

        let mut s2 = spec;
        s2.count = 90;
        s2.seed = 11;
        assert!(doc.regen_genlines(li, s2));
        let regenerated = snap(&doc);
        assert_ne!(before, regenerated, "the regen changed the raster");
        assert_eq!(
            doc.undo_labels(),
            [
                "New layer",
                "Regenerate lines",
                "Stroke",
                "Regenerate lines"
            ],
            "one step for the regen, and the older steps survived it"
        );

        assert!(doc.undo(), "the regen undoes");
        assert_eq!(snap(&doc), before, "pixels back, bit for bit");
        assert_eq!(doc.layers[li].genlines, Some(spec), "and the parameters");
        assert!(doc.redo(), "and redoes");
        assert_eq!(snap(&doc), regenerated);
        assert_eq!(doc.layers[li].genlines, Some(s2));

        // The pre-regen history is still walkable: the stroke, then the
        // first generation.
        assert!(doc.undo() && doc.undo(), "back past the stroke");
        assert!(doc.undo(), "back past the first generation");
        assert!(
            doc.layers[li].tiles().next().is_none(),
            "the layer is empty again"
        );
    }
}
