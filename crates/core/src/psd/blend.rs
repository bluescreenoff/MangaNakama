//! Our blend modes <-> Photoshop's blend keys, both directions in one place.
//!
//! The 27 keys are the ones the legacy writer always wrote
//! (`psd_legacy::blend_key`). Glow Dodge is CSP-only and leaves as Color
//! Dodge, so it comes back as Color Dodge: the one lossy pair.

use photocraft_psd::BlendMode;

use crate::doc::Blend;

/// The Photoshop mode a layer with our `b` is written with.
pub(crate) fn blend_to_psd(b: Blend) -> BlendMode {
    match b {
        Blend::Normal => BlendMode::Normal,
        Blend::Multiply => BlendMode::Multiply,
        Blend::Screen => BlendMode::Screen,
        Blend::Darken => BlendMode::Darken,
        Blend::Lighten => BlendMode::Lighten,
        // Photoshop's own name is "Linear Dodge (Add)".
        Blend::Add => BlendMode::LinearDodge,
        Blend::Subtract => BlendMode::Subtract,
        Blend::Overlay => BlendMode::Overlay,
        Blend::SoftLight => BlendMode::SoftLight,
        Blend::HardLight => BlendMode::HardLight,
        Blend::Difference => BlendMode::Difference,
        Blend::Exclusion => BlendMode::Exclusion,
        Blend::Hue => BlendMode::Hue,
        Blend::Saturation => BlendMode::Saturation,
        Blend::Color => BlendMode::Color,
        Blend::ColorBurn => BlendMode::ColorBurn,
        Blend::LinearBurn => BlendMode::LinearBurn,
        Blend::ColorDodge => BlendMode::ColorDodge,
        // CSP-only; Color Dodge is Photoshop's closest relative (CSP
        // documents it as the compatibility fallback).
        Blend::GlowDodge => BlendMode::ColorDodge,
        Blend::VividLight => BlendMode::VividLight,
        Blend::LinearLight => BlendMode::LinearLight,
        Blend::PinLight => BlendMode::PinLight,
        Blend::HardMix => BlendMode::HardMix,
        Blend::Divide => BlendMode::Divide,
        Blend::DarkerColor => BlendMode::DarkerColor,
        Blend::LighterColor => BlendMode::LighterColor,
        Blend::Luminosity => BlendMode::Luminosity,
    }
}

/// Our mode for a Photoshop mode, plus a note when the mode has no
/// counterpart and was read as Normal. Pass-through is only meaningful on a
/// folder; the importer handles folders before it gets here.
pub(crate) fn blend_from_psd(m: BlendMode) -> (Blend, Option<&'static str>) {
    let b = match m {
        BlendMode::Normal => Blend::Normal,
        BlendMode::Multiply => Blend::Multiply,
        BlendMode::Screen => Blend::Screen,
        BlendMode::Darken => Blend::Darken,
        BlendMode::Lighten => Blend::Lighten,
        BlendMode::LinearDodge => Blend::Add,
        BlendMode::Subtract => Blend::Subtract,
        BlendMode::Overlay => Blend::Overlay,
        BlendMode::SoftLight => Blend::SoftLight,
        BlendMode::HardLight => Blend::HardLight,
        BlendMode::Difference => Blend::Difference,
        BlendMode::Exclusion => Blend::Exclusion,
        BlendMode::Hue => Blend::Hue,
        BlendMode::Saturation => Blend::Saturation,
        BlendMode::Color => Blend::Color,
        BlendMode::ColorBurn => Blend::ColorBurn,
        BlendMode::LinearBurn => Blend::LinearBurn,
        BlendMode::ColorDodge => Blend::ColorDodge,
        BlendMode::VividLight => Blend::VividLight,
        BlendMode::LinearLight => Blend::LinearLight,
        BlendMode::PinLight => Blend::PinLight,
        BlendMode::HardMix => Blend::HardMix,
        BlendMode::Divide => Blend::Divide,
        BlendMode::DarkerColor => Blend::DarkerColor,
        BlendMode::LighterColor => Blend::LighterColor,
        BlendMode::Luminosity => Blend::Luminosity,
        BlendMode::PassThrough => return (Blend::Normal, Some("pass-through on a layer, read as Normal")),
        BlendMode::Dissolve => return (Blend::Normal, Some("Dissolve, read as Normal")),
        BlendMode::Unknown(_) => return (Blend::Normal, Some("unknown blend mode, read as Normal")),
    };
    (b, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every one of our modes leaves and comes back as itself, except Glow
    /// Dodge (CSP-only) which comes back as Color Dodge. Every Photoshop mode
    /// maps to something, Normal only for Normal itself and the two modes we
    /// do not have; an unknown key is Normal with a note.
    #[test]
    fn blend_map_covers_every_variant_both_ways() {
        let ours = [
            Blend::Normal,
            Blend::Multiply,
            Blend::Screen,
            Blend::Darken,
            Blend::Lighten,
            Blend::Add,
            Blend::Subtract,
            Blend::Overlay,
            Blend::SoftLight,
            Blend::HardLight,
            Blend::Difference,
            Blend::Exclusion,
            Blend::Hue,
            Blend::Saturation,
            Blend::Color,
            Blend::ColorBurn,
            Blend::LinearBurn,
            Blend::ColorDodge,
            Blend::GlowDodge,
            Blend::VividLight,
            Blend::LinearLight,
            Blend::PinLight,
            Blend::HardMix,
            Blend::Divide,
            Blend::DarkerColor,
            Blend::LighterColor,
            Blend::Luminosity,
        ];
        // Exhaustiveness guard: a new `Blend` variant breaks this match, so
        // the list above cannot silently fall behind the enum.
        for b in ours {
            match b {
                Blend::Normal
                | Blend::Multiply
                | Blend::Screen
                | Blend::Darken
                | Blend::Lighten
                | Blend::Add
                | Blend::Subtract
                | Blend::Overlay
                | Blend::SoftLight
                | Blend::HardLight
                | Blend::Difference
                | Blend::Exclusion
                | Blend::Hue
                | Blend::Saturation
                | Blend::Color
                | Blend::ColorBurn
                | Blend::LinearBurn
                | Blend::ColorDodge
                | Blend::GlowDodge
                | Blend::VividLight
                | Blend::LinearLight
                | Blend::PinLight
                | Blend::HardMix
                | Blend::Divide
                | Blend::DarkerColor
                | Blend::LighterColor
                | Blend::Luminosity => {}
            }
            let (back, note) = blend_from_psd(blend_to_psd(b));
            assert_eq!(note, None, "{b:?} has a counterpart");
            let want = if b == Blend::GlowDodge { Blend::ColorDodge } else { b };
            assert_eq!(back, want, "{b:?} round trip");
        }
        let keys: std::collections::HashSet<[u8; 4]> = ours.iter().map(|b| blend_to_psd(*b).key()).collect();
        assert_eq!(keys.len(), 26, "27 modes, Glow Dodge shares Color Dodge's key");

        for m in BlendMode::ALL {
            let (b, note) = blend_from_psd(m);
            let lossy = matches!(m, BlendMode::PassThrough | BlendMode::Dissolve);
            assert_eq!(note.is_some(), lossy, "{m:?}");
            if m != BlendMode::Normal {
                assert_eq!(b == Blend::Normal, lossy, "{m:?} is Normal only when we lack it");
            }
        }
        let (b, note) = blend_from_psd(BlendMode::Unknown(*b"zzzz"));
        assert_eq!(b, Blend::Normal);
        assert!(note.is_some());
    }
}
