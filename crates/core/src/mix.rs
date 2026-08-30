//! The colour-mixing core — CSP Ink ▸ **Mixing mode** (`I-014`) and
//! Gradient ▸ **Color mixing space** (`G-009`), which are the same question
//! asked of two different tools: *when two colours meet, what math decides
//! the colour in between?*
//!
//! Before this module the answer lived in `gradient.rs` and nowhere else, so
//! the brush — the tool an artist actually mixes paint with — had no say at
//! all (`CSP-TRIAGE-STATUS` row 58: "evidence is the GRADIENT tool's
//! `MixMode::Perceptual`, not a brush ink option"). The math moved here
//! verbatim; the gradient now calls it, and the brush is the second consumer.
//!
//! # Two families, and why they are not one function
//!
//! There are two genuinely different mixing problems here and collapsing them
//! would be a lie:
//!
//! - **[`MixMode`] — interpolating two AUTHORED colours** (a gradient stop
//!   pair, a jitter offset). Nothing is wet; there is no pigment. The only
//!   question is which space the lerp happens in, and Oklab is the answer
//!   that keeps a blue→yellow ramp from sagging through grey.
//! - **[`BrushMix`] — mixing WET PIGMENT into pigment already on the page.**
//!   That is subtractive: two paints that each reflect a band of the spectrum
//!   multiply, they do not average. Oklab would still average. libmypaint
//!   ships the real thing (weighted geometric mean over a 10-band spectral
//!   upsampling of sRGB — `rgb_to_spectral`/`mix_colors` in
//!   `vendor/libmypaint/helpers.c`, the `paint_mode` setting), and
//!   [`BrushMix::Perceptual`] turns exactly that on. Re-deriving it in Rust
//!   would be a SECOND implementation of colour science we already vendor,
//!   which is the thing this module exists to prevent.
//!
//! So: one module, one Oklab implementation shared by the gradient and the
//! brush's colour jitter, and one honest hand-off to the vendored spectral
//! code for wet paint. What both families share is the enum's meaning and its
//! default — **Standard is always the pre-existing behaviour, bit for bit.**

use serde::{Deserialize, Serialize};

/// `G-009` / `I-014` — the space two colours are mixed in.
///
/// Moved here from `gradient.rs` unchanged (including the deliberate
/// `a + (b - a) * s` spelling of the Standard arm, which is what makes an
/// unedited gradient reload bit-for-bit).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MixMode {
    /// Straight lerp of the sRGB-encoded channels. What shipped first, and
    /// what every other paint program calls "normal".
    #[default]
    Standard,
    /// Lerp in Oklab. Decides whether a blue→yellow ramp goes through a
    /// muddy grey or keeps its lightness across the middle — the difference
    /// shows up worst on the long soft ramps that get printed.
    Perceptual,
    /// Lerp in LINEAR light, which is what Photoshop does with "Blend RGB
    /// Colors Using Gamma 1.00" — the physically-correct mix, and the one
    /// that makes a black→white ramp read mid-grey at the halfway point
    /// rather than dark. CSP calls it "Linear (PS compat)".
    Linear,
}

impl MixMode {
    pub fn label(self) -> &'static str {
        match self {
            MixMode::Standard => "Standard",
            MixMode::Perceptual => "Perceptual",
            MixMode::Linear => "Linear (PS compat)",
        }
    }

    pub const ALL: [MixMode; 3] = [MixMode::Standard, MixMode::Perceptual, MixMode::Linear];
}

/// `G-010` — how hard Perceptual mixing fights the lightness dip in the
/// middle of a ramp between two saturated colours. Five levels, and it does
/// nothing outside [`MixMode::Perceptual`], exactly as in CSP.
pub const MAX_BRIGHT: u8 = 4;

/// `I-014` on the BRUSH side (triage row 58): how a dab's pigment meets the
/// pigment already on the canvas.
///
/// Two-way, not three, because [`MixMode::Linear`] has no meaning for wet
/// paint — "mix in linear light" is still an additive average, which is the
/// exact thing Perceptual exists to stop doing.
///
/// # The routing consequence
///
/// [`Self::Perceptual`] is not a colour tweak, it is a different rasterizer —
/// but since the wave-4 spectral port the GPU dab shader carries it too
/// (`dab.wgsl`'s `*_Paint` arms, parity-pinned against the C), so the mode no
/// longer forces the CPU path. Only a preset whose `paint_mode` was
/// input-mapped at load stays CPU-routed (`MyBrush::paint_mapped`). Standard
/// leaves every byte of the existing path alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BrushMix {
    /// Additive mixing in sRGB — libmypaint's classic behaviour, and what
    /// every preset that has never heard of this row already does.
    #[default]
    Standard,
    /// Subtractive spectral (pigment) mixing — libmypaint's `paint_mode`.
    /// Blue over yellow goes green instead of grey.
    Perceptual,
}

