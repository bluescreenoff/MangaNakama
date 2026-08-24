//! Krita `.kpp` preset → our engine: **dynamics only**.
//!
//! `mn_brush::parse_kpp_file` hands us a `KppPreset` — a name, a paint-op id
//! and the raw `<param>` map. This turns the handful of params whose meaning
//! is unambiguous into a `.myb` in group "imported", reusing `abr.rs`'s
//! writer so a Krita import lands in exactly the same place, with the same
//! honest-labelling convention (`mn.unmapped` + "Not translated: …"), as the
//! Photoshop and GIMP ones.
//!
//! **The tip is not in the file.** A `.kpp` references its brush tip as a
//! separate Krita resource (`brush_definition` names it; the bitmap lives in
//! the user's resource folder), so an import maps dynamics onto OUR default
//! round tip and says so in the description. That is the limitation the
//! module doc of `mn_brush::kpp` calls out, surfaced instead of hidden.
//!
//! What is translated, and nothing else:
//!
//! | Krita param | engine |
//! |---|---|
//! | `Spacing` (fraction of diameter), else the `spacing="…"` attribute of `brush_definition` | `dabs_per_actual_radius = 100 / (2 × pct)`, basic term zeroed — the same conversion `Interval::Percent` uses |
//! | `BrushSize`, else `Size` (px, plainly numeric) | `radius_logarithmic` base = `ln(d/2)` — the engine's radius is `exp(rlog)` |
//! | `PressureSize` on | `radius_logarithmic` pressure curve, sweeping down to a 10 % floor |
//! | `OpacityValue` (0..1) | `opaque` base |
//! | `FlowValue` (0..1) | `opaque_multiply` base (the ceiling of the pressure curve when there is one) |
//! | `PressureOpacity` / `PressureFlow` on | `opaque_multiply` pressure curve, 0 → flow |
//!
//! Deliberately NOT translated, because guessing would draw wrong quietly:
//! the pressure CURVE geometry (only a sensor's presence is read, never its
//! control points), any `…Sensor` blob, the mask generator's shape/softness,
//! `brush_definition`'s own scale and angle, and every paint-op that is not
//! `paintbrush` (Krita's Pixel Brush) — those import as a plain brush whose
//! note names the engine that was in the file.

use std::collections::BTreeMap;
use std::path::Path;

use mn_brush::KppPreset;
use serde_json::{Map, Value, json};

use super::abr::{
    ImportSummary, base_settings, free_slug, legacy_settings, rlog, set_base, spacing_settings,
    write_brush,
};

/// Krita's Pixel Brush — the only paint-op whose params we read.
const PIXEL_BRUSH: &str = "paintbrush";
/// Size when the preset states none plainly (Krita usually keeps it inside
/// the tip resource, which we do not have).
const DEFAULT_DIAMETER: f64 = 20.0;
/// Pressure cannot reach a 0 px dab through `exp()`: the sweep bottoms here.
const SIZE_FLOOR: f64 = 0.1;

const DESC: &str =
    "Imported from a Krita preset (dynamics only — Krita brush tips are separate resource files)";

/// Params this module reads. Anything else in the file is reported verbatim.
const READ: &[&str] = &[
    "Spacing",
    "BrushSize",
    "Size",
    "OpacityValue",
    "FlowValue",
    "PressureSize",
    "PressureOpacity",
    "PressureFlow",
];

/// Write one parsed Krita preset under `root` as a single `.myb`.
pub(super) fn write_kpp_import(root: &Path, preset: &KppPreset, set_name: &str) -> ImportSummary {
    let _ = std::fs::create_dir_all(root.join("imported"));
    let set = free_slug(root, set_name);

    let name = match preset.name.trim() {
        "" => set_name,
        n => n,
    };
    let is_pixel = preset.paintop_id == PIXEL_BRUSH;
    let (settings, notes) = if is_pixel {
        translate(&preset.params)
    } else {
        (
            legacy_settings(size_px(&preset.params).unwrap_or(DEFAULT_DIAMETER)),
            vec![format!(
                "Krita paint engine \"{}\" (imported as a plain brush)",
                preset.paintop_id
            )],
        )
    };

    let mut sum = ImportSummary::default();
    let ok = write_brush(
        root,
        "imported",
        &set,
        1,
        name,
        None,
        settings,
        Map::new(),
        DESC.to_string(),
        &notes,
    );
    sum.imported += ok as usize;
    if ok {
        sum.translated += (is_pixel && ok) as usize;
        sum.notes += notes.len();
    }
    sum
}

