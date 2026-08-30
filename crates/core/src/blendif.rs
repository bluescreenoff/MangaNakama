//! Blend If — the underlying-luminance gate (Photoshop's *Blend If ▸
//! Underlying Layer*, the one every retoucher actually reaches for).
//!
//! One idea: **this layer only shows where the page UNDER it is dark enough
//! (or light enough)**. Drop a tone over a face and it lands in the shadows
//! only; drop a highlight over ink and it stays off the black. Nothing is
//! erased and no mask is painted — the layer is gated per pixel by what it
//! is sitting on.
//!
//! # ONE range, with a source and a channel (round 2)
//!
//! Photoshop's dialog has two arms (*This Layer* as well as *Underlying
//! Layer*) times four channels (Gray/R/G/B), and both arms can be live at
//! once. Round 1 shipped the Underlying arm on luminance only. This round
//! adds the deferred arms the way the struct was shaped for: ONE band with a
//! selectable [`GateSource`] (underlying composite or the layer's own ink)
//! and a selectable [`GateChannel`] (luma or one of R/G/B).
//!
//! Still deferred, deliberately: the two arms live SIMULTANEOUSLY. That is
//! two independent bands whose weights multiply, which is two of everything
//! — two pairs of slider rows, twice the GPU instance payload, a second
//! serialized band — for a case that in practice is dialled one arm at a
//! time. When somebody asks, it is `this: Option<Band>` beside the existing
//! fields and a second `weight()` call in [`BlendIf::weight_for`]; the door
//! ([`crate::doc::Layer::gate`]) and both compositors' application point do
//! not move.
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
//! # What "underlying" means here (and what "this layer" means)
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
//!
//! [`GateSource::This`] reads the layer's OWN finished source at that pixel —
//! after the expression reduce, the layer colour, the opacity, the mask and
//! the clip, at the exact point the gate is applied. The value is
//! UNPREMULTIPLIED, so scaling the whole pixel (opacity, mask, clip) does not
//! move it: dropping a layer to 40% does not slide its own gate along the
//! range, which is what an artist expects and what Photoshop does. A fully
//! transparent source pixel reads 0 like a transparent destination does, and
//! it does not matter — there is nothing there to gate.

use crate::blend::Rgba;

/// The W3C/PDF nonseparable luma coefficients — the SAME ones
/// `blend::blend_premul` uses for Hue/Saturation/Color/Luminosity and
/// `blend2.wgsl`'s `lum3`. One definition of "how bright is this pixel" per
/// application; a second one would make Blend If and Darker Color disagree
/// about which of two greys is darker.
pub const LUMA: [f32; 3] = [0.3, 0.59, 0.11];

/// WHICH pixel the gate reads. Photoshop's two arms, as a choice rather than
/// as two simultaneous bands (see the module doc for why).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize,
)]
pub enum GateSource {
    /// The composite BELOW this layer at this point in the walk — round 1's
    /// only answer, and the one every retoucher reaches for.
    #[default]
    Underlying,
    /// The layer's OWN finished source, unpremultiplied. "Drop my darkest
    /// ink and keep the rest" without erasing anything.
    This,
}

impl GateSource {
    /// The serialized default — `mnc-blendif` omits it, so a page saved by a
    /// gate that never left the underlying arm is byte-identical to one
    /// saved before this round.
    pub fn is_underlying(&self) -> bool {
        matches!(self, GateSource::Underlying)
    }

    pub fn label(self) -> &'static str {
        match self {
            GateSource::Underlying => "Underlying",
            GateSource::This => "This layer",
        }
    }

    pub const ALL: [GateSource; 2] = [GateSource::Underlying, GateSource::This];
}

/// WHAT of that pixel the gate reads: brightness, or one colour channel.
///
/// Per-channel is the arm that does the jobs luma cannot — gating a tone off
/// a blue sky, keeping a red-pen layer only where the page is not already
/// red. The channel is read from the STRAIGHT colour, like the luma is.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize,
)]
pub enum GateChannel {
    /// The house luma ([`LUMA`]) — Photoshop's "Gray".
    #[default]
    Luma,
    R,
    G,
    B,
}

