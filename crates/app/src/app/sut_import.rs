//! Clip Studio `.sut` import: one exported sub tool → one preset, through
//! the same rails as `.abr`/`.gbr`/`.kpp` (`abr::write_brush`, the honest
//! `mn.unmapped` notes, group "imported").
//!
//! The parameter semantics are PORTED from the archive's `tools/cspmap.mjs`
//! — the converter that produced the shipped `csp/*.myb` presets, tuned and
//! eye-tested against the owner's real tools — with ONE deliberate
//! difference: cspmap baked the owner's global pen calibration
//! (`AdjustPressure.prgr`) into its curves because those presets REPLACED
//! his Clip Studio setup wholesale. An imported `.sut` has no calibration
//! file beside it, and this app feeds presets raw tablet pressure, so the
//! stored tool curve is used as-is. The mapping itself (all cspmap's, see
//! its header):
//!
//! - `radius_logarithmic` = ln(BrushSize / 2); a pressure-driven
//!   BrushSizeEffector becomes an ADDITIVE ln-space pressure mapping,
//!   midpoint-refined exactly like cspmap's sampler (a linear CSP curve is
//!   not linear in log space).
//! - `opaque` = Opacity% × BrushFlow% (both per-dab alpha in CSP);
//!   pressure on the Opacity/Flow effectors → an `opaque_multiply`
//!   pressure curve, otherwise `opaque_multiply` PINS to 1 (stock MyPaint
//!   would half-fade everything by pressure).
//! - `hardness` = BrushHardness / 100;
//!   `dabs_per_actual_radius` = 100 / (2 × BrushInterval), clamped 1..12.
//! - A tip PNG embedded in the file (MaterialFile) becomes a dab-anchored
//!   texture, like `.abr` sampled tips.
//!
//! Everything else that DRIVES (an effector enabled below 100 % minimum,
//! or a feature column set) is a "Not translated:" note, never a silent
//! difference.

use std::path::Path;

use mn_brush::sut::{SRC_PRESSURE, SRC_RANDOM, SRC_TILT, SRC_VELOCITY, SutBrush, SutEffector};
use serde_json::json;

use super::abr::{ImportSummary, base_settings, free_slug, rlog, spacing_settings, write_brush};