/// Pixel Brush params → libmypaint settings + the untranslated-params notes.
fn translate(params: &BTreeMap<String, String>) -> (Map<String, Value>, Vec<String>) {
    let mut notes = Vec::new();

    let diameter = match size_px(params) {
        Some(d) => d,
        None => {
            notes
                .push("brush size (no plainly numeric Size/BrushSize; the default is used)".into());
            DEFAULT_DIAMETER
        }
    };
    let mut s = base_settings(diameter);

    // -- spacing: a FRACTION of the diameter in Krita, a percent for us --
    if let Some(frac) = spacing_fraction(params) {
        spacing_settings(&mut s, frac * 100.0);
    }

    // -- size by pressure: presence only, never the curve's geometry --
    let mut radius = json!({ "base_value": rlog(diameter) });
    if flag(params, "PressureSize") {
        radius["inputs"]["pressure"] = json!([[0.0, SIZE_FLOOR.ln()], [1.0, 0.0]]);
    }
    s.insert("radius_logarithmic".into(), radius);

    // -- opacity (stroke alpha) and flow (per-dab) --
    let flow = unit(params, "FlowValue").unwrap_or(1.0);
    set_base(
        &mut s,
        "opaque",
        unit(params, "OpacityValue").unwrap_or(1.0),
    );
    if flag(params, "PressureOpacity") || flag(params, "PressureFlow") {
        s.insert(
            "opaque_multiply".into(),
            json!({ "base_value": 0.0, "inputs": { "pressure": [[0.0, 0.0], [1.0, flow]] } }),
        );
    } else {
        set_base(&mut s, "opaque_multiply", flow);
    }

    // -- everything else, verbatim by param name --
    for (k, v) in params {
        if READ.contains(&k.as_str()) {
            continue;
        }
        if k == "brush_definition" {
            notes.push("brush_definition (the tip itself — a separate Krita resource)".into());
        } else if !is_inert(v) {
            // An option Krita wrote as OFF was not dropped by us: saying so
            // would be dishonest in the other direction.
            notes.push(k.clone());
        }
    }
    (s, notes)
}

/// Brush diameter in px, if the preset states one plainly. The tip
/// resource's own diameter (and `brush_definition`'s scale) are NOT read:
/// compounding a scale we cannot verify would silently mis-size the brush.
fn size_px(params: &BTreeMap<String, String>) -> Option<f64> {
    ["BrushSize", "Size"]
        .iter()
        .find_map(|k| num(params, k))
        .filter(|d| (0.1..=10_000.0).contains(d))
}

/// Spacing as a fraction of the diameter: the top-level param, else the
/// `spacing="…"` attribute Krita writes on `brush_definition`'s `<Brush>`.
fn spacing_fraction(params: &BTreeMap<String, String>) -> Option<f64> {
    num(params, "Spacing")
        .or_else(|| xml_attr_num(params.get("brush_definition")?, "spacing"))
        .filter(|f| *f > 0.0 && f.is_finite())
}

fn num(params: &BTreeMap<String, String>, key: &str) -> Option<f64> {
    params.get(key)?.trim().parse::<f64>().ok()
}

fn unit(params: &BTreeMap<String, String>, key: &str) -> Option<f64> {
    num(params, key).map(|v| v.clamp(0.0, 1.0))
}

/// Is a curve option switched on? Krita writes these as a bool, and older
/// files as the sensor blob itself — accept both, and only both.
fn flag(params: &BTreeMap<String, String>, key: &str) -> bool {
    let Some(v) = params.get(key).map(|v| v.trim()) else {
        return false;
    };
    if let Ok(n) = v.parse::<f64>() {
        return n != 0.0;
    }
    if v.eq_ignore_ascii_case("true") {
        return true;
    }
    !v.eq_ignore_ascii_case("false") && v.to_ascii_lowercase().contains("pressure")
}

/// A param whose value is empty or a plain zero/false: an option that was
/// off in Krita, not something we failed to carry over.
fn is_inert(v: &str) -> bool {
    let v = v.trim();
    v.is_empty() || v.eq_ignore_ascii_case("false") || v.parse::<f64>().is_ok_and(|n| n == 0.0)
}