impl BrushMix {
    pub fn label(self) -> &'static str {
        match self {
            BrushMix::Standard => "Standard",
            BrushMix::Perceptual => "Paint",
        }
    }

    pub const ALL: [BrushMix; 2] = [BrushMix::Standard, BrushMix::Perceptual];

    /// The value libmypaint's `paint_mode` setting wants: 0 = no spectral
    /// mixing, 1 = only spectral mixing (`brushsettings.json`'s own words).
    ///
    /// Deliberately the full 0/1 and not a slider: CSP's row is a two-way
    /// switch, and a half-spectral brush is a setting nobody can describe.
    pub fn paint_weight(self) -> f32 {
        match self {
            BrushMix::Standard => 0.0,
            BrushMix::Perceptual => 1.0,
        }
    }

    /// Read a stored `paint_mode` weight back as the switch. Anything above
    /// zero is Perceptual — a preset authored in MyPaint may carry 0.7.
    pub fn from_paint_weight(w: f32) -> Self {
        if w > 0.0 {
            BrushMix::Perceptual
        } else {
            BrushMix::Standard
        }
    }
}

/// Mix two straight RGBA colours at `s` (0..=1) in `mode`'s space.
///
/// `bright` is `G-010`'s brightness correction, 0..=[`MAX_BRIGHT`]; it is
/// read only by [`MixMode::Perceptual`] and does nothing elsewhere, exactly
/// as in CSP. Alpha is a plain lerp in every mode — alpha has no perceptual
/// space to be mixed in.
pub fn mix_rgba(mode: MixMode, c0: [f32; 4], c1: [f32; 4], s: f32, bright: u8) -> [f32; 4] {
    match mode {
        // Written as `a + (b - a) * s` deliberately: it is the exact
        // expression the two-colour ramp used before stops existed, so
        // an unedited gradient reloads bit-for-bit.
        MixMode::Standard => [
            c0[0] + (c1[0] - c0[0]) * s,
            c0[1] + (c1[1] - c0[1]) * s,
            c0[2] + (c1[2] - c0[2]) * s,
            c0[3] + (c1[3] - c0[3]) * s,
        ],
        MixMode::Perceptual => {
            let a = srgb_to_oklab([c0[0], c0[1], c0[2]]);
            let b = srgb_to_oklab([c1[0], c1[1], c1[2]]);
            let mut lab = [
                a[0] + (b[0] - a[0]) * s,
                a[1] + (b[1] - a[1]) * s,
                a[2] + (b[2] - a[2]) * s,
            ];
            // `G-010`. The hump is `4s(1-s)`: exactly 0 at both ends, so
            // no correction level can move the authored stop colours —
            // it only lifts the sag in between.
            let level = bright.min(MAX_BRIGHT);
            if level > 0 {
                let peak = a[0].max(b[0]);
                let k = level as f32 / MAX_BRIGHT as f32;
                lab[0] += (peak - lab[0]).max(0.0) * k * 4.0 * s * (1.0 - s);
            }
            let rgb = oklab_to_srgb(lab);
            // Alpha has no perceptual space — it stays a plain lerp.
            [rgb[0], rgb[1], rgb[2], c0[3] + (c1[3] - c0[3]) * s]
        }
        MixMode::Linear => {
            let f = |x: f32, y: f32| {
                let (x, y) = (
                    srgb_to_linear(x.clamp(0.0, 1.0)),
                    srgb_to_linear(y.clamp(0.0, 1.0)),
                );
                linear_to_srgb(x + (y - x) * s)
            };
            [
                f(c0[0], c1[0]),
                f(c0[1], c1[1]),
                f(c0[2], c1[2]),
                c0[3] + (c1[3] - c0[3]) * s,
            ]
        }
    }
}

/// `I-014`'s second clause, the one the CSP manual states outright: the
/// mixing mode **also governs Color Jitter**. Shift an sRGB colour by a
/// hue rotation (in TURNS, the brush's own unit), a chroma multiplier and a
/// lightness offset — in Oklab, so a jitter that brightens does not also
/// wash the colour out the way an HSV `v +=` does.
///
/// The Standard path does not call this at all; it stays on libmypaint's
/// HSV offsets, byte for byte. Called with all three deltas at zero this is
/// still a round trip through Oklab and back, so callers skip it when the
/// jitter is off rather than relying on it being the identity.
pub fn shift_oklab(rgb: [f32; 3], d_hue_turns: f32, d_chroma: f32, d_light: f32) -> [f32; 3] {
    let lab = srgb_to_oklab(rgb);
    let (a, b) = (lab[1], lab[2]);
    // Chroma scales, it does not add: adding a fixed amount to a near-grey
    // invents a hue out of rounding noise, and "jitter the saturation" of
    // black has to stay black.
    let scale = (1.0 + d_chroma).max(0.0);
    let th = d_hue_turns * std::f32::consts::TAU;
    let (sin, cos) = th.sin_cos();
    oklab_to_srgb([
        (lab[0] + d_light).clamp(0.0, 1.0),
        (a * cos - b * sin) * scale,
        (a * sin + b * cos) * scale,
    ])
}

pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// sRGB-encoded 0..1 → Oklab (Björn Ottosson's matrices).
pub fn srgb_to_oklab(c: [f32; 3]) -> [f32; 3] {
    let r = srgb_to_linear(c[0].clamp(0.0, 1.0));
    let g = srgb_to_linear(c[1].clamp(0.0, 1.0));
    let b = srgb_to_linear(c[2].clamp(0.0, 1.0));
    let l = (0.412_221_5 * r + 0.536_332_54 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

pub fn oklab_to_srgb(lab: [f32; 3]) -> [f32; 3] {
    let l = (lab[0] + 0.396_337_78 * lab[1] + 0.215_803_76 * lab[2]).powi(3);
    let m = (lab[0] - 0.105_561_346 * lab[1] - 0.063_854_17 * lab[2]).powi(3);
    let s = (lab[0] - 0.089_484_18 * lab[1] - 1.291_485_5 * lab[2]).powi(3);
    [
        linear_to_srgb((4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s).clamp(0.0, 1.0)),
        linear_to_srgb((-1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s).clamp(0.0, 1.0)),
        linear_to_srgb((-0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s).clamp(0.0, 1.0)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Standard arm must be the plain lerp it always was — this is the
    /// byte-pin that lets the gradient call the moved code without its own
    /// files reloading differently.
    #[test]
    fn standard_is_the_plain_srgb_lerp() {
        let a = [0.2, 0.4, 0.6, 0.8];
        let b = [0.9, 0.1, 0.5, 0.3];
        for i in 0..=10 {
            let s = i as f32 / 10.0;
            let got = mix_rgba(MixMode::Standard, a, b, s, 0);
            for ch in 0..4 {
                assert_eq!(
                    got[ch].to_bits(),
                    (a[ch] + (b[ch] - a[ch]) * s).to_bits(),
                    "channel {ch} at s={s}"
                );
            }
        }
    }

    /// The claim in triage row 58: Standard mixing "drives blends toward
    /// grey mud". Blue→yellow is the worst case, so measure it — the
    /// Perceptual midpoint must be further from grey than Standard's.
    #[test]
    fn perceptual_midpoint_is_less_grey_than_standard() {
        let blue = [0.0, 0.0, 1.0, 1.0];
        let yellow = [1.0, 1.0, 0.0, 1.0];
        let chroma = |c: [f32; 4]| {
            let lab = srgb_to_oklab([c[0], c[1], c[2]]);
            (lab[1] * lab[1] + lab[2] * lab[2]).sqrt()
        };
        let std_mid = chroma(mix_rgba(MixMode::Standard, blue, yellow, 0.5, 0));
        let perc_mid = chroma(mix_rgba(MixMode::Perceptual, blue, yellow, 0.5, 0));
        assert!(
            perc_mid > std_mid,
            "perceptual {perc_mid} should hold more colour than standard {std_mid}"
        );
    }

    /// `G-010`: brightness correction can lift the middle and must NEVER
    /// move an authored end.
    #[test]
    fn brightness_correction_never_moves_the_ends() {
        let a = [0.1, 0.1, 0.7, 1.0];
        let b = [0.9, 0.8, 0.1, 1.0];
        for level in 0..=MAX_BRIGHT {
            assert_eq!(
                mix_rgba(MixMode::Perceptual, a, b, 0.0, level),
                mix_rgba(MixMode::Perceptual, a, b, 0.0, 0)
            );
            assert_eq!(
                mix_rgba(MixMode::Perceptual, a, b, 1.0, level),
                mix_rgba(MixMode::Perceptual, a, b, 1.0, 0)
            );
        }
    }

    #[test]
    fn brush_mix_maps_to_libmypaint_paint_mode() {
        assert_eq!(BrushMix::Standard.paint_weight(), 0.0);
        assert_eq!(BrushMix::Perceptual.paint_weight(), 1.0);
        assert_eq!(BrushMix::from_paint_weight(0.0), BrushMix::Standard);
        assert_eq!(BrushMix::from_paint_weight(0.7), BrushMix::Perceptual);
        assert_eq!(BrushMix::default(), BrushMix::Standard);
    }

    /// The jitter shift must leave a colour alone when nothing is asked of
    /// it (within the Oklab round trip's own precision), brighten without
    /// desaturating, and keep black black when only chroma moves.
    #[test]
    fn oklab_shift_behaves() {
        let c = [0.3, 0.5, 0.8];
        let same = shift_oklab(c, 0.0, 0.0, 0.0);
        for ch in 0..3 {
            assert!((same[ch] - c[ch]).abs() < 2e-3, "round trip ch{ch}");
        }
        let brighter = shift_oklab(c, 0.0, 0.0, 0.15);
        let lab0 = srgb_to_oklab(c);
        let lab1 = srgb_to_oklab(brighter);
        assert!(lab1[0] > lab0[0], "lightness rose");
        let chroma = |l: [f32; 3]| (l[1] * l[1] + l[2] * l[2]).sqrt();
        assert!(
            chroma(lab1) > chroma(lab0) * 0.85,
            "brightening must not wash the colour out: {} vs {}",
            chroma(lab1),
            chroma(lab0)
        );
        let black = shift_oklab([0.0, 0.0, 0.0], 0.0, 0.5, 0.0);
        for ch in 0..3 {
            assert!(black[ch] < 1e-3, "chroma jitter cannot colour black");
        }
    }
}
