//! .abr import (TRIAGE 151): Photoshop brush sets → our presets.
//!
//! The reader (`mn_brush::abr`) gets the sampled tips out; this side parks
//! them where the existing systems already look, so nothing else changes:
//!
//! - each tip → `textures/<set>-<n>.png`, a square (padded, ≤1024) grayscale
//!   mask — the Tool Property texture picker lists it, `load_texture` loads it
//! - each tip → `imported/<set>-<n>.myb`, a plain preset with the tip as its
//!   texture — the brush picker lists group "imported", CSP-style (.abr
//!   import there also lands as a new tool group)
//!
//! Dynamics are NOT translated (CSP doesn't either): the preset is a
//! deliberately plain pressure-opaque brush whose dabs carry the tip's shape;
//! the owner retunes like any brush. Blank tips (some sets ship one) are
//! skipped. In a shipped build the root is `play/assets/brushes`, so the
//! owner's own imports live next to his exe, not in the repo.

use std::path::Path;

use crate::app::App;

/// Import every tip of an already-parsed set under `root`. Returns
/// (imported, blank-skipped). Public for the test; the App wrapper adds
/// status + rescans.
pub fn write_import(root: &Path, tips: &[mn_brush::AbrTip], set: &str) -> (usize, usize) {
    let _ = std::fs::create_dir_all(root.join("textures"));
    let _ = std::fs::create_dir_all(root.join("imported"));
    // Never overwrite an existing set. The slug truncates at 24 chars, so
    // "…Inkers vol 1" and "…Inkers vol 2" collide — and imported/ holds
    // presets the artist may have RETUNED since (the module's own workflow
    // says so). A colliding import suffixes -2, -3, … instead of silently
    // replacing thirty textures and the retunes with them.
    let taken = |s: &str| {
        let hit = |dir: &str| {
            std::fs::read_dir(root.join(dir)).is_ok_and(|rd| {
                rd.flatten().any(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with(&format!("{s}-"))
                })
            })
        };
        hit("imported") || hit("textures")
    };
    let set = if !taken(set) {
        set.to_string()
    } else {
        let mut n = 2usize;
        loop {
            let cand = format!("{set}-{n}");
            if !taken(&cand) {
                break cand;
            }
            n += 1;
        }
    };
    let set = set.as_str();
    let (mut imported, mut blank) = (0, 0);
    for (i, tip) in tips.iter().enumerate() {
        if !tip.gray.iter().any(|&v| v > 8) {
            blank += 1;
            continue;
        }
        let tex = format!("{set}-{}", i + 1);
        let mask = square_mask(&tip.gray, tip.width, tip.height);
        if mask
            .save(root.join("textures").join(format!("{tex}.png")))
            .is_err()
        {
            continue;
        }
        let preset = serde_json::json!({
            "comment": "MyPaint brush file",
            "name": tip.name,
            "group": "imported",
            "description": format!("Sampled tip imported from a Photoshop brush set ({} tips)", tips.len()),
            "mn-texture": tex,
            "mn-texture-scroll": 0.0,
            "settings": {
                // Native-size dabs: MyPaint radius = exp2(rlog), so the default
                // dab diameter matches the tip's long edge.
                "radius_logarithmic": {
                    "base_value": (tip.width.max(tip.height) as f32 / 2.0).log2().clamp(0.0, 9.0)
                },
                "opaque": { "base_value": 0.9 },
                "opaque_multiply": {
                    "base_value": 0.0,
                    "inputs": { "pressure": [[0.0, 0.0], [0.5, 0.5], [1.0, 0.9]] }
                },
                "hardness": { "base_value": 0.9 },
                "dabs_per_basic_radius": { "base_value": 6.0 },
                "dabs_per_actual_radius": { "base_value": 6.0 },
                "slow_tracking": { "base_value": 0.3 }
            },
            "version": 3
        });
        let myb = root.join("imported").join(format!("{tex}.myb"));
        if let Ok(text) = serde_json::to_string_pretty(&preset)
            && std::fs::write(&myb, text).is_ok()
        {
            imported += 1;
        }
    }
    (imported, blank)
}

/// Center the tip in a square canvas (the texture-mask contract: square,
/// ≤1024), downscaling over-long edges with a smooth filter — masks read
/// better bilinear than nearest.
fn square_mask(gray: &[u8], w: u32, h: u32) -> image::GrayImage {
    let src = image::GrayImage::from_raw(w, h, gray.to_vec()).expect("tip buffer matches dims");
    let long = w.max(h);
    let (tw, th) = if long > 1024 {
        let s = 1024.0 / long as f32;
        (
            ((w as f32 * s) as u32).max(1),
            ((h as f32 * s) as u32).max(1),
        )
    } else {
        (w, h)
    };
    let src = if (tw, th) != (w, h) {
        image::imageops::resize(&src, tw, th, image::imageops::FilterType::Triangle)
    } else {
        src
    };
    let size = tw.max(th);
    let mut out = image::GrayImage::new(size, size);
    image::imageops::overlay(
        &mut out,
        &src,
        ((size - tw) / 2) as i64,
        ((size - th) / 2) as i64,
    );
    out
}

/// `set` name for files: lowercase ascii, filesystem- and picker-safe.
fn set_slug(stem: &str) -> String {
    let s: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "abr".into()
    } else {
        s.chars().take(24).collect()
    }
}