impl GateChannel {
    pub fn is_luma(&self) -> bool {
        matches!(self, GateChannel::Luma)
    }

    pub fn label(self) -> &'static str {
        match self {
            GateChannel::Luma => "Brightness",
            GateChannel::R => "Red",
            GateChannel::G => "Green",
            GateChannel::B => "Blue",
        }
    }

    pub const ALL: [GateChannel; 4] = [
        GateChannel::Luma,
        GateChannel::R,
        GateChannel::G,
        GateChannel::B,
    ];
}

/// One channel of a PREMULTIPLIED pixel as the gate reads it, 0..1.
///
/// Straight colour (unpremultiplied) and clamped: a Screen or Add
/// destination can carry channels slightly past its own alpha, and a weight
/// function is only defined on 0..1. Zero alpha = 0 — see the module doc.
/// `blend2.wgsl`'s `gate_value` is the twin of this.
#[inline]
pub fn channel_value(px: Rgba, ch: GateChannel) -> f32 {
    let a = px[3];
    if a <= 0.0 {
        return 0.0;
    }
    let v = match ch {
        GateChannel::Luma => LUMA[0] * px[0] + LUMA[1] * px[1] + LUMA[2] * px[2],
        GateChannel::R => px[0],
        GateChannel::G => px[1],
        GateChannel::B => px[2],
    };
    (v / a).clamp(0.0, 1.0)
}

/// The luminance of a PREMULTIPLIED destination pixel, 0..1 — the Luma case
/// of [`channel_value`], kept as its own name because "how bright is the page
/// under this layer" is the question the whole feature is about.
#[inline]
pub fn dst_luma(dst: Rgba) -> f32 {
    channel_value(dst, GateChannel::Luma)
}

