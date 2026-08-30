//! The distort family — CSP Filter ▸ Distort (FL-020..023). Moved here
//! verbatim when `filter.rs` was split; dispatch and the halo declaration
//! stay in [`super::Filter`].

use super::{Raster, WaveDir, sample_bilinear};
use crate::tile::TILE_CHANNELS;

// --------------------------------------------------------------- distort --
//
// FL-020..023, the CSP Filter ▸ Distort family. All four are the SAME op with
// a different two lines in the middle: for every destination pixel, work out
// where its colour comes from and take one bilinear tap there — the INVERSE
// map, the idiom `liquify.rs` warps with. Forward-mapping instead (push each
// source pixel to where it lands) leaves holes wherever the map stretches, and
// no amount of splatting fixes that; the inverse map cannot leave a hole
// because it fills every destination exactly once.
//
// Sampling is premultiplied fix15, for the same reason the blur family
// averages there: a bilinear tap IS a weighted average, and averaging
// un-premultiplied colour drags a transparent neighbour's arbitrary colour
// into a soft edge.

/// Run one inverse map over `buf` in place. `inverse` answers, for a
/// destination pixel, the SOURCE coordinate its colour comes from.
fn warp(buf: &mut Raster, inverse: impl Fn(f32, f32) -> (f32, f32)) {
    let mut src = Raster::new(buf.w, buf.h);
    std::mem::swap(buf, &mut src);
    for y in 0..src.h {
        for x in 0..src.w {
            let (sx, sy) = inverse(x as f32, y as f32);
            let p = sample_bilinear(&src, sx, sy);
            let mut out = [0u16; TILE_CHANNELS];
            for (o, v) in out.iter_mut().zip(p) {
                *o = (v + 0.5).clamp(0.0, u16::MAX as f32) as u16;
            }
            buf.set_pixel(x, y, out);
        }
    }
}

/// Centre and working radius of a buffer — the frame the two radial warps
/// live in, as `(cx, cy, radius)`.
///
/// The radius is the INSCRIBED circle's, not the half-diagonal's, and that is
/// load-bearing: a map that never sends a sample outside the inscribed circle
/// never reads the buffer's transparent surround, which is why Pinch and Twirl
/// can honestly declare a [`super::Filter::reach`] of zero. Outside the circle both
/// warps are the identity, so the corners of a selection come through
/// untouched rather than smeared against the marquee.
///
/// The centre is the buffer's own — the selection's bounds centre on every
/// caller today, exactly as for radial and spin blur. A draggable centre
/// handle is the same missing interaction round for all four.
fn radial_frame(buf: &Raster) -> (f32, f32, f32) {
    let cx = (buf.w as f32 - 1.0) * 0.5;
    let cy = (buf.h as f32 - 1.0) * 0.5;
    (cx, cy, cx.min(cy).max(1.0))
}

/// The amplitude a sine warp is allowed, shared by [`super::Filter::reach`] and the
/// kernels so the halo and the taps cannot disagree.
pub(super) fn wave_amplitude(amplitude: f32) -> f32 {
    amplitude.clamp(-1024.0, 1024.0)
}

/// FL-020: `r_src = R·(r/R)^(1−a)`. For `a > 0` the exponent is below one, so
/// the source radius is the LARGER — each destination ring pulls in content
/// from further out and the picture contracts toward the centre, which is the
/// pinch. Negative `a` runs it the other way and is the bulge/fish-eye; that
/// is why there is no separate Fish-eye arm. The exponent stays positive, so
/// `r_src ≤ R` always and no tap leaves the inscribed circle.
pub(super) fn pinch(buf: &mut Raster, amount: f32) {
    let a = amount.clamp(-0.95, 0.95);
    let (cx, cy, rad) = radial_frame(buf);
    warp(buf, |x, y| {
        let (ux, uy) = (x - cx, y - cy);
        let r = ux.hypot(uy);
        if r <= 0.0 || r >= rad {
            return (x, y);
        }
        // (r_src / r), so the ray direction comes along for free.
        let k = (r / rad).powf(1.0 - a) * rad / r;
        (cx + ux * k, cy + uy * k)
    });
}

/// FL-021: the sample radius wobbles — `r_src = r + A·sin(2πr/λ)`. Purely
/// radial, so ink never leaves the ray it started on; the rings are what a
/// drop in water does to a reflection.
pub(super) fn ripple(buf: &mut Raster, amplitude: f32, wavelength: f32) {
    let amp = wave_amplitude(amplitude);
    let lam = wavelength.max(1.0);
    let (cx, cy, _) = radial_frame(buf);
    warp(buf, |x, y| {
        let (ux, uy) = (x - cx, y - cy);
        let r = ux.hypot(uy);
        if r <= 0.0 {
            return (x, y);
        }
        let rs = (r + amp * (std::f32::consts::TAU * r / lam).sin()).max(0.0);
        (cx + ux * rs / r, cy + uy * rs / r)
    });
}

/// FL-022: one sine shear. Horizontal slides each ROW sideways by
/// `A·sin(2πy/λ)`; vertical does the transpose. Nothing moves along the axis
/// the wave runs down, so straight lines parallel to it stay exactly as long
/// as they were.
pub(super) fn wave(buf: &mut Raster, amplitude: f32, wavelength: f32, dir: WaveDir) {
    let amp = wave_amplitude(amplitude);
    let phase = std::f32::consts::TAU / wavelength.max(1.0);
    warp(buf, |x, y| match dir {
        WaveDir::Horizontal => (x + amp * (phase * y).sin(), y),
        WaveDir::Vertical => (x, y + amp * (phase * x).sin()),
    });
}

/// FL-023: rotate about the centre by `angle_deg`, the turn falling linearly
/// to zero at the rim so the warp blends into the untouched surround instead
/// of tearing against it. Radius is preserved exactly, so — like pinch — no
/// tap escapes the inscribed circle.
pub(super) fn twirl(buf: &mut Raster, angle_deg: f32) {
    let a = angle_deg.clamp(-1440.0, 1440.0).to_radians();
    let (cx, cy, rad) = radial_frame(buf);
    warp(buf, |x, y| {
        let (ux, uy) = (x - cx, y - cy);
        let r = ux.hypot(uy);
        if r >= rad {
            return (x, y);
        }
        let th = uy.atan2(ux) - a * (1.0 - r / rad);
        (cx + r * th.cos(), cy + r * th.sin())
    });
}
