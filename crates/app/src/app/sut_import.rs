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
//! - `radius_logarithmic` = ln(diameter_px / 2), where diameter_px is
//!   BrushSize converted from its stored LENGTH unit (unit 0 = 1/100 mm,
//!   unit 2 = mm — see `write_sut_import`) at the document's dpi; a
//!   pressure-driven BrushSizeEffector becomes an ADDITIVE ln-space
//!   pressure mapping, midpoint-refined exactly like cspmap's sampler (a
//!   linear CSP curve is not linear in log space).
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
use mn_brush::todb::TobdTool;
use serde_json::json;

use super::abr::{
    ImportSummary, base_settings, free_slug, rlog, set_slug, spacing_settings, write_brush,
};

/// T5b: the whole Clip Studio tool database — one preset per LEAF sub
/// tool, grouped by the tool's first CSP group (`csp-pen`, `csp-airbrush`
/// …). Bitmap tips are NOT migrated on this path (v1): a tool with a
/// stamped tip keeps its stamp only through a `.sut` export, which the
/// caller's status says once rather than per tool.
pub fn write_todb_import(root: &Path, tools: &[TobdTool], doc_dpi: u32) -> ImportSummary {
    let mut sum = ImportSummary::default();
    for t in tools {
        let group = t
            .group_path
            .first()
            .cloned()
            .unwrap_or_else(|| "tools".into());
        let set = format!("csp-{}", set_slug(&group));
        let s = write_sut_import(root, &t.brush, &set, doc_dpi);
        sum.imported += s.imported;
        sum.blank += s.blank;
        sum.translated += s.translated;
        sum.notes += s.notes;
    }
    sum
}

