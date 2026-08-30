//! The blur family's kernels — CSP Filter ▸ Blur, plus the unsharp mask that
//! subtracts one. Moved here verbatim when `filter.rs` was split; every
//! entry point is still reached through [`super::Filter`]'s dispatch, and
//! the halo each one needs is still declared by [`super::Filter::reach`].

use super::{Filter, MAX_SIGMA, MotionDir, MotionMode, Raster, RasterKernel, Smear, sample_bilinear};
use crate::tile::TILE_CHANNELS;

// ----------------------------------------------------------------- kernels --

/// Kovesi's three-box approximation of a Gaussian: the box widths whose
/// successive application has (very nearly) the requested σ.
///
/// Three uniform passes convolve to a quadratic B-spline, which is within
/// ~0.5 % of a true Gaussian and costs O(1) per pixel per pass instead of
/// O(σ). At 600 dpi a σ-20 shadow is a 121-tap kernel; the direct form is not
/// shippable and the approximation is what every fast implementation uses.
///
/// The cost of the trick is quantisation at the bottom of the range: the
/// widths are odd integers, so σ below ~1 rounds to three identity passes.
/// [`super::Filter::is_identity`] reports that rather than pretending.
pub(super) fn box_radii(sigma: f32) -> [usize; 3] {
    const N: f32 = 3.0;
    let sigma = sigma.clamp(0.0, MAX_SIGMA);
    if !(sigma > 0.0) {
        return [0; 3];
    }
    let v = 12.0 * sigma * sigma;
    let mut wl = (v / N + 1.0).sqrt().floor() as i32;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wl = wl.max(1);
    let wu = wl + 2;
    let m = ((v - N * (wl * wl) as f32 - 4.0 * N * wl as f32 - 3.0 * N) / (-4.0 * wl as f32 - 4.0))
        .round()
        .clamp(0.0, N) as usize;
    let mut out = [0usize; 3];
    for (i, o) in out.iter_mut().enumerate() {
        let w = if i < m { wl } else { wu } as usize;
        *o = (w - 1) / 2;
    }
    out
}

/// Total reach of the three box passes — they compose, so the radii add.
pub(super) fn gaussian_reach(sigma: f32) -> i32 {
    box_radii(sigma).iter().sum::<usize>() as i32
}

/// One horizontal box pass, running-sum. Outside the buffer counts as
/// transparent (the denominator stays the full window), which is the same
/// convention the gather uses for absent tiles.
fn box_h(src: &Raster, dst: &mut Raster, r: usize) {
    let (w, h) = (src.w, src.h);
    let denom = (2 * r + 1) as u32;
    let half = denom / 2;
    for y in 0..h {
        let row = y * w * TILE_CHANNELS;
        let mut acc = [0u32; TILE_CHANNELS];
        for x in 0..(r + 1).min(w) {
            let o = row + x * TILE_CHANNELS;
            for (c, a) in acc.iter_mut().enumerate() {
                *a += src.px[o + c] as u32;
            }
        }
        for x in 0..w {
            let o = row + x * TILE_CHANNELS;
            for (c, a) in acc.iter().enumerate() {
                dst.px[o + c] = ((*a + half) / denom) as u16;
            }
            if x >= r {
                let drop = row + (x - r) * TILE_CHANNELS;
                for (c, a) in acc.iter_mut().enumerate() {
                    *a -= src.px[drop + c] as u32;
                }
            }
            let add = x + r + 1;
            if add < w {
                let take = row + add * TILE_CHANNELS;
                for (c, a) in acc.iter_mut().enumerate() {
                    *a += src.px[take + c] as u32;
                }
            }
        }
    }
}

/// One vertical box pass. Same running sum, striding by a row.
fn box_v(src: &Raster, dst: &mut Raster, r: usize) {
    let (w, h) = (src.w, src.h);
    let denom = (2 * r + 1) as u32;
    let half = denom / 2;
    let stride = w * TILE_CHANNELS;
    for x in 0..w {
        let col = x * TILE_CHANNELS;
        let mut acc = [0u32; TILE_CHANNELS];
        for y in 0..(r + 1).min(h) {
            let o = col + y * stride;
            for (c, a) in acc.iter_mut().enumerate() {
                *a += src.px[o + c] as u32;
            }
        }
        for y in 0..h {
            let o = col + y * stride;
            for (c, a) in acc.iter().enumerate() {
                dst.px[o + c] = ((*a + half) / denom) as u16;
            }
            if y >= r {
                let drop = col + (y - r) * stride;
                for (c, a) in acc.iter_mut().enumerate() {
                    *a -= src.px[drop + c] as u32;
                }
            }
            let add = y + r + 1;
            if add < h {
                let take = col + add * stride;
                for (c, a) in acc.iter_mut().enumerate() {
                    *a += src.px[take + c] as u32;
                }
            }
        }
    }
}