/// A layer's Blend If gate: one band, on one [`GateChannel`] of one
/// [`GateSource`]. `None` on the layer = off.
///
/// Serialized as one `mnc-blendif` JSON blob (the `mnc-tone` idiom): a field
/// added here needs no new attribute, and an absent attribute is off. The two
/// fields added in round 2 are `#[serde(default)]` and omitted at their
/// defaults, so old files load and old builds read new files (they ignore the
/// unknown members and show the underlying-luma gate, which IS the gate for
/// every file that does not use the new arms).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BlendIf {
    /// Bottom of the full-strength band, 0..1.
    pub lo: f32,
    /// Top of the full-strength band, 0..1. `< lo` is normalised away.
    pub hi: f32,
    /// Fade distance OUTSIDE each end, in the same units. 0 = a hard edge.
    pub feather: f32,
    /// Which pixel the band is measured on.
    #[serde(default, skip_serializing_if = "GateSource::is_underlying")]
    pub source: GateSource,
    /// Which value of that pixel.
    #[serde(default, skip_serializing_if = "GateChannel::is_luma")]
    pub channel: GateChannel,
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
        source: GateSource::Underlying,
        channel: GateChannel::Luma,
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
            source: self.source,
            channel: self.channel,
        }
    }

    /// Does this gate pass everything? Then it is not worth a snapshot pass
    /// on the GPU or a value read per pixel on the CPU — and, because the
    /// feather points outward, the answer does not depend on it.
    ///
    /// Source and channel deliberately do not enter into it: an open band
    /// passes every value of every channel of either pixel, so the arms
    /// cannot turn an inert gate into a live one. That is what keeps
    /// [`Self::FULL`] usable as the GPU's neutral instance word whatever the
    /// combo boxes say.
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

    /// The value this gate reads at one pixel: the chosen channel of the
    /// chosen pixel. `src` is the layer's finished source, `dst` the
    /// accumulated destination — both PREMULTIPLIED, both at the one point
    /// the gate is applied.
    pub fn value(self, src: Rgba, dst: Rgba) -> f32 {
        let px = match self.source {
            GateSource::Underlying => dst,
            GateSource::This => src,
        };
        channel_value(px, self.channel)
    }

    /// How much of the source survives at this pixel — [`Self::value`] put
    /// through [`Self::weight`]. **The one call every CPU compositor makes**,
    /// so "which pixel, which channel" is answered in exactly one place;
    /// `blend2.wgsl`'s `blendif_weight` is its twin.
    pub fn weight_for(self, src: Rgba, dst: Rgba) -> f32 {
        self.weight(self.value(src, dst))
    }

    /// Bit-exact signature for the GPU's `LayerSig`. Blend If never touches a
    /// tile revision — the pixels are unchanged, only which of them survive —
    /// so without this word the canvas would keep showing the ungated
    /// composite until something else forced a rebuild. (The wave-5 lesson,
    /// paid up front this time.) The arms are in it too: swapping the channel
    /// moves no float, and the canvas would keep the old picture.
    pub fn sig(self) -> [u32; 4] {
        [
            self.lo.to_bits(),
            self.hi.to_bits(),
            self.feather.to_bits(),
            self.mode_bits(),
        ]
    }

    /// The three floats the GPU instance buffer carries, normalised.
    pub fn packed(self) -> [f32; 3] {
        let b = self.normalized();
        [b.lo, b.hi, b.feather]
    }

    /// Source and channel as the one word the GPU instance buffer carries
    /// beside [`Self::packed`]: bit 0 = source (0 underlying, 1 this),
    /// bits 1–2 = channel (0 luma, 1 R, 2 G, 3 B). `0` is the open gate's
    /// value AND the underlying-luma default, which is why every ungated
    /// draw can pass a plain zero.
    pub fn mode_bits(self) -> u32 {
        let s = match self.source {
            GateSource::Underlying => 0,
            GateSource::This => 1,
        };
        let c = match self.channel {
            GateChannel::Luma => 0,
            GateChannel::R => 1,
            GateChannel::G => 2,
            GateChannel::B => 3,
        };
        s | (c << 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    /// An underlying-luma band — round 1's whole vocabulary, and still the
    /// default one, so the curve tests below say only what they are about.
    fn band(lo: f32, hi: f32, feather: f32) -> BlendIf {
        BlendIf {
            lo,
            hi,
            feather,
            ..BlendIf::FULL
        }
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
        let b = band(0.25, 0.75, 0.0);
        assert!(close(b.weight(0.24), 0.0));
        assert!(close(b.weight(0.25), 1.0));
        assert!(close(b.weight(0.75), 1.0));
        assert!(close(b.weight(0.76), 0.0));
    }

    #[test]
    fn the_knees_ramp_linearly_over_the_feather() {
        let b = band(0.4, 0.6, 0.2);
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
        let b = band(0.8, 0.2, 0.0);
        assert_eq!(b.normalized(), band(0.2, 0.8, 0.0));
        assert!(close(b.weight(0.5), 1.0));
    }

    #[test]
    fn out_of_range_values_clamp() {
        let b = band(-3.0, 9.0, -1.0);
        assert_eq!(b.normalized(), BlendIf::FULL);
        assert!(b.is_open());
    }

    /// The arms ride through `normalized` untouched — it is the door every
    /// write into the document goes through, so a source or channel lost
    /// here would be lost on load, on undo and on every command.
    #[test]
    fn normalising_keeps_the_source_and_the_channel() {
        let b = BlendIf {
            lo: 0.9,
            hi: 0.1,
            feather: 3.0,
            source: GateSource::This,
            channel: GateChannel::B,
        };
        let n = b.normalized();
        assert_eq!(n.source, GateSource::This);
        assert_eq!(n.channel, GateChannel::B);
        assert_eq!((n.lo, n.hi, n.feather), (0.1, 0.9, 1.0));
    }

    /// An OPEN band is inert on every arm. This is what lets `FULL` stay the
    /// GPU's neutral instance word however the combo boxes are set — and
    /// what lets `Layer::gate` drop it without asking what it was reading.
    #[test]
    fn every_arm_of_an_open_band_is_still_open() {
        for source in GateSource::ALL {
            for channel in GateChannel::ALL {
                let b = BlendIf {
                    feather: 0.3,
                    source,
                    channel,
                    ..BlendIf::FULL
                };
                assert!(b.is_open(), "{source:?}/{channel:?} stopped being open");
            }
        }
    }

    /// The per-channel read, on a pixel whose three channels are far apart —
    /// a flat mean or a luma would pass every one of these tests wrongly.
    #[test]
    fn a_channel_gate_reads_that_channel_and_not_the_brightness() {
        // Straight (0.8, 0.2, 0.4) at full alpha.
        let px = [0.8, 0.2, 0.4, 1.0];
        assert!(close(channel_value(px, GateChannel::R), 0.8));
        assert!(close(channel_value(px, GateChannel::G), 0.2));
        assert!(close(channel_value(px, GateChannel::B), 0.4));
        // …and the luma is none of them.
        let l = channel_value(px, GateChannel::Luma);
        assert!(close(l, 0.3 * 0.8 + 0.59 * 0.2 + 0.11 * 0.4));

        // Premultiplied at half alpha reads the SAME straight values: the
        // gate must not slide when a layer's opacity comes down.
        let half = [0.4, 0.1, 0.2, 0.5];
        for ch in GateChannel::ALL {
            assert!(close(channel_value(half, ch), channel_value(px, ch)));
        }
        // Nothing there = 0, on every channel.
        for ch in GateChannel::ALL {
            assert!(close(channel_value([0.0; 4], ch), 0.0));
        }
    }

    /// `weight_for` is the single door: it decides WHICH pixel is read, and
    /// the two sources must be able to give opposite answers on the same
    /// pair. (The gate below passes only bright values.)
    #[test]
    fn the_source_decides_which_pixel_is_read() {
        let src = [1.0, 1.0, 1.0, 1.0]; // white ink
        let dst = [0.0, 0.0, 0.0, 1.0]; // on a black page
        let highlights = BlendIf {
            lo: 0.6,
            hi: 1.0,
            feather: 0.0,
            ..BlendIf::FULL
        };
        assert!(close(highlights.weight_for(src, dst), 0.0), "reads the page");
        let this = BlendIf {
            source: GateSource::This,
            ..highlights
        };
        assert!(close(this.weight_for(src, dst), 1.0), "reads its own ink");
        // Swap the pixels and the two answers swap with them.
        assert!(close(highlights.weight_for(dst, src), 1.0));
        assert!(close(this.weight_for(dst, src), 0.0));
    }

    /// The GPU word: every combo is a distinct value, `0` is the default
    /// pair, and the signature moves when an arm does (a channel swap moves
    /// no float, and the canvas would keep the stale picture).
    #[test]
    fn the_mode_word_is_unique_per_combo_and_zero_is_the_default() {
        assert_eq!(BlendIf::FULL.mode_bits(), 0);
        let mut seen = std::collections::HashSet::new();
        for source in GateSource::ALL {
            for channel in GateChannel::ALL {
                let b = BlendIf {
                    lo: 0.2,
                    hi: 0.8,
                    source,
                    channel,
                    ..BlendIf::FULL
                };
                assert!(seen.insert(b.mode_bits()), "{source:?}/{channel:?} collides");
                assert_eq!(b.sig()[3], b.mode_bits(), "the sig carries the arms");
            }
        }
        assert_eq!(seen.len(), 8);
        let a = BlendIf {
            lo: 0.2,
            hi: 0.8,
            ..BlendIf::FULL
        };
        assert_ne!(
            a.sig(),
            BlendIf {
                channel: GateChannel::R,
                ..a
            }
            .sig(),
            "swapping the channel must dirty the canvas"
        );
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