/// Import one parsed `.sut` under `root`. `doc_dpi` is the importing
/// document's dpi — CSP's brush sizes are LENGTHS (see the unit table
/// below), so the px the brush lands at is relative to the paper, the
/// same conversion CSP's own Tool Property display does.
pub fn write_sut_import(root: &Path, b: &SutBrush, set_name: &str, doc_dpi: u32) -> ImportSummary {
    let _ = std::fs::create_dir_all(root.join("textures"));
    let _ = std::fs::create_dir_all(root.join("imported"));
    let set_name = free_slug(root, set_name);
    let mut sum = ImportSummary::default();
    let mut notes: Vec<String> = Vec::new();
    let p = |k: &str, dflt: f64| b.params.get(k).copied().unwrap_or(dflt);

    // BrushSizeUnit, established 2026-08-24 from the owner's own tool bank
    // (research/csp-tools.json + the Downloads .sut files): CSP stores
    // BrushSize as a LENGTH, never px. Unit 2 tools carry textbook-
    // millimetre fractions (0.75, 1.2, 8.3 — pens and markers); unit 0
    // tools carry integers exactly 100× their mm sizes (Mapping pen 30 =
    // 0.30 mm, the owner's Real G-Pen 100 = 1.0 mm, 不気味線 改 170 =
    // 1.7 mm). Reading unit-0 as px was the "how do sizes balloon" bug:
    // 1.7 mm became 170 px, the pressure→size curve rode it into the
    // rlog 6.2 clamp (a 985 px full-pressure dab — the original freeze).
    // Unknown unit codes stay px-literal with an honest note rather than
    // a guess.
    let dpi = if (30..=4800).contains(&doc_dpi) {
        doc_dpi as f64
    } else {
        600.0 // manga standard; a dpi-less startup canvas
    };
    let raw_size = p("BrushSize", 20.0);
    let (authored, size_desc) = match p("BrushSizeUnit", 0.0) as i64 {
        0 => {
            let mm = (raw_size / 100.0).max(0.002);
            let px = mm * dpi / 25.4;
            (px, format!("{mm:.2} mm ({px:.0} px at {dpi:.0} dpi)"))
        }
        2 => {
            let mm = raw_size.max(0.02);
            let px = mm * dpi / 25.4;
            (px, format!("{mm:.2} mm ({px:.0} px at {dpi:.0} dpi)"))
        }
        other => {
            notes.push(format!(
                "BrushSizeUnit {other} untranslated — size read as px"
            ));
            (raw_size.max(0.2), format!("{raw_size:.0} px"))
        }
    };
    let diameter = authored.min(super::abr::MAX_DEFAULT_PX);
    if authored > super::abr::MAX_DEFAULT_PX {
        notes.push(format!(
            "authored {size_desc} — default capped at {:.0} px",
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
        if b.effectors.get(col).is_some_and(drives_any) && !notes.iter().any(|n| n == label) {
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
        "imported",
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
    use std::collections::BTreeMap;
    use std::path::PathBuf;

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
        let sum = write_sut_import(&root, &b, "airbrush", 600);
        assert_eq!(sum.imported, 1);
        let myb = read_myb(&root, "airbrush-1");
        assert_eq!(myb["name"], "Test Pen");
        let s = &myb["settings"];
        // BrushSize 40, unit 0 (default) = 0.40 mm → 0.4 × 600/25.4 px.
        let d: f64 = 0.4 * 600.0 / 25.4;
        assert!(
            (s["radius_logarithmic"]["base_value"].as_f64().unwrap() - (d / 2.0).ln()).abs() < 1e-6
        );
        assert!((s["opaque"]["base_value"].as_f64().unwrap() - 0.4).abs() < 1e-9);
        assert!((s["hardness"]["base_value"].as_f64().unwrap() - 0.6).abs() < 1e-9);
        assert!((s["dabs_per_actual_radius"]["base_value"].as_f64().unwrap() - 5.0).abs() < 1e-9);
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
        let _ = write_sut_import(&root, &b, "pen", 600);
        let myb = read_myb(&root, "pen-1");
        let s = &myb["settings"];
        let pts = s["radius_logarithmic"]["inputs"]["pressure"]
            .as_array()
            .unwrap();
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
    /// C RLE dab-mask buffer overflowed its smooth-profile size and smashed    /// the stack, and `end_atomic` then walked ~268 million garbage "dirty
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
        assert_eq!(write_sut_import(&root, &b, "freeze", 600).imported, 1);
        let myb = read_myb(&root, "freeze-1");
        let mut brush =
            mn_brush::MyBrush::load(&root.join("imported").join("freeze-1.myb")).unwrap();
        let name = myb["mn-texture"].as_str().expect("tip texture imported");
        brush.set_texture(Some(
            mn_brush::load_texture(&root, name).expect("texture loads"),
        ));
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
        let sum = write_sut_import(&root, &b, "hard-airbrush", 600);
        assert_eq!((sum.imported, sum.translated), (1, 1));
        let myb = read_myb(&root, "hard-airbrush-1");
        let r = myb["settings"]["radius_logarithmic"]["base_value"]
            .as_f64()
            .unwrap();
        // Hard Airbrush: BrushSize 40, unit 0 = 0.40 mm → 0.4 × 600/25.4 px.
        let d: f64 = 0.4 * 600.0 / 25.4;
        assert!(
            (r - (d / 2.0).ln()).abs() < 1e-6,
            "Hard Airbrush is 0.4 mm ({d:.1} px at 600 dpi)"
        );
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    /// The unit table, established from the owner's tool bank: unit 0 =
    /// 1/100 mm, unit 2 = mm, anything else = px-literal with an honest
    /// note. A dpi-less document falls back to 600 (manga standard).
    #[test]
    fn sut_sizes_are_lengths_not_pixels() {
        let root = tmp_root("units");
        // 不気味線 改's numbers: 170 unit 0 = 1.7 mm = 40.2 px at 600.
        let b = brush(&[("BrushSize", 170.0), ("BrushSizeUnit", 0.0)]);
        write_sut_import(&root, &b, "u0", 600);
        let r0 = read_myb(&root, "u0-1")["settings"]["radius_logarithmic"]["base_value"]
            .as_f64()
            .unwrap();
        let d0: f64 = 1.7 * 600.0 / 25.4;
        assert!((r0 - (d0 / 2.0).ln()).abs() < 1e-6);

        // 薄墨's numbers: 8.3048 unit 2 = 8.30 mm = 196.2 px at 600.
        let b = brush(&[("BrushSize", 8.304816848703076), ("BrushSizeUnit", 2.0)]);
        write_sut_import(&root, &b, "u2", 600);
        let r2 = read_myb(&root, "u2-1")["settings"]["radius_logarithmic"]["base_value"]
            .as_f64()
            .unwrap();
        let d2: f64 = 8.304816848703076 * 600.0 / 25.4;
        assert!((r2 - (d2 / 2.0).ln()).abs() < 1e-6);

        // Same 1.7 mm at 72 dpi lands at 4.8 px — the paper-relative
        // semantic CSP's own Tool Property uses.
        let b = brush(&[("BrushSize", 170.0), ("BrushSizeUnit", 0.0)]);
        write_sut_import(&root, &b, "u0-72", 72);
        let r72 = read_myb(&root, "u0-72-1")["settings"]["radius_logarithmic"]["base_value"]
            .as_f64()
            .unwrap();
        let d72: f64 = 1.7 * 72.0 / 25.4;
        assert!((r72 - (d72 / 2.0).ln()).abs() < 1e-6);

        // dpi 0 (a dpi-less startup canvas) falls back to 600.
        let b = brush(&[("BrushSize", 170.0), ("BrushSizeUnit", 0.0)]);
        write_sut_import(&root, &b, "u0-fb", 0);
        let rfb = read_myb(&root, "u0-fb-1")["settings"]["radius_logarithmic"]["base_value"]
            .as_f64()
            .unwrap();
        assert!((rfb - r0).abs() < 1e-9);

        // Unknown unit: px-literal (yesterday's reading) + a note.
        let b = brush(&[("BrushSize", 50.0), ("BrushSizeUnit", 7.0)]);
        write_sut_import(&root, &b, "u7", 600);
        let myb = read_myb(&root, "u7-1");
        let r7 = myb["settings"]["radius_logarithmic"]["base_value"]
            .as_f64()
            .unwrap();
        assert!((r7 - 25f64.ln()).abs() < 1e-6, "unknown unit reads px");
        let notes: Vec<String> = myb["mn"]["unmapped"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert!(
            notes.iter().any(|n| n.starts_with("BrushSizeUnit 7")),
            "the honest note, got {notes:?}"
        );
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    /// The freeze brush itself (local-only fixture; skip where absent):
    /// 不気味線 改 — the brush whose misread 170-"px" size (actually
    /// 1.7 mm) rode the pressure curve into the 985 px rlog clamp and
    /// froze the app. It now imports at its paper size.
    #[test]
    fn the_freeze_brush_imports_at_paper_size() {
        let path = std::path::PathBuf::from(r"C:\Users\Max\Downloads\不気味線 改.sut");
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("[fixture] {} missing, skipping", path.display());
            return;
        };
        let b = mn_brush::sut::parse_sut(&bytes, "freeze").unwrap();
        assert_eq!(
            b.params.get("BrushSize").copied(),
            Some(170.0),
            "the fixture is the brush the unit table was derived from"
        );
        let root = tmp_root("paper");
        write_sut_import(&root, &b, "freeze", 600);
        let myb = read_myb(&root, "freeze-1");
        let r = myb["settings"]["radius_logarithmic"]["base_value"]
            .as_f64()
            .unwrap();
        let d: f64 = 1.7 * 600.0 / 25.4;
        assert!(
            (r - (d / 2.0).ln()).abs() < 1e-6,
            "1.7 mm at 600 dpi = {d:.1} px, not 170"
        );
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    /// T5b: the whole Clip Studio tool database imports — one preset per
    /// leaf sub tool, grouped by CSP group name (`csp-pen`, `csp-airbrush`),
    /// sizes converted as lengths. Local-only fixture; skip where absent.
    #[test]
    fn the_whole_tool_database_imports_grouped() {
        let db = Path::new(env!("CARGO_MANIFEST_DIR")).join("../brush/tests/data/todb_sample.todb");
        let Ok(tools) = mn_brush::todb::parse_todb_file(&db) else {
            eprintln!("[fixture] todb_sample.todb missing, skipping");
            return;
        };
        assert_eq!(tools.len(), 3);
        let root = tmp_root("todb");
        let sum = write_todb_import(&root, &tools, 600);
        assert_eq!(sum.imported, 3, "one preset per leaf sub tool");
        // Mapping pen, 0.30 mm at 600 dpi = 7.09 px, in the csp-pen set.
        let myb = read_myb(&root, "csp-pen-1");
        assert_eq!(myb["name"], "Mapping pen");
        let d: f64 = 0.3 * 600.0 / 25.4;
        let r = myb["settings"]["radius_logarithmic"]["base_value"]
            .as_f64()
            .unwrap();
        assert!(
            (r - (d / 2.0).ln()).abs() < 1e-6,
            "paper-relative size, got {r}"
        );
        // The airbrush set exists beside it, its own first preset.
        let air = read_myb(&root, "csp-airbrush-1");
        assert_eq!(air["name"], "Hard Airbrush");
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }
}