/// FL-011: separable Gaussian, in place. Three box passes per axis; the
/// horizontal ones all run before the vertical ones, which is legal because
/// box blur is separable and the passes commute.
pub(super) fn gaussian(buf: &mut Raster, sigma: f32) {
    let radii = box_radii(sigma);
    if radii.iter().all(|&r| r == 0) {
        return;
    }
    let mut tmp = Raster::new(buf.w, buf.h);
    for r in radii {
        if r > 0 {
            box_h(buf, &mut tmp, r);
            std::mem::swap(buf, &mut tmp);
        }
    }
    for r in radii {
        if r > 0 {
            box_v(buf, &mut tmp, r);
            std::mem::swap(buf, &mut tmp);
        }
    }
}

/// FL-013: the 3×3 binomial [1 2 1]⊗[1 2 1]/16, separable, in place. Weak on
/// purpose — its job is filling in the missing intermediate values along a
/// jagged edge, not softening the drawing.
pub(super) fn smoothing(buf: &mut Raster) {
    let mut tmp = Raster::new(buf.w, buf.h);
    tent_h(buf, &mut tmp);
    std::mem::swap(buf, &mut tmp);
    tent_v(buf, &mut tmp);
    std::mem::swap(buf, &mut tmp);
}

fn tent_h(src: &Raster, dst: &mut Raster) {
    let (w, h) = (src.w, src.h);
    for y in 0..h {
        for x in 0..w {
            let (l, m, r) = (
                src.pixel(x.wrapping_sub(1), y),
                src.pixel(x, y),
                src.pixel(x + 1, y),
            );
            let mut out = [0u16; TILE_CHANNELS];
            for c in 0..TILE_CHANNELS {
                out[c] = ((l[c] as u32 + 2 * m[c] as u32 + r[c] as u32 + 2) / 4)
                    .min(u16::MAX as u32) as u16;
            }
            dst.set_pixel(x, y, out);
        }
    }
}

fn tent_v(src: &Raster, dst: &mut Raster) {
    let (w, h) = (src.w, src.h);
    for y in 0..h {
        for x in 0..w {
            let (u, m, d) = (
                src.pixel(x, y.wrapping_sub(1)),
                src.pixel(x, y),
                src.pixel(x, y + 1),
            );
            let mut out = [0u16; TILE_CHANNELS];
            for c in 0..TILE_CHANNELS {
                out[c] = ((u[c] as u32 + 2 * m[c] as u32 + d[c] as u32 + 2) / 4)
                    .min(u16::MAX as u32) as u16;
            }
            dst.set_pixel(x, y, out);
        }
    }
}

/// The parameter range a motion blur integrates over, in pixels along the
/// angle. Shared by [`super::Filter::reach`] and [`motion`] so the halo and the taps
/// can never disagree.
pub(super) fn motion_span(length: f32, dir: MotionDir) -> (f32, f32) {
    let l = length.max(0.0).min(4096.0);
    match dir {
        MotionDir::Both => (-l * 0.5, l * 0.5),
        MotionDir::Forward => (0.0, l),
        MotionDir::Backward => (-l, 0.0),
    }
}