impl App {
    /// Import a Photoshop .abr picked from the menu. Rescans presets and
    /// texture names so the new tips appear without a restart.
    pub fn import_abr(&mut self, path: &Path) {
        let tips = match mn_brush::parse_abr_file(path) {
            Ok(t) => t,
            Err(e) => return self.set_error(format!("abr import failed: {e}")),
        };
        if tips.is_empty() {
            return self.set_error("abr import: no sampled tips in that file");
        }
        let Some(root) = self.brushes_root.clone() else {
            return self.set_error("abr import: no brushes folder found");
        };
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (imported, blank) = write_import(&root, &tips, &set_slug(&stem));
        println!(
            "[abr] {}: {} tips, {imported} imported, {blank} blank skipped",
            path.display(),
            tips.len()
        );
        // The new files are inside the discovered root: rescan picks them up.
        self.presets = super::scan_presets();
        self.texture_names = super::scan_textures(self.brushes_root.as_deref());
        self.set_status(format!(
            "imported {imported} brush tips from {} (group \"imported\")",
            path.display()
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round trip through a synthetic set: PNG + .myb on disk, both loadable
    /// by the exact code paths the app uses (load_texture, preset scan).
    #[test]
    fn tips_become_textures_and_presets() {
        let dir = std::env::temp_dir().join(format!("mn-abr-{}", std::process::id()));
        let root = dir.join("brushes");
        std::fs::create_dir_all(&root).unwrap();

        let tips = vec![
            mn_brush::AbrTip {
                name: "Ink".into(),
                // A 4x2 tip inside a 2-wide notch: rows [255, 0] [255, 255].
                gray: vec![255, 0, 255, 255],
                width: 2,
                height: 2,
            },
            mn_brush::AbrTip {
                name: "Blank".into(),
                gray: vec![0; 4],
                width: 2,
                height: 2,
            },
        ];
        let (imported, blank) = write_import(&root, &tips, "myset");
        assert_eq!((imported, blank), (1, 1));

        // The mask loads through the real loader: square, padded, ink kept.
        let mask = mn_brush::load_texture(&root, "myset-1").expect("texture written");
        assert_eq!(mask.size, 2);
        // Centered blit of [255,0 / 255,255] onto 2x2 is the identity.
        assert_eq!(&mask.data[..], &[255, 0, 255, 255]);

        // The preset is valid .myb JSON pointing at that texture.
        let myb: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("imported/myset-1.myb")).unwrap(),
        )
        .unwrap();
        assert_eq!(myb["name"], "Ink");
        assert_eq!(myb["group"], "imported");
        assert_eq!(myb["mn-texture"], "myset-1");
        assert!(myb["settings"]["radius_logarithmic"]["base_value"].is_number());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn slug_is_safe_and_bounded() {
        assert_eq!(set_slug("My Brush Set!"), "my-brush-set");
        assert_eq!(set_slug("???"), "abr");
        assert_eq!(set_slug("ø").len() <= 24, true);
        assert!(set_slug(&"x".repeat(80)).len() <= 24);
    }

    /// The real vendored v6 set, end to end: parse → write_import → the
    /// masks come back through `load_texture` (the exact loader the brush
    /// engine uses). 31 tips, 1 blank skipped, first tip 567x701 → 701
    /// square (downscale path not taken under 1024).
    #[test]
    fn real_set_round_trips_through_the_loader() {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../brush/tests/data/abr_v6_sample.abr");
        let Ok(bytes) = std::fs::read(&fixture) else {
            return; // fixture not shipped: skip silently
        };
        let tips = mn_brush::parse_abr(&bytes, "sample").unwrap();
        let dir = std::env::temp_dir().join(format!("mn-abr-real-{}", std::process::id()));
        let root = dir.join("brushes");
        std::fs::create_dir_all(&root).unwrap();
        let (imported, blank) = write_import(&root, &tips, "sample");
        assert_eq!((imported, blank), (30, 1));
        let mask = mn_brush::load_texture(&root, "sample-1").expect("tip 1 written");
        assert_eq!(mask.size, 701); // 567x701 padded, not scaled
        assert!(mask.data.iter().any(|&v| v > 200));
        // The blank tip's slot imports nothing.
        assert!(mn_brush::load_texture(&root, "sample-11").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A 2000px-wide tip downscales to a 1024 square, centered.
    #[test]
    fn oversized_tip_downscales_into_the_square() {
        let dir = std::env::temp_dir().join(format!("mn-abr-big-{}", std::process::id()));
        let root = dir.join("brushes");
        std::fs::create_dir_all(&root).unwrap();
        let (w, h) = (2000u32, 1000u32);
        let gray = vec![200u8; (w * h) as usize];
        let tips = vec![mn_brush::AbrTip {
            name: "Wide".into(),
            gray,
            width: w,
            height: h,
        }];
        write_import(&root, &tips, "wide").1;
        let mask = mn_brush::load_texture(&root, "wide-1").expect("texture written");
        assert_eq!(mask.size, 1024);
        // Scaled height 512, centered: rows 0 and 1023 are padding (0),
        // the mid row carries ink.
        assert_eq!(mask.data[0], 0);
        assert_eq!(mask.data[(1023 * 1024) as usize], 0);
        assert!(mask.data[(512 * 1024 + 512) as usize] > 100);
        std::fs::remove_dir_all(&dir).ok();
    }
}