/// Import one parsed `.sut` under `root`.
pub fn write_sut_import(root: &Path, b: &SutBrush, set_name: &str) -> ImportSummary {
    let _ = std::fs::create_dir_all(root.join("textures"));
    let _ = std::fs::create_dir_all(root.join("imported"));
    let set_name = free_slug(root, set_name);
    let mut sum = ImportSummary::default();
    let mut notes: Vec<String> = Vec::new();
    let p = |k: &str, dflt: f64| b.params.get(k).copied().unwrap_or(dflt);

    let authored = p("BrushSize", 20.0).max(0.2);
    // Same default-size cap as .abr imports (abr::MAX_DEFAULT_PX): the
    // authored size is a note, the Size control still goes anywhere.
    let diameter = authored.min(super::abr::MAX_DEFAULT_PX);
    if authored > super::abr::MAX_DEFAULT_PX {
        notes.push(format!(
            "authored at {authored:.0} px (default capped at {:.0})",
            super::abr::MAX_DEFAULT_PX
        ));
    }
    let mut settings = base_settings(diameter);
    spacing_settings(
        &mut settings,
        p("BrushInterval", 10.0).clamp(100.0 / 24.0, 50.0),
    );
    super::abr::set_base(
        &mut settings,
        "hardness",
        (p("BrushHardness", 100.0) / 100.0).clamp(0.05, 1.0),
    );

    // -- size by pressure: additive ln-space mapping, midpoint-refined --
    let mut radius = json!({ "base_value": rlog(diameter) });
    if let Some(f) = pressure_factor(b.effectors.get("BrushSizeEffector")) {
        radius["inputs"]["pressure"] = json!(sample_ln(&f));
    }
    settings.insert("radius_logarithmic".into(), radius);

    // -- alpha: Opacity × Flow per dab; pressure only when CSP says so --
    let opacity = (p("Opacity", 100.0) / 100.0).clamp(0.0, 1.0);
    let flow = (p("BrushFlow", 100.0) / 100.0).clamp(0.0, 1.0);
    settings.insert("opaque".into(), json!({ "base_value": opacity * flow }));
    let op_f = pressure_factor(b.effectors.get("BrushOpacityEffector"));
    let fl_f = pressure_factor(b.effectors.get("BrushFlowEffector"));
    let alpha = match (op_f, fl_f) {
        (Some(a), Some(fb)) => Some(combine(a, fb)),
        (Some(a), None) | (None, Some(a)) => Some(a),
        (None, None) => None,
    };
    match alpha {
        Some(f) => {
            let pts: Vec<[f64; 2]> = sample_linear(&f);
            settings.insert(
                "opaque_multiply".into(),
                json!({ "base_value": 0.0, "inputs": { "pressure": pts } }),
            );
        }
        // cspmap's neutralisation: without it stock MyPaint half-fades
        // every stroke by pressure the file never asked for.
        None => {
            settings.insert("opaque_multiply".into(), json!({ "base_value": 1.0 }));
        }
    }

    // -- the honest remainder --
    for (col, label) in [
        ("BrushSprayDensityEffector", "spray"),
        ("BrushSpraySizeEffector", "spray"),
        ("DualSizeEffector", "dual brush"),
        ("DualFlowEffector", "dual brush"),
        ("TextureDensityEffector", "paper texture density"),
        ("BrushMixColorEffector", "colour mixing"),
        ("BrushMixAlphaEffector", "colour mixing"),
        ("BrushHueChangeEffector", "colour jitter"),
        ("BrushSaturationChangeEffector", "colour jitter"),
        ("BrushValueChangeEffector", "colour jitter"),
        ("BrushThicknessEffector", "tip thickness dynamics"),
        ("BrushBlurEffector", "edge blur dynamics"),
        ("BrushIntervalEffector", "spacing dynamics"),
    ] {
        if b.effectors.get(col).is_some_and(drives_any)
            && !notes.iter().any(|n| n == label)
        {
            notes.push(label.into());
        }
    }
    for (col, label) in [
        ("BrushSizeEffector", "size"),
        ("BrushOpacityEffector", "opacity"),
        ("BrushFlowEffector", "flow"),
    ] {
        if let Some(e) = b.effectors.get(col) {
            for (bit, src) in [
                (SRC_TILT, "tilt"),
                (SRC_VELOCITY, "speed"),
                (SRC_RANDOM, "random"),
            ] {
                if e.drives(bit) {
                    notes.push(format!("{label} by {src}"));
                }
            }
        }
    }
    if p("BrushWaterColor2", 0.0) > 0.0 || p("BrushUseWaterColor", 0.0) > 0.0 {
        notes.push("watercolour blending".into());
    }

    // -- tip material, when the file carries one --
    let tip_gray = b.tip_pngs.first().and_then(|png| {
        let img = image::load_from_memory(png).ok()?.to_luma8();
        Some((img.clone().into_raw(), img.width(), img.height()))
    });
    let mut extras = serde_json::Map::new();
    if tip_gray.is_some() {
        extras.insert("mn-texture-anchor".into(), json!("dab"));
    }

    let desc = "Imported from a Clip Studio sub tool (.sut)".to_string();
    let ok = write_brush(
        root,
        &set_name,
        1,
        &b.name,
        tip_gray.as_ref().map(|(g, w, h)| (g.as_slice(), *w, *h)),
        settings,
        extras,
        desc,
        &notes,
    );
    sum.imported += ok as usize;
    if ok {
        sum.translated += 1;
        sum.notes += notes.len();
    }
    sum
}

/// One pressure "factor" function: `min + (1-min) * curve(p)`, cspmap's
/// sourceFactor without the calibration pull-back (see the module doc).
struct Factor {
    min: f64,
    curve: Vec<(f64, f64)>,
}

fn pressure_factor(e: Option<&SutEffector>) -> Option<Factor> {
    let e = e?;
    if !e.drives(SRC_PRESSURE) {
        return None;
    }
    let curve = e
        .curve(SRC_PRESSURE)
        .map(<[(f64, f64)]>::to_vec)
        .unwrap_or_else(|| vec![(0.0, 0.0), (1.0, 1.0)]);
    Some(Factor {
        min: e.minimum(SRC_PRESSURE),
        curve,
    })
}

impl Factor {
    fn eval(&self, x: f64) -> f64 {
        self.min + (1.0 - self.min) * eval_curve(&self.curve, x)
    }
}

fn combine(a: Factor, b: Factor) -> Factor {
    // Two alpha factors multiply per dab; fold into one sampled curve.
    let pts: Vec<(f64, f64)> = (0..=8)
        .map(|i| {
            let x = f64::from(i) / 8.0;
            (x, a.eval(x) * b.eval(x))
        })
        .collect();
    Factor {
        min: 0.0,
        curve: pts,
    }
}

fn eval_curve(pts: &[(f64, f64)], x: f64) -> f64 {
    if pts.is_empty() {
        return x;
    }
    if x <= pts[0].0 {
        return pts[0].1;
    }
    for w in pts.windows(2) {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        if x <= x1 {
            return if x1 == x0 {
                y1
            } else {
                y0 + (y1 - y0) * (x - x0) / (x1 - x0)
            };
        }
    }
    pts[pts.len() - 1].1
}