/// FL-015: a directional line integral — the same machinery as the Gaussian,
/// walked along one angle instead of the two axes. Not separable, so it is the
/// one filter here whose cost grows with its parameter.
pub(super) fn motion(buf: &mut Raster, angle_deg: f32, length: f32, dir: MotionDir, mode: MotionMode) {
    let (t0, t1) = motion_span(length, dir);
    let span = t1 - t0;
    if span <= 0.5 {
        return;
    }
    // One sample per pixel of travel; bilinear between them, so a 37° streak
    // does not come out as a staircase.
    let n = (span.ceil() as usize + 1).max(2);
    let a = angle_deg.to_radians();
    let (dx, dy) = (a.cos(), a.sin());
    let far = t0.abs().max(t1.abs()).max(1e-6);
    let mut src = Raster::new(buf.w, buf.h);
    std::mem::swap(buf, &mut src);
    for y in 0..src.h {
        for x in 0..src.w {
            let mut acc = [0f32; TILE_CHANNELS];
            let mut wsum = 0f32;
            for i in 0..n {
                let t = t0 + span * (i as f32) / ((n - 1) as f32);
                let w = match mode {
                    MotionMode::Uniform => 1.0,
                    // Linear taper to zero at the far end; the +ε keeps the
                    // outermost sample from contributing literally nothing.
                    MotionMode::Taper => 1.0 - (t.abs() / far) * 0.999,
                };
                let p = sample_bilinear(&src, x as f32 + t * dx, y as f32 + t * dy);
                for c in 0..TILE_CHANNELS {
                    acc[c] += p[c] * w;
                }
                wsum += w;
            }
            let mut out = [0u16; TILE_CHANNELS];
            for c in 0..TILE_CHANNELS {
                out[c] = (acc[c] / wsum + 0.5).clamp(0.0, u16::MAX as f32) as u16;
            }
            buf.set_pixel(x, y, out);
        }
    }
}

/// The centre both smears turn about: the buffer's own centre.
fn smear_centre(w: usize, h: usize) -> [f32; 2] {
    [(w as f32 - 1.0) * 0.5, (h as f32 - 1.0) * 0.5]
}

/// FL-016: the zoom smear — dest pixel p averages the segment of its own
/// ray from `p · (1−k)` to `p` (k = strength), uniformly weighted over
/// taps that scale with the smear. Premultiplied averaging, exactly the
/// motion blur's arithmetic walked radially instead of linearly.
///
/// Expressed as a [`Smear`] — one scale matrix per sample — because that is
/// the form the GPU kernel can run; [`super::Filter::smear_samples`] is the
/// seam and this is the only place the numbers live.
pub(super) fn radial_samples(w: usize, h: usize, strength: f32) -> Option<Smear> {
    let k = strength.clamp(0.0, 0.95);
    if k <= 0.02 {
        return None;
    }
    let n = ((k * w.min(h) as f32).ceil() as usize).clamp(8, 48);
    Some(Smear {
        centre: smear_centre(w, h),
        mats: (0..n)
            .map(|i| {
                let t = 1.0 - k * (i as f32) / ((n - 1) as f32);
                [t, 0.0, 0.0, t]
            })
            .collect(),
    })
}

/// FL-017: the rotational smear — dest pixel p averages the arc of ±a
/// about the centre at its own radius. Near the centre the arc is short
/// (the samples collapse onto p), which is physically right: a spin
/// blurs the rim far more than the axle.
///
/// The arc is spelled as a ROTATION MATRIX per sample rather than as the
/// polar round trip (`r = hypot(u)`, `θ₀ = atan2(u)`, sample at
/// `c + r·(cos, sin)(θ₀ + φ)`) it started as. Same operator — rotating `u`
/// by φ is what that computes — but the per-sample form has two properties
/// the polar one lacks: the transcendentals are evaluated `n` times for the
/// whole buffer instead of `2n + 2` times per pixel, and the inner loop is
/// left with nothing but multiplies and adds, which is what makes the
/// operator expressible on the GPU at all (WGSL's `sin`/`cos` are only
/// promised to 2⁻¹¹ absolute, so a shader that recomputed the angle would
/// land up to `r/2048` pixels away from the CPU and no honest tolerance
/// could cover it).
pub(super) fn spin_samples(w: usize, h: usize, angle_deg: f32) -> Option<Smear> {
    let a = angle_deg.clamp(0.5, 180.0).to_radians();
    let centre = smear_centre(w, h);
    let max_r = centre[0].max(centre[1]);
    let n = ((a * max_r).ceil() as usize).clamp(8, 48);
    Some(Smear {
        centre,
        mats: (0..n)
            .map(|i| {
                let t = (i as f32) / ((n - 1) as f32) * 2.0 - 1.0;
                let (s, c) = (a * t).sin_cos();
                [c, -s, s, c]
            })
            .collect(),
    })
}

