//! Publisher / printer profile (ROADMAP medium M2): ONE choice on the work
//! that drives trim/bleed/safety geometry, binding direction, screen ruling,
//! page-count padding and the export finish — pickable AT ANY TIME, not only
//! at creation (that is the whole point: "I decided to submit this to X"
//! happens mid-work). The profile is data only; APPLYING it writes through
//! the existing doors (page-size machinery, preflight inputs, export
//! finish), so nothing here duplicates a mechanism.
//!
//! v2 fields the JP crawl says a BOOK needs (spine width from page count ×
//! paper thickness, cover-spread setup, 台割り page allocation) are
//! deliberately absent but the struct is serde-defaulted throughout, so
//! adding them later costs no migration.

use serde::{Deserialize, Serialize};

use crate::doc::LayerExpression;
use crate::export::ExportCrop;
use crate::page::PageSetup;

/// The export half of a profile: where the pages go when this target is
/// picked. Owned strings — unlike `export::PRINT_PRESETS` this is user
/// data that lives in the work file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProfileExport {
    /// Output resolution; 0 = the work's own (no resample). Downscale only —
    /// the never-upsample rule holds everywhere.
    pub dpi: u32,
    pub colour: LayerExpression,
    /// What rectangle leaves the building. Print targets want `TrimBleed`
    /// (the follow-up that makes paper-size presets real); web wants `Trim`
    /// (no printer ever sees the bleed) or `Paper`.
    #[serde(default)]
    pub crop: ExportCrop,
    /// Exact output height in px (web targets are speced in pixels, not
    /// dpi — the owner's own site takes 1991 px pages). 0 = off. Applied
    /// after the crop, wins over `dpi`, still never upsamples.
    #[serde(default)]
    pub px_height: u32,
    /// Spreads leave as two files (the submission norm) when true.
    pub split_spreads: bool,
}

/// A publisher/printer target on the work. `setup` is a full PageSetup —
/// picking a profile can restate paper/trim/bleed/safety wholesale (through
/// the page-size-change door, never by hand).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PublisherProfile {
    pub name: String,
    pub setup: PageSetup,
    pub binding_right: bool,
    /// Screen ruling the target prints well; preflight compares tone layers
    /// against it (M1's clash check gets a norm to check against). 0 = the
    /// target does not care.
    #[serde(default)]
    pub lpi: f32,
    /// Total page count must divide by this (offset printing's 台 rule —
    /// magazines take multiples of 8 or 16, doujin printers of 4). None =
    /// unchecked.
    #[serde(default)]
    pub page_count_multiple: Option<u8>,
    pub export: ProfileExport,
}

impl PublisherProfile {
    /// The built-in targets. Named after the SPEC each encodes, not a
    /// brand promise: the numbers are the published submission norms
    /// (600 dpi 1-bit mono interiors; 商業 B4 manuscript, doujin B5), and
    /// `note()` says which document each came from so nobody has to trust
    /// a bare preset.
    pub fn builtins() -> Vec<PublisherProfile> {
        let presets = PageSetup::presets();
        let find = |needle: &str| {
            presets
                .iter()
                .find(|p| p.name.contains(needle))
                .cloned()
                // Presets are compiled in; a rename here is a bug we want
                // loud in tests, but a fallback keeps release builds sane.
                .unwrap_or_else(|| presets[0].clone())
        };
        vec![
            PublisherProfile {
                name: "商業誌 投稿 (B4 manuscript, mono 600)".into(),
                setup: find("投稿"),
                binding_right: true,
                lpi: 60.0,
                page_count_multiple: None,
                export: ProfileExport {
                    dpi: 600,
                    colour: LayerExpression::Mono,
                    crop: ExportCrop::TrimBleed,
                    px_height: 0,
                    split_spreads: true,
                },
            },
            PublisherProfile {
                name: "同人誌 B5 (mono 600, ×4 pages)".into(),
                setup: find("同人誌"),
                binding_right: true,
                lpi: 60.0,
                page_count_multiple: Some(4),
                export: ProfileExport {
                    dpi: 600,
                    colour: LayerExpression::Mono,
                    crop: ExportCrop::TrimBleed,
                    px_height: 0,
                    split_spreads: true,
                },
            },
            PublisherProfile {
                name: "Web (trim crop, grey, fit height)".into(),
                setup: find("投稿"),
                binding_right: true,
                lpi: 0.0,
                page_count_multiple: None,
                export: ProfileExport {
                    dpi: 0,
                    colour: LayerExpression::Grey,
                    crop: ExportCrop::Trim,
                    // 2048 reads crisply on every current phone/tablet and
                    // stays under site upload caps; a judgment call like
                    // the 150 dpi web preset before it — owner-tunable in
                    // the dialog, recorded here for the same honesty.
                    px_height: 2048,
                    split_spreads: false,
                },
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip: a profile survives serde exactly, and an ABSENT profile
    /// (every work saved before this module existed) deserializes as None
    /// wherever `Option<PublisherProfile>` is serde-defaulted.
    #[test]
    fn profile_round_trips_and_absence_is_none() {
        for p in PublisherProfile::builtins() {
            let s = serde_json::to_string(&p).expect("serialize");
            let back: PublisherProfile = serde_json::from_str(&s).expect("parse");
            assert_eq!(back, p);
        }
        #[derive(Deserialize)]
        struct Holder {
            #[serde(default)]
            profile: Option<PublisherProfile>,
        }
        let h: Holder = serde_json::from_str("{}").expect("old shape");
        assert!(h.profile.is_none(), "old files load clean");
    }

    /// The builtins reference real presets — a PageSetup rename must fail
    /// HERE, not fall back silently in a release.
    #[test]
    fn builtins_reference_real_presets() {
        let names: Vec<String> = PageSetup::presets().iter().map(|p| p.name.clone()).collect();
        for b in PublisherProfile::builtins() {
            assert!(
                names.contains(&b.setup.name),
                "profile '{}' fell back — preset '{}' not found in {names:?}",
                b.name,
                b.setup.name
            );
        }
    }
}