/// Sample `ln(max(factor, 0.01))` into ≤12 mapping points, splitting the
/// worst mid-segment until ≤2 % off in log space (cspmap's sampler).
fn sample_ln(f: &Factor) -> Vec<[f64; 2]> {
    let g = |x: f64| f.eval(x).max(0.01).ln();
    refine(g)
}

fn sample_linear(f: &Factor) -> Vec<[f64; 2]> {
    let g = |x: f64| f.eval(x).clamp(0.0, 1.0);
    refine(g)
}

fn refine(g: impl Fn(f64) -> f64) -> Vec<[f64; 2]> {
    let mut xs = vec![0.0f64, 0.5, 1.0];
    while xs.len() < 12 {
        let (mut seg, mut err, mut at) = (usize::MAX, 0.0f64, 0.0f64);
        for i in 0..xs.len() - 1 {
            let mid = (xs[i] + xs[i + 1]) / 2.0;
            let e = (g(mid) - (g(xs[i]) + g(xs[i + 1])) / 2.0).abs();
            if e > err {
                (seg, err, at) = (i, e, mid);
            }
        }
        if err < 0.02 || seg == usize::MAX {
            break;
        }
        xs.insert(seg + 1, at);
    }
    let r = |v: f64| (v * 1e4).round() / 1e4;
    xs.iter().map(|&x| [r(x), r(g(x))]).collect()
}

