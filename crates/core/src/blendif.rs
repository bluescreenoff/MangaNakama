//! Blend If — the underlying-luminance gate (Photoshop's *Blend If ▸
//! Underlying Layer*, the one every retoucher actually reaches for).
//!
//! One idea: **this layer only shows where the page UNDER it is dark enough
//! (or light enough)**. Drop a tone over a face and it lands in the shadows
//! only; drop a highlight over ink and it stays off the black. Nothing is
//! erased and no mask is painted — the layer is gated per pixel by what it
//! is sitting on.
//!
//! # Deliberately ONE range (owner ruling 2026-08-30: "keep it super basic")
//!
//! Photoshop's full dialog has two arms (*This Layer* as well as *Underlying
//! Layer*) times four channels (Gray/R/G/B). This is the Underlying arm, on
//! luminance, and nothing else. The other arms are recorded as deferred, not
//! forgotten: [`BlendIf`] is a struct, so a `this: Option<..>` or a channel
//! selector can be added later without moving a single call site — but the
//! matrix is not built until somebody asks for it.
//!
//! # The shape of the range
//!
//! `lo..hi` is the band that shows at FULL strength; `feather` fades the
//! layer out over that many luma units **outside** each end:
//!
//! ```text
//!  weight
//!    1 |        ______________
//!      |       /              \
//!    0 |______/                \______
//!      +-----------------------------> underlying luma
//!         lo-f  lo          hi  hi+f
//! ```
//!
//! Feather points OUTWARD on purpose. Two reasons, both practical: dragging
//! the feather never eats into a band you already dialled in (the soft edge
//! only grows outward from it), and — the load-bearing one — the default
//! `0..1` stays a perfect no-op *at any feather*, so "open" is a property of
//! the range alone. That is what lets [`BlendIf::FULL`] be the neutral value
//! the GPU instance buffer carries on every draw, with no sentinel.
//!
//! # What "underlying" means here
//!
//! Exactly what the compositor has accumulated **below this layer at this
//! point in the walk** — which is the destination accumulator in
//! `export::composite_size`, and the destination snapshot in `blend2.wgsl`.
//! Inside a sealed folder that is the GROUP's content so far (the folder is
//! isolated, so the page under it is not visible to a child — the same
//! answer Photoshop gives an isolated group); a Through folder collapses onto
//! its parent accumulator, so a child there does see the page. Both fall out
//! of the existing accumulator model rather than being special-cased, and
//! both are pinned by tests.
//!
//! Where the underlying composite is TRANSPARENT the luma reads as 0 (black).
//! That is a real consequence, not an oversight: over an empty transparent
//! page a "shadows only" layer shows everywhere and a "highlights only" layer
//! shows nowhere. On the paper-white canvas the artist actually draws on, the
//! backdrop is opaque and the question never comes up.

use crate::blend::Rgba;

/// The W3C/PDF nonseparable luma coefficients — the SAME ones
/// `blend::blend_premul` uses for Hue/Saturation/Color/Luminosity and
/// `blend2.wgsl`'s `lum3`. One definition of "how bright is this pixel" per
/// application; a second one would make Blend If and Darker Color disagree
/// about which of two greys is darker.
pub const LUMA: [f32; 3] = [0.3, 0.59, 0.11];

/// The luminance of a PREMULTIPLIED destination pixel, 0..1.
///
/// Straight colour (unpremultiplied) and clamped: a Screen or Add
/// destination can carry channels slightly past its own alpha, and a weight
/// function is only defined on 0..1. Zero alpha = 0 — see the module doc.
#[inline]
pub fn dst_luma(dst: Rgba) -> f32 {
    let a = dst[3];
    if a <= 0.0 {
        return 0.0;
    }
    let l = (LUMA[0] * dst[0] + LUMA[1] * dst[1] + LUMA[2] * dst[2]) / a;
    l.clamp(0.0, 1.0)
}

/// A layer's underlying-luminance gate. `None` on the layer = off.
///
/// Serialized as one `mnc-blendif` JSON blob (the `mnc-tone` idiom): a field
/// added here needs no new attribute, and an absent attribute is off.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BlendIf {
    /// Bottom of the full-strength band, 0..1 luma.
    pub lo: f32,
    /// Top of the full-strength band, 0..1 luma. `< lo` is normalised away.
    pub hi: f32,
    /// Fade distance OUTSIDE each end, in luma units. 0 = a hard edge.
    pub feather: f32,
}

impl Default for BlendIf {
    fn default() -> Self {
        Self::FULL
    }
}

impl BlendIf {
    /// The open gate: passes every luminance at full strength. The value a
    /// freshly ticked-on Blend If starts at, the value the reset affordance
    /// returns to, and the neutral the GPU instance buffer carries on every
    /// draw that has no gate.
    pub const FULL: BlendIf = BlendIf {
        lo: 0.0,
        hi: 1.0,
        feather: 0.0,
    };

    /// Clamp into range and put `lo`/`hi` the right way round. Every door
    /// into the document goes through this, so no compositor ever has to
    /// wonder whether `hi < lo` (it would silently hide the whole layer).
    pub fn normalized(self) -> Self {
        let lo = self.lo.clamp(0.0, 1.0);
        let hi = self.hi.clamp(0.0, 1.0);
        Self {
            lo: lo.min(hi),
            hi: lo.max(hi),
            feather: self.feather.clamp(0.0, 1.0),
        }
    }