/// The CPU reference for a [`Smear`]: dest pixel p averages one bilinear tap
/// per sample matrix, taken at `centre + M·(p − centre)`. Uniform weights —
/// the `n` samples ARE the weighting, exactly as the two smears had it.
pub(super) fn smear(buf: &mut Raster, s: &Smear) {
    let n = s.mats.len();
    if n == 0 {
        return;
    }
    let c = s.centre;
    let mut src = Raster::new(buf.w, buf.h);
    std::mem::swap(buf, &mut src);
    for y in 0..src.h {
        for x in 0..src.w {
            let (ux, uy) = (x as f32 - c[0], y as f32 - c[1]);
            let mut acc = [0f32; TILE_CHANNELS];
            for m in &s.mats {
                let p = sample_bilinear(
                    &src,
                    c[0] + m[0] * ux + m[1] * uy,
                    c[1] + m[2] * ux + m[3] * uy,
                );
                for (a, v) in acc.iter_mut().zip(p) {
                    *a += v;
                }
            }
            let mut out = [0u16; TILE_CHANNELS];
            for (o, a) in out.iter_mut().zip(acc) {
                *o = (a / n as f32 + 0.5).clamp(0.0, u16::MAX as f32) as u16;
            }
            buf.set_pixel(x, y, out);
        }
    }
}

/// FL-014: the classic unsharp mask — `out = orig + (orig − blur)·amount`,
/// the blur being the same Kovesi three-box Gaussian the blur family runs. All
/// the sharpening is in the sign: the difference is large only where the blur
/// disagrees with the original, which is exactly at an edge, so a flat field
/// comes out untouched and an edge gains the overshoot on both sides that
/// reads as "crisper".
///
/// Sharpened premultiplied, then REPAIRED. Colour and alpha overshoot
/// independently, and an overshot colour channel can land above the alpha it
/// is premultiplied by, which is not a representable pixel — so alpha is
/// computed first and clamps the three colour channels. That is the right
/// answer visually too: an over-sharpened edge should saturate, not glow.
pub(super) fn unsharp(buf: &mut Raster, radius: f32, amount: f32) {
    let amount = amount.clamp(0.0, 10.0);
    if amount <= 0.01 || box_radii(radius).iter().all(|&r| r == 0) {
        return;
    }
    let orig = buf.clone();
    gaussian(buf, radius);
    combine(&orig, buf, amount);
}

/// FL-014 for a host that has a blur kernel: the blur half goes through the
/// lent kernel as the [`Filter::Gaussian`] it literally is, and the combine
/// runs here. Byte-identical to [`unsharp`] by construction — same
/// `box_radii`, same [`combine`] — as long as the kernel's blur is, which is
/// what the separable seam already guarantees.
///
/// Returns false when the kernel declined, having touched nothing: the
/// contract is that a declining kernel leaves the buffer alone, so the
/// caller's `Filter::run` fallback still sees original pixels. Not "blur on
/// the CPU and combine here" on that path, because that would be the whole
/// reference anyway with an extra buffer copy.
pub(super) fn unsharp_split(
    buf: &mut Raster,
    radius: f32,
    amount: f32,
    run: &mut RasterKernel<'_>,
) -> bool {
    let amount = amount.clamp(0.0, 10.0);
    if amount <= 0.01 || box_radii(radius).iter().all(|&r| r == 0) {
        return false;
    }
    let orig = buf.clone();
    if !run(Filter::Gaussian { sigma: radius }, buf) {
        return false;
    }
    combine(&orig, buf, amount);
    true
}

/// `out = orig + (orig − blur)·amount`, in place over the blurred buffer.
/// The one copy of the arithmetic both unsharp paths run.
fn combine(orig: &Raster, buf: &mut Raster, amount: f32) {
    for i in (0..buf.px.len()).step_by(TILE_CHANNELS) {
        let mut out = [0u16; TILE_CHANNELS];
        // Alpha (channel 3) first — it is the ceiling for the other three.
        for c in (0..TILE_CHANNELS).rev() {
            let o = orig.px[i + c] as f32;
            let v = o + (o - buf.px[i + c] as f32) * amount;
            let hi = if c == 3 {
                crate::blend::FIX15_ONE_F
            } else {
                out[3] as f32
            };
            out[c] = (v + 0.5).clamp(0.0, hi) as u16;
        }
        buf.px[i..i + TILE_CHANNELS].copy_from_slice(&out);
    }
}
