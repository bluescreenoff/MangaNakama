//! The blend formulas, in one place.
//!
//! These are the CPU half of a contract: `mn-gpu` implements the same three
//! modes with **fixed-function blend states**, and the `cpu_matches_gpu` test in
//! `mn-gpu` renders synthetic documents both ways and asserts they agree. If you
//! change a formula here, change the matching blend state in
//! `crates/gpu/src/lib.rs` (they are commented with these exact equations).
//!
//! # Colour model
//!
//! Everything is **premultiplied**: `c = C * a`, where `C` is the straight
//! colour. Tiles store premultiplied fix15 (`1.0 == 32768`); blending happens in
//! `f32` 0..1; only export quantises to 8-bit.
//!
//! Layer opacity is applied to the source *before* the blend, all four channels:
//! `s = s * opacity`. That keeps it premultiplied and makes it exactly what the
//! GPU fragment shader does (`raw / 32768 * opacity`), so no blend-constant
//! trickery is needed.
//!
//! # The three modes (premultiplied, `s` = source, `d` = destination)
//!
//! ```text
//! Normal (svg:src-over)
//!     out.rgb = s.rgb + d.rgb * (1 - s.a)
//!     out.a   = s.a   + d.a   * (1 - s.a)
//!
//! Multiply (svg:multiply)
//!     out.rgb = s.rgb * d.rgb + s.rgb * (1 - d.a) + d.rgb * (1 - s.a)
//!     out.a   = s.a + d.a * (1 - s.a)
//!
//! Screen (svg:screen)
//!     out.rgb = s.rgb + d.rgb - s.rgb * d.rgb
//!     out.a   = s.a + d.a * (1 - s.a)
//! ```
//!
//! These are the PDF/SVG separable blend equations written for premultiplied
//! input (`Co = as*(1-ab)*Cs + as*ab*B(Cb,Cs) + (1-as)*ab*Cb`).
//!
//! **Why the GPU can do Multiply with fixed function.** The general Multiply
//! above has three terms, and fixed-function blending only gives you two
//! (`src * srcFactor + dst * dstFactor`). It fits because the GPU always
//! composites onto an **opaque** canvas (cleared to paper white), i.e. `d.a == 1`
//! — which kills the `s.rgb * (1 - d.a)` term. Screen needs no such assumption:
//! its premultiplied form is already two-term. See the blend states in the gpu
//! crate for the factor-by-factor mapping.
//!
//! CPU export with a **transparent** background is the one case where `d.a < 1`,
//! and there the general formula above is what runs — correct, and unreachable
//! from the display path.
//!
//! # Part 3: the dodge/burn/light family, and where CSP is not Photoshop
//!
//! Twelve more modes composite through the same general frame. Nine are
//! separable (colour burn, linear burn, colour dodge, glow dodge, vivid
//! light, linear light, pin light, hard mix, divide); three are nonseparable
//! (darker colour, lighter colour, brightness). For nine of the twelve CSP
//! and Photoshop agree channel for channel and the W3C/PDF operator is the
//! implementation. The three that needed a call:
//!
//! - **Darker / Lighter color.** Photoshop compares the *sum* of the three
//!   channels; CSP's manual says **brightness**. We compare brightness, using
//!   the same `Lum()` the nonseparable trio already uses (0.3/0.59/0.11).
//!   The two orderings differ for colours of equal sum but different hue — a
//!   saturated blue against a mid grey is the visible case.
//! - **Brightness.** CSP's name for what Photoshop and SVG call Luminosity.
//!   Same operator, `SetLum(Cb, Lum(Cs))`; only the label differs, and the
//!   picker shows CSP's.
//! - **Glow dodge** (CSP's 覆い焼き（発光）) has no Photoshop counterpart and
//!   no published formula. What is certain is the behaviour that makes it
//!   worth having: it lifts pure black, which plain colour dodge cannot. Ours
//!   is `(Cb + Cs) / (1 - Cs)` — colour dodge with the blend colour *emitting*
//!   as well as dividing. It is the identity at `Cs = 0`, it is monotone, and
//!   it is everywhere ≥ colour dodge, which is what "stronger dodge" has to
//!   mean. **It is our shape, not a reproduction of CSP's**; if the owner
//!   A/Bs it against CSP and it reads wrong, this one function is the fix.
//!
//! And one mode deliberately NOT added. CSP lists **Add** and **Add (Glow)**;
//! our `Add` is already defined directly on premultiplied values, i.e.
//! `min(1, s.rgb + d.rgb)`, which for a translucent source is *stronger* than
//! the SVG-frame add the same way CSP's Add (Glow) is stronger than its Add —
//! and identical to it for an opaque one. So the mode we ship as "Add" is
//! behaviourally the glow one, and the row that is really missing is CSP's
//! plainer Add. Splitting them means re-specifying a shipped mode, changing
//! how the owner's existing files render, and moving Add off its
//! fixed-function fast path onto the shader; that is his call, not ours.

use crate::doc::Blend;

/// fix15 unity as a float: `1.0 == 32768.0`.
pub const FIX15_ONE_F: f32 = 32768.0;

/// Premultiplied RGBA in 0..1.
pub type Rgba = [f32; 4];

/// One fix15 channel to 0..1.
#[inline]
pub fn fix15_to_f32(v: u16) -> f32 {
    v as f32 / FIX15_ONE_F
}

/// 0..1 to fix15, rounded to nearest and clamped.
#[inline]
pub fn f32_to_fix15(v: f32) -> u16 {
    if !v.is_finite() {
        return 0;
    }
    (v.clamp(0.0, 1.0) * FIX15_ONE_F + 0.5).min(FIX15_ONE_F) as u16
}

/// One premultiplied fix15 pixel to premultiplied 0..1.
#[inline]
pub fn px_to_f32(px: [u16; 4]) -> Rgba {
    [
        fix15_to_f32(px[0]),
        fix15_to_f32(px[1]),
        fix15_to_f32(px[2]),
        fix15_to_f32(px[3]),
    ]
}