/// One plainly numeric `key="…"` attribute out of an XML blob. The match must
/// start a word so `spacing` never picks up `autoSpacingCoeff`.
fn xml_attr_num(blob: &str, key: &str) -> Option<f64> {
    let pat = format!("{key}=\"");
    let mut from = 0usize;
    while let Some(rel) = blob[from..].find(&pat) {
        let at = from + rel;
        let boundary = at == 0
            || blob[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_whitespace() || c == '<');
        let value = &blob[at + pat.len()..];
        if boundary {
            let end = value.find('"')?;
            return value[..end].trim().parse::<f64>().ok();
        }
        from = at + pat.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mn-kpp-{tag}-{}", std::process::id()));
        let root = dir.join("brushes");
        std::fs::create_dir_all(&root).unwrap();
        root
    }
    fn read_myb(root: &Path, slug: &str) -> Value {
        serde_json::from_str(
            &std::fs::read_to_string(root.join("imported").join(format!("{slug}.myb"))).unwrap(),
        )
        .unwrap()
    }
    fn preset(paintop: &str, params: &[(&str, &str)]) -> KppPreset {
        KppPreset {
            name: "Ink_gpen".into(),
            paintop_id: paintop.into(),
            params: params
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        }
    }
    fn notes_of(myb: &Value) -> Vec<String> {
        myb["mn"]["unmapped"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// NEW BEHAVIOR, pinned here: a Pixel Brush preset's spacing, size,
    /// pressure-size and opacity translate; a param we do not model is
    /// reported verbatim; an OFF option is not reported as lost.
    #[test]
    fn pixel_brush_dynamics_translate_and_the_rest_is_noted() {
        let root = tmp_root("dyn");
        let p = preset(
            "paintbrush",
            &[
                ("BrushSize", "40"),
                ("Spacing", "0.25"),
                ("PressureSize", "1"),
                ("OpacityValue", "0.8"),
                ("FlowValue", "1.0"),
                ("LightnessStrengthValue", "0.42"),
                ("PressureRotation", "0"),
            ],
        );
        let sum = write_kpp_import(&root, &p, "ink");
        assert_eq!((sum.imported, sum.translated), (1, 1));

        let myb = read_myb(&root, "ink-1");
        let s = &myb["settings"];
        assert_eq!(myb["name"], "Ink_gpen");
        assert_eq!(myb["group"], "imported");
        assert!(myb["mn-texture"].is_null(), "no tip comes with a .kpp");
        // Size 40 px → ln(20); pressure sweeps down to the 10 % floor.
        assert!(
            (s["radius_logarithmic"]["base_value"].as_f64().unwrap() - 20f64.ln()).abs() < 1e-9
        );
        let pr = &s["radius_logarithmic"]["inputs"]["pressure"];
        assert!((pr[0][1].as_f64().unwrap() - 0.1f64.ln()).abs() < 1e-9);
        assert_eq!(pr[1], json!([1.0, 0.0]));
        // Spacing 0.25 = 25 % → 100 / (2 × 25) = 2 dabs per actual radius.
        assert!((s["dabs_per_actual_radius"]["base_value"].as_f64().unwrap() - 2.0).abs() < 1e-9);
        assert_eq!(s["dabs_per_basic_radius"]["base_value"], 0.0);
        // Opacity is the stroke alpha; no pressure transfer → flat flow.
        assert!((s["opaque"]["base_value"].as_f64().unwrap() - 0.8).abs() < 1e-9);
        assert_eq!(s["opaque_multiply"]["base_value"], 1.0);
        // Honest notes: the unmodelled param verbatim, the OFF one silent.
        let notes = notes_of(&myb);
        assert!(
            notes.iter().any(|n| n == "LightnessStrengthValue"),
            "{notes:?}"
        );
        assert!(
            !notes.iter().any(|n| n.contains("PressureRotation")),
            "{notes:?}"
        );
        let desc = myb["description"].as_str().unwrap();
        assert!(desc.contains("dynamics only"), "{desc}");
        assert!(desc.contains("Not translated:"), "{desc}");
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    /// Pressure flow/opacity → the `opaque_multiply` curve, capped by flow;
    /// spacing also reads out of `brush_definition` (where Krita really puts
    /// it), and the tip it names is reported as not imported.
    #[test]
    fn pressure_transfer_and_brush_definition_spacing() {
        let root = tmp_root("flow");
        let p = preset(
            "paintbrush",
            &[
                ("BrushSize", "20"),
                ("FlowValue", "0.5"),
                ("PressureFlow", "true"),
                (
                    "brush_definition",
                    "<Brush autoSpacing=\"0\" autoSpacingCoeff=\"1\" spacing=\"0.1\" \
                     type=\"auto_brush\"/>",
                ),
            ],
        );
        assert_eq!(write_kpp_import(&root, &p, "flow").imported, 1);
        let myb = read_myb(&root, "flow-1");
        let s = &myb["settings"];
        assert_eq!(s["opaque_multiply"]["base_value"], 0.0);
        assert_eq!(
            s["opaque_multiply"]["inputs"]["pressure"],
            json!([[0.0, 0.0], [1.0, 0.5]])
        );
        // 0.1 = 10 % → 5 dabs per actual radius, NOT autoSpacingCoeff's 1.
        assert!((s["dabs_per_actual_radius"]["base_value"].as_f64().unwrap() - 5.0).abs() < 1e-9);
        assert!(
            notes_of(&myb)
                .iter()
                .any(|n| n.starts_with("brush_definition")),
            "the tip is a separate resource and must say so"
        );
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    /// Any other Krita paint engine imports as a plain brush whose note names
    /// the engine — we do not pretend its params mean what a Pixel Brush's do.
    #[test]
    fn other_paintops_import_plain_with_the_engine_named() {
        let root = tmp_root("smudge");
        let p = preset("colorsmudge", &[("BrushSize", "60"), ("SmudgeRate", "0.7")]);
        let sum = write_kpp_import(&root, &p, "smudge");
        assert_eq!((sum.imported, sum.translated, sum.notes), (1, 0, 1));
        let myb = read_myb(&root, "smudge-1");
        // The size it stated plainly still applies; nothing else does.
        assert!(
            (myb["settings"]["radius_logarithmic"]["base_value"]
                .as_f64()
                .unwrap()
                - 30f64.ln())
            .abs()
                < 1e-9
        );
        let notes = notes_of(&myb);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("colorsmudge"), "{notes:?}");
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    /// A preset with nothing we can read still becomes a usable brush at the
    /// default size and the default gap, rather than failing the import.
    #[test]
    fn empty_params_still_import_a_plain_brush() {
        let root = tmp_root("empty");
        let sum = write_kpp_import(&root, &preset("paintbrush", &[]), "empty");
        assert_eq!((sum.imported, sum.translated), (1, 1));
        let s = read_myb(&root, "empty-1")["settings"].clone();
        assert!(
            (s["radius_logarithmic"]["base_value"].as_f64().unwrap() - 10f64.ln()).abs() < 1e-9
        );
        assert!(s["radius_logarithmic"]["inputs"].is_null(), "no dynamics");
        assert_eq!(s["dabs_per_basic_radius"]["base_value"], 6.0, "default gap");
        assert_eq!(s["opaque_multiply"]["base_value"], 1.0);
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    /// The parser's own sample file, end to end: parse → .myb on disk.
    #[test]
    fn parsed_kpp_round_trips_into_a_preset() {
        let root = tmp_root("round");
        let xml = concat!(
            "<Preset paintopid=\"paintbrush\" name=\"Round\">",
            "<param name=\"BrushSize\" type=\"internal\">10.0</param>",
            "<param name=\"PressureOpacity\" type=\"internal\">1</param>",
            "</Preset>",
        );
        // A minimal PNG carrying the preset in a tEXt chunk.
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let chunk = |kind: &[u8; 4], data: &[u8]| {
            let mut v = (data.len() as u32).to_be_bytes().to_vec();
            v.extend_from_slice(kind);
            v.extend_from_slice(data);
            v.extend_from_slice(&[0; 4]);
            v
        };
        png.extend_from_slice(&chunk(b"IHDR", &[0u8; 13]));
        let mut text = b"preset\0".to_vec();
        text.extend_from_slice(xml.as_bytes());
        png.extend_from_slice(&chunk(b"tEXt", &text));
        png.extend_from_slice(&chunk(b"IEND", &[]));

        let p = mn_brush::parse_kpp(&png).expect("parses");
        assert_eq!(write_kpp_import(&root, &p, "round").imported, 1);
        let myb = read_myb(&root, "round-1");
        assert_eq!(myb["name"], "Round");
        assert_eq!(
            myb["settings"]["opaque_multiply"]["inputs"]["pressure"],
            json!([[0.0, 0.0], [1.0, 1.0]])
        );
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }
}