    /// Does this gate pass everything? Then it is not worth a snapshot pass
    /// on the GPU or a luma read per pixel on the CPU — and, because the
    /// feather points outward, the answer does not depend on it.
    pub fn is_open(self) -> bool {
        self.lo <= 0.0 && self.hi >= 1.0
    }

    /// How much of the source survives at this underlying luminance, 0..1.
    ///
    /// The knees are linear ramps. Feather 0 gives a hard step, which is what
    /// a hand-typed "shadows below 0.25" wants; anything else is what stops
    /// a tone from ending in a visible contour line.
    pub fn weight(self, luma: f32) -> f32 {
        let b = self.normalized();
        if luma >= b.lo && luma <= b.hi {
            return 1.0;
        }
        if b.feather <= 0.0 {
            return 0.0;
        }
        let d = if luma < b.lo {
            b.lo - luma
        } else {
            luma - b.hi
        };
        (1.0 - d / b.feather).clamp(0.0, 1.0)
    }

    /// Bit-exact signature for the GPU's `LayerSig`. Blend If never touches a
    /// tile revision — the pixels are unchanged, only which of them survive —
    /// so without this word the canvas would keep showing the ungated
    /// composite until something else forced a rebuild. (The wave-5 lesson,
    /// paid up front this time.)
    pub fn sig(self) -> [u32; 3] {
        [
            self.lo.to_bits(),
            self.hi.to_bits(),
            self.feather.to_bits(),
        ]
    }

    /// The three floats the GPU instance buffer carries, normalised.
    pub fn packed(self) -> [f32; 3] {
        let b = self.normalized();
        [b.lo, b.hi, b.feather]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn the_default_is_open_and_weights_everything_at_one() {
        let b = BlendIf::default();
        assert!(b.is_open());
        for i in 0..=10 {
            assert!(close(b.weight(i as f32 / 10.0), 1.0));
        }
    }

    /// The feather points OUTWARD, so it cannot turn the open gate into a
    /// gate. This is what lets `FULL` be the GPU's neutral instance value.
    #[test]
    fn an_open_range_stays_open_at_any_feather() {
        let b = BlendIf {
            feather: 0.5,
            ..BlendIf::FULL
        };
        assert!(b.is_open());
        assert!(close(b.weight(0.0), 1.0));
        assert!(close(b.weight(1.0), 1.0));
    }

    #[test]
    fn a_hard_range_is_a_step() {
        let b = BlendIf {
            lo: 0.25,
            hi: 0.75,
            feather: 0.0,
        };
        assert!(close(b.weight(0.24), 0.0));
        assert!(close(b.weight(0.25), 1.0));
        assert!(close(b.weight(0.75), 1.0));
        assert!(close(b.weight(0.76), 0.0));
    }

    #[test]
    fn the_knees_ramp_linearly_over_the_feather() {
        let b = BlendIf {
            lo: 0.4,
            hi: 0.6,
            feather: 0.2,
        };
        // Lower knee: full at 0.4, half at 0.3, gone at 0.2 and below.
        assert!(close(b.weight(0.4), 1.0));
        assert!(close(b.weight(0.3), 0.5));
        assert!(close(b.weight(0.2), 0.0));
        assert!(close(b.weight(0.1), 0.0));
        // Upper knee mirrors it.
        assert!(close(b.weight(0.6), 1.0));
        assert!(close(b.weight(0.7), 0.5));
        assert!(close(b.weight(0.8), 0.0));
    }

    /// `hi < lo` would hide the layer everywhere. Normalising at every door
    /// makes that unrepresentable rather than a surprise.
    #[test]
    fn an_inverted_range_normalises_instead_of_hiding_everything() {
        let b = BlendIf {
            lo: 0.8,
            hi: 0.2,
            feather: 0.0,
        };
        assert_eq!(
            b.normalized(),
            BlendIf {
                lo: 0.2,
                hi: 0.8,
                feather: 0.0
            }
        );
        assert!(close(b.weight(0.5), 1.0));
    }

    #[test]
    fn out_of_range_values_clamp() {
        let b = BlendIf {
            lo: -3.0,
            hi: 9.0,
            feather: -1.0,
        };
        assert_eq!(b.normalized(), BlendIf::FULL);
        assert!(b.is_open());
    }

    /// The premultiplied → straight → luma path, including the transparent
    /// case the module doc promises reads as black.
    #[test]
    fn dst_luma_unpremultiplies_and_reads_transparent_as_black() {
        assert!(close(dst_luma([1.0, 1.0, 1.0, 1.0]), 1.0));
        assert!(close(dst_luma([0.0, 0.0, 0.0, 1.0]), 0.0));
        // Half-covered white: premultiplied 0.5s at alpha 0.5 is white.
        assert!(close(dst_luma([0.5, 0.5, 0.5, 0.5]), 1.0));
        assert!(close(dst_luma([0.0; 4]), 0.0));
        // The house coefficients, not a flat mean.
        assert!(close(dst_luma([0.0, 1.0, 0.0, 1.0]), 0.59));
    }

    /// A Screen/Add destination can carry channels past its own alpha; the
    /// weight function is only defined on 0..1.
    #[test]
    fn dst_luma_clamps_an_over_bright_destination() {
        assert!(close(dst_luma([2.0, 2.0, 2.0, 1.0]), 1.0));
    }
}