/// 0..1 to 8-bit, round-half-up. This is the exact rounding export uses; the
/// GPU's `Rgba8Unorm` write rounds to nearest too, hence the tiny epsilon in the
/// cross-check test rather than an exact match.
#[inline]
pub fn to_u8(v: f32) -> u8 {
    if !v.is_finite() {
        return 0;
    }
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Premultiplied 0..1 to **straight** (non-premultiplied) 8-bit RGBA, which is
/// what PNG stores. Fully transparent pixels become all-zero.
#[inline]
pub fn unpremultiply_u8(p: Rgba) -> [u8; 4] {
    if p[3] <= 0.0 {
        return [0, 0, 0, 0];
    }
    let inv = 1.0 / p[3];
    [
        to_u8(p[0] * inv),
        to_u8(p[1] * inv),
        to_u8(p[2] * inv),
        to_u8(p[3]),
    ]
}

/// Straight 8-bit RGBA (PNG) to premultiplied fix15 (tile storage).
///
/// The inverse of [`unpremultiply_u8`] up to quantisation — see the round-trip
/// note in `core::ora`.
#[inline]
pub fn straight_u8_to_fix15(px: [u8; 4]) -> [u16; 4] {
    let a = ((px[3] as u32 * FIX15_ONE_F as u32 + 127) / 255) as u16;
    let ch = |c: u8| -> u16 {
        // round(c/255 * a)
        ((c as u32 * a as u32 + 127) / 255) as u16
    };
    [ch(px[0]), ch(px[1]), ch(px[2]), a]
}

/// W3C/PDF colour-burn on straight channels. Shared with Vivid light, which
/// is this operator at double strength below the midpoint.
///
/// The two guards are the spec's, in the spec's order: a white base can never
/// be burnt, and a black blend channel burns to black rather than dividing by
/// zero.
#[inline]
fn color_burn(cs: f32, cb: f32) -> f32 {
    if cb >= 1.0 {
        1.0
    } else if cs <= 0.0 {
        0.0
    } else {
        1.0 - (1.0 - cb).min(cs) / cs
    }
}

/// W3C/PDF colour-dodge on straight channels. Shared with Vivid light above
/// the midpoint. Black bases stay black — that is the mode's defining
/// property, and the reason CSP also ships Glow dodge.
#[inline]
fn color_dodge(cs: f32, cb: f32) -> f32 {
    if cb <= 0.0 {
        0.0
    } else if cs >= 1.0 {
        1.0
    } else {
        // min-then-divide, not divide-then-min: it lands on exactly 1.0 at
        // the clamp instead of 1.0 ± an ulp, which is what lets the WGSL
        // twin of this line agree bit for bit.
        cb.min(1.0 - cs) / (1.0 - cs)
    }
}

/// Blend one premultiplied source pixel over one premultiplied destination.
///
/// `src` must already have layer opacity folded in. See the module docs for
/// the equations; they are mirrored in the GPU blend states.
#[inline]
pub fn blend_premul(mode: Blend, src: Rgba, dst: Rgba) -> Rgba {
    let (sa, da) = (src[3], dst[3]);
    let out_a = sa + da * (1.0 - sa);
    let mut out = [0.0f32; 4];
    match mode {
        Blend::Normal => {
            for i in 0..3 {
                out[i] = src[i] + dst[i] * (1.0 - sa);
            }
        }
        Blend::Multiply => {
            for i in 0..3 {
                out[i] = src[i] * dst[i] + src[i] * (1.0 - da) + dst[i] * (1.0 - sa);
            }
        }
        Blend::Screen => {
            for i in 0..3 {
                out[i] = src[i] + dst[i] - src[i] * dst[i];
            }
        }
        // Darken / Lighten: the straight-colour min/max through the general
        // premultiplied form
        //   out.rgb = s.rgb*(1-da) + sa*da*B(cs,cb) + d.rgb*(1-sa)
        // On the opaque canvas (d.a == 1) this collapses EXACTLY to the GPU's
        // Min/Max states (see the proof next to them); min/max have no
        // saturation stage, so translucent sources agree too.
        Blend::Darken | Blend::Lighten => {
            let pick = |s: f32, d: f32| {
                if mode == Blend::Darken {
                    s.min(d)
                } else {
                    s.max(d)
                }
            };
            for i in 0..3 {
                // The B term only lives when both alphas are non-zero; the
                // corner cases (transparent source or dest) zero sa*da, and
                // the remaining terms are exact without dividing by zero.
                let b = if sa > 0.0 && da > 0.0 {
                    pick(src[i] / sa, dst[i] / da)
                } else {
                    0.0
                };
                out[i] = src[i] * (1.0 - da) + sa * da * b + dst[i] * (1.0 - sa);
            }
        }
        // Add is OUR operator (mn:add): defined directly on premultiplied
        // values, which is exactly what the GPU state computes, so CPU and
        // GPU agree at every alpha — the straight-colour SVG form would
        // diverge on translucent bright sources (it lerps toward the
        // operator, the GPU saturates it). For opaque sources both
        // definitions coincide, and over a transparent destination
        // min(src + 0, 1) = src, so the mode keeps the source like every
        // other one.
        Blend::Add => {
            for i in 0..3 {
                out[i] = (src[i] + dst[i]).min(1.0);
            }
        }
        // CSP's Subtract: base minus blend, floored at zero — but through
        // the general premultiplied frame, NOT the old premultiplied
        // max(d - s, 0). The premultiplied form yielded rgb = 0 with
        // out_a = sa over a transparent destination: an artist floating a
        // Subtract layer inside a transparent folder got silent black ink.
        // The general frame keeps the source there (the B term needs
        // d.a > 0), collapses to max(d - s, 0) on the opaque canvas, and is
        // what the blend2 shader computes — Subtract left the fixed-function
        // ReverseSubtract state when this changed.
        Blend::Subtract => {
            for i in 0..3 {
                let b = if sa > 0.0 && da > 0.0 {
                    (dst[i] / da - src[i] / sa).max(0.0)
                } else {
                    0.0
                };
                out[i] = src[i] * (1.0 - da) + sa * da * b + dst[i] * (1.0 - sa);
            }
        }
        // The part-2 and part-3 separable modes: the straight-colour operator
        // B(cs, cb) through the SAME general premultiplied frame Darken uses
        //   out.rgb = s.rgb*(1-da) + sa*da*B(cs,cb) + d.rgb*(1-sa)
        // which reduces to 1*1*B(cs,cb) on the opaque canvas.
        //
        // Every part-3 operator is written to be TOTAL: the degenerate inputs
        // (a zero denominator, a saturated quotient) are branched on, never
        // divided through, so neither path can produce an inf or a NaN. That
        // is not tidiness — WGSL leaves division by zero implementation
        // defined, so a guard the CPU skips is a guard the two paths would
        // disagree about on somebody else's GPU.
        Blend::Overlay
        | Blend::SoftLight
        | Blend::HardLight
        | Blend::Difference
        | Blend::Exclusion
        | Blend::ColorBurn
        | Blend::LinearBurn
        | Blend::ColorDodge
        | Blend::GlowDodge
        | Blend::VividLight
        | Blend::LinearLight
        | Blend::PinLight
        | Blend::HardMix
        | Blend::Divide => {
            let hard_light = |cs: f32, cb: f32| {
                if cs <= 0.5 {
                    2.0 * cs * cb
                } else {
                    1.0 - 2.0 * (1.0 - cs) * (1.0 - cb)
                }
            };
            let op = |cs: f32, cb: f32| match mode {
                // Overlay is HardLight with the operands swapped.
                Blend::Overlay => hard_light(cb, cs),
                Blend::HardLight => hard_light(cs, cb),
                // W3C soft-light (the pegged spec form, with the rational
                // shadow/midpiece d(x)).
                Blend::SoftLight => {
                    let d = |x: f32| {
                        if x <= 0.25 {
                            ((16.0 * x - 12.0) * x + 4.0) * x
                        } else {
                            x.sqrt()
                        }
                    };
                    if cs <= 0.5 {
                        cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb)
                    } else {
                        cb + (2.0 * cs - 1.0) * (d(cb) - cb)
                    }
                }
                Blend::Difference => (cb - cs).abs(),
                // Exclusion: cb + cs - 2*cb*cs (the invertible screen twin).
                Blend::Exclusion => cb + cs - 2.0 * cb * cs,
                // --- part 3: the burn/dodge/light family ------------------
                Blend::ColorBurn => color_burn(cs, cb),
                // Linear burn: darkens by the blend colour with no contrast
                // lift — the plain sum, floored (CSP BM-005).
                Blend::LinearBurn => (cb + cs - 1.0).clamp(0.0, 1.0),
                Blend::ColorDodge => color_dodge(cs, cb),
                // Glow dodge (CSP's 覆い焼き（発光）, BM-010): colour dodge
                // that also EMITS the blend colour, so it lifts pure black —
                // which plain colour dodge cannot (0/(1-cs) is 0 forever) and
                // which is the entire reason an artist reaches for it. See
                // the deviation note in the module docs: the shape is ours.
                Blend::GlowDodge => {
                    if cs >= 1.0 {
                        1.0
                    } else {
                        (cb + cs).min(1.0 - cs) / (1.0 - cs)
                    }
                }
                // Vivid light: colour burn below the midpoint, colour dodge
                // above, both driven at double strength (CSP BM-017).
                Blend::VividLight => {
                    if cs <= 0.5 {
                        color_burn(2.0 * cs, cb)
                    } else {
                        color_dodge(2.0 * cs - 1.0, cb)
                    }
                }
                // Linear light: linear burn below, linear dodge above — the
                // same double-strength split without the contrast lift.
                Blend::LinearLight => (cb + 2.0 * cs - 1.0).clamp(0.0, 1.0),
                // Pin light: Darken below the midpoint, Lighten above; the
                // base is REPLACED wherever it falls outside the window,
                // which is why it shreds gradients and is used on flats.
                Blend::PinLight => {
                    if cs <= 0.5 {
                        cb.min(2.0 * cs)
                    } else {
                        cb.max(2.0 * cs - 1.0)
                    }
                }
                // Hard mix, exactly as the ledger row states it: the two
                // channel values are summed and the result clamps to 0 or 1.
                // A step function — see the test note about its threshold.
                Blend::HardMix => {
                    if cb + cs >= 1.0 {
                        1.0
                    } else {
                        0.0
                    }
                }
                // Divide: base / blend, brightening; a zero blend channel is
                // an infinite quotient, which saturates to white.
                Blend::Divide => {
                    if cs <= 0.0 {
                        1.0
                    } else {
                        cb.min(cs) / cs
                    }
                }
                _ => unreachable!(),
            };
            for i in 0..3 {
                let b = if sa > 0.0 && da > 0.0 {
                    op(src[i] / sa, dst[i] / da)
                } else {
                    0.0
                };
                out[i] = src[i] * (1.0 - da) + sa * da * b + dst[i] * (1.0 - sa);
            }
        }
        // The nonseparable modes (W3C/PDF part 2 + part 3): the blend is a
        // mix of the sources' LUMINANCE and SATURATION, not per-channel, so
        // the whole RGB triple goes through one operator.
        Blend::Hue
        | Blend::Saturation
        | Blend::Color
        | Blend::DarkerColor
        | Blend::LighterColor
        | Blend::Luminosity => {
            // Both colours straight; degenerate alphas contribute nothing.
            let straight = |p: Rgba| -> [f32; 3] {
                let a = p[3];
                if a > 0.0 {
                    [p[0] / a, p[1] / a, p[2] / a]
                } else {
                    [0.0; 3]
                }
            };
            let cs = straight(src);
            let cb = straight(dst);
            // W3C nonseparable helpers (compositing spec, rounded 2-decimal
            // luma coefficients).
            let lum = |c: [f32; 3]| 0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2];
            let sat = |c: [f32; 3]| c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2]);
            let clip_color = |mut c: [f32; 3]| -> [f32; 3] {
                let l = lum(c);
                let n = c[0].min(c[1]).min(c[2]);
                let x = c[0].max(c[1]).max(c[2]);
                if n < 0.0 {
                    let d = l - n;
                    if d != 0.0 {
                        for v in c.iter_mut() {
                            *v = l + (*v - l) * l / d;
                        }
                    }
                }
                if x > 1.0 {
                    let d = x - l;
                    if d != 0.0 {
                        for v in c.iter_mut() {
                            *v = l + (*v - l) * (1.0 - l) / d;
                        }
                    }
                }
                c
            };
            let set_lum = |c: [f32; 3], l: f32| {
                clip_color([c[0] + l - lum(c), c[1] + l - lum(c), c[2] + l - lum(c)])
            };
            // The spec's SetSat, by channel POSITION (min/mid/max channels):
            //   Cmax > Cmin ⇒ Cmid' = (Cmid-Cmin)*s/(Cmax-Cmin), Cmin'=0,
            //   Cmax'=s; else all zero.
            let set_sat = |c: [f32; 3], s: f32| -> [f32; 3] {
                let mut out3 = c;
                let imin = (0..3).min_by(|&a, &b| c[a].total_cmp(&c[b])).unwrap();
                let imax = (0..3).max_by(|&a, &b| c[a].total_cmp(&c[b])).unwrap();
                let imid = 3 - imin - imax;
                if c[imax] > c[imin] {
                    out3[imid] = (c[imid] - c[imin]) * s / (c[imax] - c[imin]);
                    out3[imin] = 0.0;
                    out3[imax] = s;
                } else {
                    out3 = [0.0; 3];
                }
                out3
            };
            let blended: [f32; 3] = match mode {
                Blend::Hue => set_lum(set_sat(cs, sat(cb)), lum(cb)),
                Blend::Saturation => set_lum(set_sat(cb, sat(cs)), lum(cb)),
                Blend::Color => set_lum(cs, lum(cb)),
                // Darker / Lighter color (CSP BM-022/023): compare the two
                // colours' BRIGHTNESS and keep the whole winning colour —
                // unlike Darken/Lighten, which compare per channel and can
                // therefore emit a colour that is in neither layer. Ties go
                // to the source, on both paths, so the step lands in the
                // same place.
                Blend::DarkerColor => {
                    if lum(cs) <= lum(cb) {
                        cs
                    } else {
                        cb
                    }
                }
                Blend::LighterColor => {
                    if lum(cs) >= lum(cb) {
                        cs
                    } else {
                        cb
                    }
                }
                // CSP's Brightness = SVG's Luminosity: the base's hue and
                // saturation carried to the blend colour's brightness. The
                // exact inverse of Color, which is the pair CSP ships.
                Blend::Luminosity => set_lum(cb, lum(cs)),
                _ => unreachable!(),
            };
            for i in 0..3 {
                out[i] = src[i] * (1.0 - da) + sa * da * blended[i] + dst[i] * (1.0 - sa);
            }
        }
    }
    out[3] = out_a;
    out
}