fn drives_any(e: &SutEffector) -> bool {
    [SRC_PRESSURE, SRC_TILT, SRC_VELOCITY, SRC_RANDOM]
        .iter()
        .any(|&s| e.drives(s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::collections::BTreeMap;

    fn brush(params: &[(&str, f64)]) -> SutBrush {
        SutBrush {
            name: "Test Pen".into(),
            params: params
                .iter()
                .map(|(k, v)| (k.to_string(), *v))
                .collect::<BTreeMap<_, _>>(),
            effectors: BTreeMap::new(),
            tip_pngs: Vec::new(),
        }
    }
    fn read_myb(root: &Path, slug: &str) -> serde_json::Value {
        serde_json::from_str(
            &std::fs::read_to_string(root.join("imported").join(format!("{slug}.myb"))).unwrap(),
        )
        .unwrap()
    }
    fn tmp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mn-sut-{tag}-{}", std::process::id()));
        let root = dir.join("brushes");
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// The cspmap semantics hold: size in ln units, alpha = opacity×flow,
    /// spacing = 100/(2×interval) clamped, and — the subtle one —
    /// opaque_multiply PINNED to 1 when no effector drives pressure.
    #[test]
    fn plain_sut_maps_like_cspmap_and_pins_alpha() {
        let root = tmp_root("plain");
        let b = brush(&[
            ("BrushSize", 40.0),
            ("Opacity", 80.0),
            ("BrushFlow", 50.0),
            ("BrushHardness", 60.0),
            ("BrushInterval", 10.0),
        ]);
        let sum = write_sut_import(&root, &b, "airbrush");
        assert_eq!(sum.imported, 1);
        let myb = read_myb(&root, "airbrush-1");
        assert_eq!(myb["name"], "Test Pen");
        let s = &myb["settings"];
        assert!(
            (s["radius_logarithmic"]["base_value"].as_f64().unwrap() - 20f64.ln()).abs() < 1e-6
        );
        assert!((s["opaque"]["base_value"].as_f64().unwrap() - 0.4).abs() < 1e-9);
        assert!((s["hardness"]["base_value"].as_f64().unwrap() - 0.6).abs() < 1e-9);
        assert!(
            (s["dabs_per_actual_radius"]["base_value"].as_f64().unwrap() - 5.0).abs() < 1e-9
        );
        assert_eq!(
            s["opaque_multiply"]["base_value"], 1.0,
            "no pressure effector: alpha must be pinned, not half-faded"
        );
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    /// A pressure-driven size effector with a 30 % floor becomes an
    /// additive ln-space pressure mapping ending at 0 (full size), starting
    /// near ln(0.3); untranslated effectors become notes.
    #[test]
    fn pressure_size_translates_and_the_rest_is_noted() {
        let root = tmp_root("dyn");
        let mut b = brush(&[("BrushSize", 40.0), ("BrushInterval", 10.0)]);
        let eff = |min: u32, enabled: u32| SutEffector {
            enabled_mask: enabled,
            words: vec![44, 0xf0, enabled, min, 100, 100, 100, 0, 0, 0, 100],
            curves: vec![],
        };
        b.effectors
            .insert("BrushSizeEffector".into(), eff(30, SRC_PRESSURE));
        b.effectors
            .insert("BrushSprayDensityEffector".into(), eff(0, SRC_PRESSURE));
        // Flow: pressure min 20 % AND random min 40 % (words[3] and [6]) —
        // a source parked at 100 % would rightly not drive.
        let mut flow = eff(20, SRC_PRESSURE | SRC_RANDOM);
        flow.words[6] = 40;
        b.effectors.insert("BrushFlowEffector".into(), flow);
        let _ = write_sut_import(&root, &b, "pen");
        let myb = read_myb(&root, "pen-1");
        let s = &myb["settings"];
        let pts = s["radius_logarithmic"]["inputs"]["pressure"].as_array().unwrap();
        let first = pts.first().unwrap();
        let last = pts.last().unwrap();
        assert!((first[1].as_f64().unwrap() - 0.3f64.ln()).abs() < 0.02);
        assert!(last[1].as_f64().unwrap().abs() < 1e-6);
        // Flow by pressure → an opaque_multiply curve exists (not pinned).
        assert!(s["opaque_multiply"]["inputs"]["pressure"].is_array());
        let notes: Vec<String> = myb["mn"]["unmapped"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert!(notes.iter().any(|n| n == "spray"));
        assert!(notes.iter().any(|n| n == "flow by random"));
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    /// PATCHES.md #13 (local-only fixture; skip where absent): the owner's
    /// spotty-tip brush 不気味線 改 froze the app at the first dabs. The tip
    /// texture's hard black speckle made per-pixel ink/zero alternation, the
    /// C RLE dab-mask buffer overflowed its smooth-profile size and smashed
    /// the stack, and `end_atomic` then walked ~268 million garbage "dirty
    /// tiles" — the freeze — before the access violation. A CURVED stroke is
    /// load-bearing: a straight horizontal one leaves the speckle aligned
    /// well enough that the old bound happened to survive.
    #[test]
    fn spotty_sut_tip_strokes_without_queue_corruption() {
        use mn_core::{Document, PenSample, StrokeSink};
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../brush/tests/data");
        let Ok(bytes) = std::fs::read(dir.join("sut_freeze.sut")) else {
            return;
        };
        let b = mn_brush::sut::parse_sut(&bytes, "freeze").unwrap();
        let root = tmp_root("freeze");
        assert_eq!(write_sut_import(&root, &b, "freeze").imported, 1);
        let myb = read_myb(&root, "freeze-1");
        let mut brush =
            mn_brush::MyBrush::load(&root.join("imported").join("freeze-1.myb")).unwrap();
        let name = myb["mn-texture"].as_str().expect("tip texture imported");
        brush.set_texture(Some(mn_brush::load_texture(&root, name).expect("texture loads")));
        let mut doc = Document::new(1024, 1024);
        brush.begin(&mut doc);
        for i in 0..=300u32 {
            let a = f64::from(i) * 0.13;
            brush.sample(
                &mut doc,
                PenSample {
                    x: 450.0 + (a.sin() * 320.0) as f32,
                    y: 350.0 + ((a * 0.7).cos() * 250.0) as f32,
                    pressure: 0.6,
                    tilt_x: 0.0,
                    tilt_y: 0.0,
                    t_ms: f64::from(i) * 8.0,
                },
            );
        }
        brush.end(&mut doc);
        // Corruption showed up as ink queued absurdly far from the stroke;
        // a clean run stays inside the canvas the samples actually covered.
        let (x, y, w, h) = doc.active_layer().tile_bounds().expect("stroke painted");
        assert!(
            x >= -128 && y >= -128 && x + w as i32 <= 1152 && y + h as i32 <= 1152,
            "ink far outside the stroke area: bounds {:?}",
            (x, y, w, h)
        );
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    /// The REAL exports (local-only fixtures; skip where absent) import end
    /// to end with sane numbers.
    #[test]
    fn real_sut_files_import() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../brush/tests/data");
        let Ok(bytes) = std::fs::read(dir.join("sut_sample.sut")) else {
            return;
        };
        let b = mn_brush::sut::parse_sut(&bytes, "hard-airbrush").unwrap();
        let root = tmp_root("real");
        let sum = write_sut_import(&root, &b, "hard-airbrush");
        assert_eq!((sum.imported, sum.translated), (1, 1));
        let myb = read_myb(&root, "hard-airbrush-1");
        let r = myb["settings"]["radius_logarithmic"]["base_value"].as_f64().unwrap();
        assert!((r - 20f64.ln()).abs() < 1e-6, "Hard Airbrush is 40 px");
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }
}