/// Scale a premultiplied pixel by a layer opacity (all four channels).
#[inline]
pub fn scale_opacity(p: Rgba, opacity: f32) -> Rgba {
    if opacity >= 1.0 {
        return p;
    }
    let o = opacity.clamp(0.0, 1.0);
    [p[0] * o, p[1] * o, p[2] * o, p[3] * o]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    fn approx(a: Rgba, b: Rgba) -> bool {
        (0..4).all(|i| close(a[i], b[i]))
    }

    #[test]
    fn normal_over_opaque_white_is_hand_computable() {
        // 50%-alpha pure red (premultiplied: 0.5, 0, 0, 0.5) over opaque white.
        // out.rgb = s + d*(1-0.5) = 0.5 + 0.5 = 1.0 for red; 0 + 0.5 for g/b.
        let out = blend_premul(Blend::Normal, [0.5, 0.0, 0.0, 0.5], [1.0, 1.0, 1.0, 1.0]);
        assert!(approx(out, [1.0, 0.5, 0.5, 1.0]), "{out:?}");
    }

    #[test]
    fn multiply_over_opaque_is_hand_computable() {
        // Opaque mid-grey source over opaque white: 0.5*1 + 0.5*0 + 1*0 = 0.5.
        let out = blend_premul(Blend::Multiply, [0.5, 0.5, 0.5, 1.0], [1.0, 1.0, 1.0, 1.0]);
        assert!(approx(out, [0.5, 0.5, 0.5, 1.0]), "{out:?}");

        // Opaque red over opaque green: 1*0 = 0 everywhere -> black.
        let out = blend_premul(Blend::Multiply, [1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0]);
        assert!(approx(out, [0.0, 0.0, 0.0, 1.0]), "{out:?}");

        // Half-alpha black (premul 0,0,0,0.5) over opaque white:
        // 0*1 + 0*0 + 1*(1-0.5) = 0.5.
        let out = blend_premul(Blend::Multiply, [0.0, 0.0, 0.0, 0.5], [1.0, 1.0, 1.0, 1.0]);
        assert!(approx(out, [0.5, 0.5, 0.5, 1.0]), "{out:?}");
    }

    #[test]
    fn screen_is_hand_computable() {
        // Opaque mid-grey over opaque mid-grey: 0.5 + 0.5 - 0.25 = 0.75.
        let out = blend_premul(Blend::Screen, [0.5, 0.5, 0.5, 1.0], [0.5, 0.5, 0.5, 1.0]);
        assert!(approx(out, [0.75, 0.75, 0.75, 1.0]), "{out:?}");
        // Anything screened over white stays white.
        let out = blend_premul(Blend::Screen, [0.3, 0.2, 0.1, 1.0], [1.0, 1.0, 1.0, 1.0]);
        assert!(approx(out, [1.0, 1.0, 1.0, 1.0]), "{out:?}");
    }

    #[test]
    fn transparent_source_is_a_no_op_in_every_mode() {
        let dst = [0.25, 0.5, 0.75, 1.0];
        for m in Blend::ALL {
            let out = blend_premul(m, [0.0, 0.0, 0.0, 0.0], dst);
            assert!(approx(out, dst), "{m:?} -> {out:?}");
        }
    }

    /// The other degenerate corner: a source over NOTHING keeps the source
    /// pixel unchanged in every mode (the B term needs `d.a > 0`). This is
    /// the arm a transparent-background export takes, and it is where a
    /// divide-by-zero in a new operator would surface as a NaN.
    #[test]
    fn transparent_destination_keeps_the_source_in_every_mode() {
        let src = [0.2, 0.8, 0.5, 0.5];
        for m in Blend::ALL {
            let out = blend_premul(m, src, [0.0, 0.0, 0.0, 0.0]);
            assert!(approx(out, src), "{m:?} -> {out:?}");
            assert!(out.iter().all(|v| v.is_finite()), "{m:?} -> {out:?}");
        }
    }

    /// The round-27 modes on the OPAQUE canvas (the GPU's `d.a == 1` world):
    /// the general formula must collapse to exactly what the fixed-function
    /// states compute — this is the CPU half of the cpu_matches_gpu pin.
    #[test]
    fn round27_modes_reduce_to_the_gpu_equations_on_opaque() {
        let (s, d) = ([0.5f32, 0.2, 0.9, 0.75], [0.9f32, 0.2, 0.1, 1.0]);
        let (sa, dr) = (s[3], d);
        // Darken: min(d.rgb, s.rgb + d.rgb*(1-s.a))
        let want = |i: usize, f: fn(f32, f32) -> f32| f(dr[i], s[i] + dr[i] * (1.0 - sa));
        let out = blend_premul(Blend::Darken, s, d);
        for i in 0..3 {
            assert!(
                close(out[i], want(i, f32::min)),
                "darken ch{i}: {} vs {}",
                out[i],
                want(i, f32::min)
            );
        }
        let out = blend_premul(Blend::Lighten, s, d);
        for i in 0..3 {
            assert!(close(out[i], want(i, f32::max)), "lighten ch{i}");
        }
        let out = blend_premul(Blend::Add, s, d);
        for i in 0..3 {
            assert!(close(out[i], (s[i] + d[i]).min(1.0)), "add ch{i}");
        }
        // Subtract rides the general frame now (no fixed-function state):
        // on the opaque canvas out = sa*max(cb - cs, 0) + d*(1-sa).
        let out = blend_premul(Blend::Subtract, s, d);
        for i in 0..3 {
            let want = sa * (dr[i] - s[i] / sa).max(0.0) + dr[i] * (1.0 - sa);
            assert!(close(out[i], want), "subtract ch{i}: {} vs {want}", out[i]);
        }
    }

    #[test]
    fn round27_modes_are_hand_computable() {
        // Opaque dark red over opaque light: darken keeps the darker channel,
        // lighten the lighter, add saturates, subtract floors at zero.
        let (s, d) = ([0.2, 0.8, 0.5, 1.0], [0.6, 0.4, 0.5, 1.0]);
        let out = blend_premul(Blend::Darken, s, d);
        assert!(approx(out, [0.2, 0.4, 0.5, 1.0]), "{out:?}");
        let out = blend_premul(Blend::Lighten, s, d);
        assert!(approx(out, [0.6, 0.8, 0.5, 1.0]), "{out:?}");
        let out = blend_premul(Blend::Add, s, d);
        assert!(approx(out, [0.8, 1.0, 1.0, 1.0]), "{out:?}");
        let out = blend_premul(Blend::Subtract, s, d);
        assert!(approx(out, [0.4, 0.0, 0.0, 1.0]), "{out:?}");
        // Over a transparent destination every mode keeps the premultiplied
        // source pixel, exactly like Normal (the B term needs d.a > 0).
        let out = blend_premul(Blend::Darken, [0.2, 0.8, 0.5, 0.5], [0.0, 0.0, 0.0, 0.0]);
        assert!(approx(out, [0.2, 0.8, 0.5, 0.5]), "{out:?}");
        // And the premultiplied Add saturates, never exceeds 1.
        let out = blend_premul(Blend::Add, [0.8, 0.8, 0.8, 0.5], [0.7, 0.7, 0.7, 1.0]);
        assert!(approx(out, [1.0, 1.0, 1.0, 1.0]), "{out:?}");
    }

    /// The part-2 separable modes, opaque canvas (the GPU's world): the
    /// general premultiplied frame must reduce to the straight operator.
    #[test]
    fn part2_separable_reduce_to_the_straight_ops_on_opaque() {
        let (s, d) = ([0.6f32, 0.2, 0.9, 1.0], [0.3f32, 0.7, 0.4, 1.0]);
        // Plain fns so the table can hold fn pointers (closures capturing
        // `hl` would not coerce).
        fn hl(cs: f32, cb: f32) -> f32 {
            if cs <= 0.5 {
                2.0 * cs * cb
            } else {
                1.0 - 2.0 * (1.0 - cs) * (1.0 - cb)
            }
        }
        fn overlay(cs: f32, cb: f32) -> f32 {
            hl(cb, cs)
        }
        fn diff(cb: f32, cs: f32) -> f32 {
            (cb - cs).abs()
        }
        fn excl(cb: f32, cs: f32) -> f32 {
            cb + cs - 2.0 * cb * cs
        }
        fn soft(cs: f32, cb: f32) -> f32 {
            let dd = |x: f32| {
                if x <= 0.25 {
                    ((16.0 * x + 12.0) * x + 4.0) * x
                } else {
                    x.sqrt()
                }
            };
            if cs <= 0.5 {
                cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb)
            } else {
                cb + (2.0 * cs - 1.0) * (dd(cb) - cb)
            }
        }
        let cases: [(Blend, fn(f32, f32) -> f32); 5] = [
            (Blend::Overlay, overlay),
            (Blend::HardLight, hl),
            (Blend::Difference, diff),
            (Blend::Exclusion, excl),
            (Blend::SoftLight, soft),
        ];
        for (mode, op) in cases {
            let out = blend_premul(mode, s, d);
            assert!(close(out[3], 1.0), "{mode:?} alpha");
            for i in 0..3 {
                let want = op(s[i], d[i]);
                assert!(close(out[i], want), "{mode:?} ch{i}: {} vs {want}", out[i]);
            }
        }
    }

    #[test]
    fn part2_separable_are_hand_computable() {
        // Overlay: opaque mid-grey over opaque white — HardLight(1, .5):
        // cb=1 > .5 ⇒ 1-2*(0)*(1-1)... via the swap: hl(1.0, 0.5) with
        // cs=1: 1-2*(0)*(0.5) = 1. White dominates.
        let out = blend_premul(Blend::Overlay, [0.5, 0.5, 0.5, 1.0], [1.0, 1.0, 1.0, 1.0]);
        assert!(approx(out, [1.0, 1.0, 1.0, 1.0]), "{out:?}");
        // Same over black: hl(0, .5): cs=0 ≤ .5 ⇒ 0. Black dominates.
        let out = blend_premul(Blend::Overlay, [0.5, 0.5, 0.5, 1.0], [0.0, 0.0, 0.0, 1.0]);
        assert!(approx(out, [0.0, 0.0, 0.0, 1.0]), "{out:?}");
        // Difference: red over green = |0-1|,|1-0| = yellow.
        let out = blend_premul(
            Blend::Difference,
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 1.0],
        );
        assert!(approx(out, [1.0, 1.0, 0.0, 1.0]), "{out:?}");
        // Exclusion with either source white INVERTS the other.
        let out = blend_premul(Blend::Exclusion, [1.0, 1.0, 1.0, 1.0], [0.2, 0.5, 0.8, 1.0]);
        assert!(approx(out, [0.8, 0.5, 0.2, 1.0]), "{out:?}");
        // Half-alpha source over opaque white: the frame keeps
        // d.rgb*(1-sa) + sa*B.
        let out = blend_premul(
            Blend::Difference,
            [0.5, 0.0, 0.0, 0.5],
            [1.0, 1.0, 1.0, 1.0],
        );
        // B = |1-1| = 0 for r, |1-0| = 1 for g/b:
        // r: 0*0 + .5*0 + 1*.5 = .5; g: 0 + .5*1 + .5 = 1; b likewise 1.
        assert!(approx(out, [0.5, 1.0, 1.0, 1.0]), "{out:?}");
    }

    /// The nonseparable trio: hand-computed W3C examples on the opaque
    /// canvas, plus the alpha-frame behavior off it.
    #[test]
    fn part2_nonseparable_are_hand_computable() {
        // Color: takes the source hue+chroma, keeps dest luminance.
        // Opaque red (lum .3) over opaque mid-grey (lum .5):
        // setlum([1,0,0], .5) = [1.2, .2, .2], clip high side
        // (1-l)/(x-l) = .5/.7 ⇒ [1.0, .2857, .2857].
        let out = blend_premul(Blend::Color, [1.0, 0.0, 0.0, 1.0], [0.5, 0.5, 0.5, 1.0]);
        assert!(
            close(out[0], 1.0) && close(out[1], 0.2857143) && close(out[2], 0.2857143),
            "{out:?}"
        );
        // W3C Saturation = SetLum(SetSat(Cb, Sat(Cs)), Lum(Cb)): the DEST's
        // hue+luminance under the SOURCE's saturation. A grey source (sat 0)
        // desaturates the dest to its luminance: lum([.8,.2,.2]) = .38.
        let out = blend_premul(
            Blend::Saturation,
            [0.5, 0.5, 0.5, 1.0],
            [0.8, 0.2, 0.2, 1.0],
        );
        assert!(
            close(out[0], 0.38) && close(out[1], 0.38) && close(out[2], 0.38),
            "{out:?}"
        );
        // Hue takes the source hue at the dest sat+lum: opaque blue-ish hue
        // over the grey keeps the grey's luminance.
        let out = blend_premul(Blend::Hue, [0.0, 0.0, 1.0, 1.0], [0.5, 0.5, 0.5, 1.0]);
        assert!(
            close(0.3 * out[0] + 0.59 * out[1] + 0.11 * out[2], 0.5),
            "{out:?}"
        );
        // Transparent source stays a no-op in every part-2 mode.
        let dst = [0.25, 0.5, 0.75, 1.0];
        for m in [
            Blend::Overlay,
            Blend::SoftLight,
            Blend::HardLight,
            Blend::Difference,
            Blend::Exclusion,
            Blend::Hue,
            Blend::Saturation,
            Blend::Color,
        ] {
            let out = blend_premul(m, [0.0, 0.0, 0.0, 0.0], dst);
            assert!(approx(out, dst), "{m:?} -> {out:?}");
        }
    }

    /// The part-3 separable family, asserted by the identities that DEFINE
    /// each mode rather than by a table of numbers I would have to get right
    /// twice. A grid sweep so every branch boundary (0, ½, 1) is crossed on
    /// both operands.
    #[test]
    fn part3_separable_hold_their_defining_identities() {
        let grid = [0.0f32, 0.1, 0.25, 0.4, 0.5, 0.6, 0.75, 0.9, 1.0];
        // Grey source over grey dest, both OPAQUE (alpha 1, so premultiplied
        // == straight): the general frame reduces to B(cs, cb), and one
        // channel is the whole story.
        let b = |m: Blend, cs: f32, cb: f32| {
            blend_premul(m, [cs, cs, cs, 1.0], [cb, cb, cb, 1.0])[0]
        };
        // The mirror identity below inverts both operands, and `1 - (1 - x)`
        // is not `x` in f32; the slack absorbs that, not a formula error.
        let close5 = |a: f32, b: f32| (a - b).abs() < 1e-5;
        for &cs in &grid {
            for &cb in &grid {
                for m in Blend::ALL {
                    let v = b(m, cs, cb);
                    assert!(
                        v.is_finite() && (-1e-6..=1.0 + 1e-6).contains(&v),
                        "{m:?}({cs}, {cb}) left the gamut: {v}"
                    );
                }
                // Burn and dodge are each other's mirror under inversion —
                // the PDF definition's own symmetry, and the cheapest way to
                // catch a transposed operand in either one.
                let burn = b(Blend::ColorBurn, cs, cb);
                let mirror = 1.0 - b(Blend::ColorDodge, 1.0 - cs, 1.0 - cb);
                assert!(close5(burn, mirror), "burn({cs},{cb}) {burn} vs {mirror}");
                // Vivid light IS burn below the midpoint, dodge above, each
                // driven at double strength.
                let vivid = b(Blend::VividLight, cs, cb);
                let want = if cs <= 0.5 {
                    b(Blend::ColorBurn, 2.0 * cs, cb)
                } else {
                    b(Blend::ColorDodge, 2.0 * cs - 1.0, cb)
                };
                assert!(close(vivid, want), "vivid({cs},{cb}) {vivid} vs {want}");
                // Linear light is the same split without the contrast lift:
                // below the midpoint it is exactly linear burn at 2cs.
                if cs <= 0.5 {
                    let ll = b(Blend::LinearLight, cs, cb);
                    let lb = b(Blend::LinearBurn, 2.0 * cs, cb);
                    assert!(close(ll, lb), "linear light({cs},{cb}) {ll} vs {lb}");
                }
                // Pin light is Darken below and Lighten above, same split.
                let pin = b(Blend::PinLight, cs, cb);
                let want = if cs <= 0.5 {
                    b(Blend::Darken, 2.0 * cs, cb)
                } else {
                    b(Blend::Lighten, 2.0 * cs - 1.0, cb)
                };
                assert!(close(pin, want), "pin({cs},{cb}) {pin} vs {want}");
                // Hard mix posterises: only ever 0 or 1, and 1 exactly when
                // the two channels sum to full (the CSP-SURFACE row's words).
                let hm = b(Blend::HardMix, cs, cb);
                assert!(hm == 0.0 || hm == 1.0, "hard mix({cs},{cb}) = {hm}");
                assert_eq!(hm == 1.0, cs + cb >= 1.0, "hard mix({cs},{cb})");
                // Glow dodge is never weaker than colour dodge — that is the
                // whole claim the mode makes.
                let (glow, dodge) = (b(Blend::GlowDodge, cs, cb), b(Blend::ColorDodge, cs, cb));
                assert!(glow >= dodge - 1e-6, "glow({cs},{cb}) {glow} < {dodge}");
            }
        }
    }

    #[test]
    fn part3_separable_are_hand_computable() {
        let one = |m: Blend, cs: f32, cb: f32| {
            blend_premul(m, [cs, cs, cs, 1.0], [cb, cb, cb, 1.0])[0]
        };
        // Colour burn cannot darken a white base, and burns to black under a
        // black blend colour.
        assert!(close(one(Blend::ColorBurn, 0.0, 1.0), 1.0));
        assert!(close(one(Blend::ColorBurn, 0.0, 0.5), 0.0));
        // Half over half: 1 - (1-.5)/.5 = 0.
        assert!(close(one(Blend::ColorBurn, 0.5, 0.5), 0.0));
        // Linear burn is the plain sum, floored: .5 + .75 - 1 = .25.
        assert!(close(one(Blend::LinearBurn, 0.75, 0.5), 0.25));
        assert!(close(one(Blend::LinearBurn, 0.25, 0.5), 0.0));
        // Colour dodge: .5/(1-.25) = 2/3, and a black base stays black no
        // matter how bright the blend colour is. THAT is why Glow dodge
        // exists — same inputs, and it lifts.
        assert!(close(one(Blend::ColorDodge, 0.25, 0.5), 2.0 / 3.0));
        assert!(close(one(Blend::ColorDodge, 0.9, 0.0), 0.0));
        assert!(one(Blend::GlowDodge, 0.9, 0.0) > 0.99, "glow lifts black");
        assert!(close(one(Blend::GlowDodge, 0.0, 0.4), 0.4), "identity at cs=0");
        // Divide: base over an equal blend is white; a black blend channel
        // is an infinite quotient, which saturates rather than exploding.
        assert!(close(one(Blend::Divide, 0.5, 0.5), 1.0));
        assert!(close(one(Blend::Divide, 0.0, 0.3), 1.0));
        assert!(close(one(Blend::Divide, 0.8, 0.4), 0.5));
        // Pin light REPLACES: a dark blend colour pulls a light base down to
        // 2cs, a light one pushes a dark base up to 2cs-1.
        assert!(close(one(Blend::PinLight, 0.25, 0.9), 0.5));
        assert!(close(one(Blend::PinLight, 0.75, 0.1), 0.5));
    }

    #[test]
    fn part3_nonseparable_are_hand_computable() {
        // Darker / Lighter color keep the WHOLE winning colour — unlike
        // Darken/Lighten, which mix channels from both. Source lum
        // .3*.8+.59*.2+.11*.2 = .38; dest lum .3*.2+.59*.7+.11*.2 = .494.
        let (s, d) = ([0.8, 0.2, 0.2, 1.0], [0.2, 0.7, 0.2, 1.0]);
        let out = blend_premul(Blend::DarkerColor, s, d);
        assert!(approx(out, s), "darker color takes the source whole: {out:?}");
        let out = blend_premul(Blend::LighterColor, s, d);
        assert!(approx(out, d), "lighter color takes the dest whole: {out:?}");
        // The contrast with per-channel Darken, which emits a colour that is
        // in NEITHER layer — this is the pair the two modes exist to
        // distinguish, and the row a CSP user reads the manual for.
        let out = blend_premul(Blend::Darken, s, d);
        assert!(approx(out, [0.2, 0.2, 0.2, 1.0]), "{out:?}");
        // Brightness (SVG luminosity) is the exact inverse of Color: the
        // BASE's hue and saturation carried to the SOURCE's brightness.
        // Slack of 1e-5: Lum() is recomputed here from the blended triple,
        // so three more roundings sit between the two sides.
        let lum = |c: [f32; 4]| 0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2];
        let close5 = |a: f32, b: f32| (a - b).abs() < 1e-5;
        let out = blend_premul(Blend::Luminosity, s, d);
        assert!(close5(lum(out), lum(s)), "takes the source's lum: {out:?}");
        let out = blend_premul(Blend::Color, s, d);
        assert!(close5(lum(out), lum(d)), "Color takes the dest's: {out:?}");
        // On greyscale there is no hue to carry, so Brightness is just the
        // source — the sanity check that the operands are not swapped.
        let out = blend_premul(Blend::Luminosity, [0.25, 0.25, 0.25, 1.0], [0.75, 0.75, 0.75, 1.0]);
        assert!(approx(out, [0.25, 0.25, 0.25, 1.0]), "{out:?}");
    }

    /// Layer opacity is folded into the source BEFORE the blend. At 0% every
    /// mode must vanish; at 50% no mode may produce a NaN, an infinity or an
    /// out-of-gamut channel — the translucent case is exactly where a wrong
    /// premultiplied guard hides, because nobody looks there.
    #[test]
    fn part3_modes_survive_a_translucent_source() {
        let (s, d) = ([0.6, 0.3, 0.9, 1.0], [0.4, 0.7, 0.2, 1.0]);
        for m in Blend::ALL {
            let none = blend_premul(m, scale_opacity(s, 0.0), d);
            assert!(approx(none, d), "{m:?} at 0% changed the dest: {none:?}");
            for o in [0.01f32, 0.25, 0.5, 0.99] {
                let out = blend_premul(m, scale_opacity(s, o), d);
                for v in out {
                    assert!(
                        v.is_finite() && (-1e-6..=1.0 + 1e-6).contains(&v),
                        "{m:?} at {o}: {out:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn opacity_scales_premultiplied_channels() {
        let out = scale_opacity([1.0, 0.5, 0.0, 1.0], 0.5);
        assert!(approx(out, [0.5, 0.25, 0.0, 0.5]));
        // Opacity 0.5 on opaque black over white = 50% grey.
        let out = blend_premul(Blend::Normal, out, [1.0, 1.0, 1.0, 1.0]);
        assert!(close(out[3], 1.0));
    }

    #[test]
    fn fix15_and_8bit_conversions() {
        assert_eq!(f32_to_fix15(1.0), 32768);
        assert_eq!(f32_to_fix15(0.0), 0);
        assert_eq!(fix15_to_f32(32768), 1.0);
        assert_eq!(to_u8(1.0), 255);
        assert_eq!(to_u8(0.0), 0);
        assert_eq!(to_u8(0.5), 128); // round-half-up of 127.5
        assert_eq!(unpremultiply_u8([0.0, 0.0, 0.0, 0.0]), [0, 0, 0, 0]);
        // Premultiplied half-alpha red -> straight red at alpha 128.
        assert_eq!(unpremultiply_u8([0.5, 0.0, 0.0, 0.5]), [255, 0, 0, 128]);
        assert_eq!(straight_u8_to_fix15([255, 0, 0, 255]), [32768, 0, 0, 32768]);
        assert_eq!(straight_u8_to_fix15([0, 0, 0, 0]), [0, 0, 0, 0]);
    }

    /// Straight 8-bit -> fix15 -> straight 8-bit must be lossless for every
    /// colour at every alpha >= 2. See `core::ora` for why alpha 1 cannot be.
    #[test]
    fn eight_bit_roundtrip_is_exact_above_alpha_one() {
        for a in [2u8, 3, 17, 64, 128, 200, 254, 255] {
            for c in 0..=255u8 {
                let src = [c, c / 2, 255 - c, a];
                let f = straight_u8_to_fix15(src);
                let back = unpremultiply_u8(px_to_f32(f));
                assert_eq!(back, src, "alpha {a}, colour {c}");
            }
        }
    }
}

/// LP-016/LP-017 layer-colour tint of one PREMULTIPLIED fix15 pixel: the
/// layer's dark ink renders as `tint` (the MAIN colour), its white end as
/// `sub` (the SUB colour, `None` = plain white), and the alpha and luminance
/// structure are preserved — non-destructive display math every compositor
/// shares. Straight value v (per-channel) → luminance ℓ = mean; new straight
/// = lerp(main, sub, ℓ); premul again.
///
/// `sub = None` reduces to the LP-016 formula bit-for-bit (`1.0 * ℓ == ℓ`),
/// which is why an old file with a main colour and no sub colour renders
/// exactly the pixels it always did.
#[inline]
pub fn layer_colour_tint(px: [u16; 4], tint: [u8; 3], sub: Option<[u8; 3]>) -> [u16; 4] {
    let a = px[3] as u32;
    if a == 0 {
        return px;
    }
    let v = |c: usize| px[c] as f32 / a as f32;
    let lum = (v(0) + v(1) + v(2)) / 3.0;
    let mut out = [0u16; 4];
    for c in 0..3 {
        let t = tint[c] as f32 / 255.0;
        let s = match sub {
            Some(s) => t * (1.0 - lum) + (s[c] as f32 / 255.0) * lum,
            None => t * (1.0 - lum) + lum,
        };
        out[c] = (s * a as f32 + 0.5) as u16;
    }
    out[3] = px[3];
    out
}

/// LP-022 "decrease colour" DISPLAY reduce of one PREMULTIPLIED fix15 pixel:
/// what the layer would look like flattened to grey, or to 1-bit monochrome.
///
/// This is a PREVIEW and nothing else — no pixel is converted, and the
/// export composite skips it (`export::CompOpts`). Monochrome thresholds the
/// ALPHA as well as the value, on purpose: the point of the print check is to
/// show you the aliased edge a 1-bit page would actually print, and a
/// preview that kept the brush's soft edge would hide exactly the problem it
/// is there to find. The threshold is a fixed 50 % — see the manual.
#[inline]
pub fn expression_reduce(px: [u16; 4], e: crate::doc::LayerExpression) -> [u16; 4] {
    use crate::doc::LayerExpression as E;
    let a = px[3] as f32;
    if e == E::Colour || a == 0.0 {
        return px;
    }
    let lum = (px[0] as f32 + px[1] as f32 + px[2] as f32) / (3.0 * a);
    match e {
        E::Colour => px,
        E::Grey => {
            let s = (lum * a + 0.5) as u16;
            [s, s, s, px[3]]
        }
        E::Mono => {
            let a1 = if px[3] >= 16384 { 32768u16 } else { 0 };
            let s = if lum >= 0.5 { a1 } else { 0 };
            [s, s, s, a1]
        }
    }
}

#[cfg(test)]
mod layer_colour_tests {
    use super::*;

    /// LP-016 math: black ink shows as the tint (at full alpha), white
    /// stays white, grey lands on the lerp, alpha is never touched.
    #[test]
    fn layer_colour_tint_maps_the_ink() {
        let blue = [0x00, 0x00, 0xff];
        // Opaque black → the tint, straight.
        let p = layer_colour_tint([0, 0, 0, 32767], blue, None);
        assert_eq!(p[3], 32767, "alpha preserved");
        assert_eq!(p[2], 32767, "black shows as blue (B channel full)");
        assert_eq!(p[0], 0);
        // Opaque white → white (white stays white).
        let p = layer_colour_tint([32767; 4], blue, None);
        assert_eq!(p, [32767, 32767, 32767, 32767]);
        // Mid grey: lum 0.5 → halfway between blue and white per channel.
        let g = layer_colour_tint([16384, 16384, 16384, 32767], blue, None);
        // R: 0*(0.5)+0.5 = 0.5; B: 1*(0.5)+0.5 = 1.0.
        assert!((g[0] as i32 - 16384).abs() <= 2, "R half: {}", g[0]);
        assert!(g[2] >= 32766, "B full-ish: {}", g[2]);
        // Half-alpha black: premul tint at half alpha.
        let p = layer_colour_tint([0, 0, 0, 16384], blue, None);
        assert_eq!(p[3], 16384);
        assert!((p[2] as i32 - 16384).abs() <= 1, "premul B half: {}", p[2]);
        // Transparent: untouched.
        assert_eq!(layer_colour_tint([5, 5, 5, 0], blue, None), [5, 5, 5, 0]);
    }

    /// LP-017: the SUB colour is the other half of the two-tone pair — main
    /// replaces black, sub replaces WHITE. And an explicit white sub must
    /// agree with `None` to the bit, because that is the compatibility
    /// promise every old file rides on.
    #[test]
    fn sub_colour_replaces_the_white_end() {
        let blue = [0x00, 0x00, 0xff];
        let amber = [0xff, 0xc0, 0x00];
        // Opaque WHITE now shows the sub colour.
        let p = layer_colour_tint([32768; 4], blue, Some(amber));
        assert_eq!(p[0], 32768, "white → amber R full");
        assert!(p[2] <= 1, "white → amber B empty, got {}", p[2]);
        // Opaque BLACK still shows the main colour: the two ends are
        // independent.
        let p = layer_colour_tint([0, 0, 0, 32768], blue, Some(amber));
        assert_eq!(p[2], 32768, "black → blue B full");
        assert!(p[0] <= 1, "black → blue R empty, got {}", p[0]);
        // Mid grey lands halfway between the two chips, not between a chip
        // and white.
        let g = layer_colour_tint([16384, 16384, 16384, 32768], blue, Some(amber));
        assert!((g[0] as i32 - 16384).abs() <= 2, "R half: {}", g[0]);
        assert!((g[2] as i32 - 16384).abs() <= 2, "B half: {}", g[2]);
        // An explicit white sub == no sub, everywhere.
        for a in [0u16, 1, 9000, 16384, 32768] {
            for v in [0u16, 3000, 16384, 32768] {
                let px = [v.min(a), v.min(a) / 2, 0, a];
                assert_eq!(
                    layer_colour_tint(px, blue, Some([255, 255, 255])),
                    layer_colour_tint(px, blue, None),
                    "white sub must be bit-identical to no sub at {px:?}"
                );
            }
        }
    }

    /// LP-022: grey drops the chroma and keeps the alpha; mono is 1-BIT in
    /// both value and coverage; Colour is the identity on every input (the
    /// old-file promise).
    #[test]
    fn expression_reduce_greys_and_thresholds() {
        use crate::doc::LayerExpression as E;
        // Opaque red → its own luminance, three equal channels.
        let red = [32768, 0, 0, 32768];
        let g = expression_reduce(red, E::Grey);
        assert_eq!(g[3], 32768, "alpha survives grey");
        assert_eq!(g[0], g[1]);
        assert_eq!(g[1], g[2]);
        assert!((g[0] as i32 - 10923).abs() <= 2, "mean of 1,0,0: {}", g[0]);
        // Mono: dark ink goes to black-at-full-alpha, light ink to white,
        // and a soft edge (alpha 40 %) drops out entirely.
        assert_eq!(expression_reduce([0, 0, 0, 32768], E::Mono), [0, 0, 0, 32768]);
        assert_eq!(
            expression_reduce([32768; 4], E::Mono),
            [32768, 32768, 32768, 32768]
        );
        assert_eq!(expression_reduce([0, 0, 0, 13000], E::Mono), [0; 4]);
        // A half-covered dark pixel keeps its coverage but loses the ramp.
        assert_eq!(
            expression_reduce([0, 0, 0, 20000], E::Mono),
            [0, 0, 0, 32768]
        );
        // Colour is the identity, transparent pixels are never touched.
        for e in [E::Colour, E::Grey, E::Mono] {
            assert_eq!(expression_reduce([7, 8, 9, 0], e), [7, 8, 9, 0]);
        }
        assert_eq!(expression_reduce(red, E::Colour), red);
    }
}
