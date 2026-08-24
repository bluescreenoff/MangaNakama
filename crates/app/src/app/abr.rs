//! Brush-set import: Photoshop `.abr` (tips + dynamics) and GIMP
//! `.gbr`/`.gih` → our presets.
//!
//! The readers (`mn_brush::abr`, `abr_desc`, `gbr`) get the data out; this
//! side turns it into files the existing systems already look at:
//!
//! - each tip → `textures/<set>-<n>.png`, a square (padded, ≤1024) grayscale
//!   mask — the Tool Property texture picker lists it, `load_texture` loads it
//! - each brush → `imported/<set>-<n>.myb` in group "imported"
//!
//! **Dynamics are translated as far as they honestly map** (the ROADMAP's
//! "faithful brush imports"):
//!
//! - static tip geometry (flip X/Y, angle, roundness) is BAKED into the
//!   texture bitmap — for a static value that is exact, not an approximation
//! - spacing % of diameter → `dabs_per_actual_radius` (the same conversion
//!   `Interval::Percent` uses: `100 / (2 × interval)`)
//! - size by pen pressure (with minimum diameter) and size jitter →
//!   `radius_logarithmic` pressure/random input curves (offsets in ln units:
//!   the engine's radius is `exp(radius_logarithmic)`)
//! - opacity/flow by pressure → an `opaque_multiply` pressure curve;
//!   opacity jitter → an `opaque` random curve
//! - scatter % of diameter → `mn-scatter` (radius-relative, ×2), dab count →
//!   dab density
//! - computed (parametric round) brushes — previously dropped entirely —
//!   become real presets: hardness, roundness (`elliptical_dab_ratio`) and
//!   angle (`elliptical_dab_angle`) are all engine settings
//!
//! **What cannot map says so** instead of silently drawing differently:
//! every untranslatable dynamic (fade/tilt/direction controllers, per-dab
//! angle jitter — our texture tips do not rotate per dab — wet edges,
//! airbrush build-up, dual brush, color dynamics, pattern texture) becomes a
//! human-readable line in the preset's description AND an entry in the
//! `.myb`'s `mn.unmapped` list, the same convention the CSP import uses.
//!
//! The tip's default size prefers the `desc` diameter (the size the brush
//! was saved at); without dynamics it is the tip's TIGHT ink bounding box —
//! not the padded bitmap, which imported extreme-aspect tips oversized (the
//! ROADMAP wart), and in ln units — not log2, which oversized EVERY tip
//! (radius is `exp(rlog)`; the old `.log2()` import drew a 128 px tip ~7×
//! too big).
//!
//! In a shipped build the root is `play/assets/brushes`, so the owner's own
//! imports live next to his exe, not in the repo.

use std::path::Path;

use mn_brush::{AbrPresetInfo, AbrSet, BrushKind, Control, DynGroup, GbrBrush};
use serde_json::{Value, json};

use crate::app::App;

/// What an import did, for the status line and the log.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ImportSummary {
    /// Presets written (sampled + computed).
    pub imported: usize,
    /// Tips skipped because they carry no ink.
    pub blank: usize,
    /// Presets whose Photoshop dynamics were translated (a `desc` entry was
    /// found and applied).
    pub translated: usize,
    /// Total "could not translate" notes written into preset descriptions.
    pub notes: usize,
}

/// Ink threshold: below this a mask pixel counts as empty (matches the old
/// blank-tip check).
const INK: u8 = 8;

/// Import a parsed `.abr` set (tips + dynamics) under `root`.
///
/// Every `desc` preset that resolves to a tip (or is computed) becomes a
/// brush; tips no preset references import plain, so a v1/v2 file — or a v6
/// file whose `desc` failed to parse — still yields its tips.
pub fn write_import(root: &Path, set: &AbrSet, set_name: &str) -> ImportSummary {
    let _ = std::fs::create_dir_all(root.join("textures"));
    let _ = std::fs::create_dir_all(root.join("imported"));
    let set_name = free_slug(root, set_name);
    let set_name = set_name.as_str();

    let mut sum = ImportSummary::default();
    let mut tip_used = vec![false; set.tips.len()];
    let mut n = 0usize; // file index, shared by every brush of the set

    for info in &set.presets {
        match &info.kind {
            BrushKind::Sampled { sample_id } => {
                let Some(ti) = set
                    .tips
                    .iter()
                    .position(|t| t.sample_id.as_deref() == Some(sample_id.as_str()))
                else {
                    continue; // preset references a tip the samp walk lost
                };
                tip_used[ti] = true;
                let tip = &set.tips[ti];
                n += 1;
                write_sampled(root, set_name, n, tip, Some(info), set.tips.len(), &mut sum);
            }
            BrushKind::Computed { hardness_pct } => {
                n += 1;
                write_computed(root, set_name, n, info, *hardness_pct, &mut sum);
            }
        }
    }
    // Tips nothing referenced (v1/v2, missing desc, or dropped entries).
    for (ti, tip) in set.tips.iter().enumerate() {
        if !tip_used[ti] {
            n += 1;
            write_sampled(root, set_name, n, tip, None, set.tips.len(), &mut sum);
        }
    }
    sum
}

/// Import GIMP brushes: no dynamics beyond spacing, by format.
pub fn write_gimp_import(root: &Path, brushes: &[GbrBrush], set_name: &str) -> ImportSummary {
    let _ = std::fs::create_dir_all(root.join("textures"));
    let _ = std::fs::create_dir_all(root.join("imported"));
    let set_name = free_slug(root, set_name);
    let mut sum = ImportSummary::default();
    for (i, b) in brushes.iter().enumerate() {
        let Some((gray, w, h)) = tight_crop(&b.gray, b.width, b.height) else {
            sum.blank += 1;
            continue;
        };
        let mut settings = base_settings(w.max(h) as f64);
        if b.spacing_pct >= 1 {
            spacing_settings(&mut settings, b.spacing_pct as f64);
        }
        let ok = write_brush(
            root,
            "imported",
            &set_name,
            i + 1,
            &b.name,
            Some((&gray, w, h)),
            settings,
            serde_json::Map::new(),
            "Imported from a GIMP brush (.gbr/.gih)".into(),
            &[],
        );
        sum.imported += ok as usize;
    }
    sum
}

/// One sampled tip → texture + preset. `info` present = dynamics translated.
fn write_sampled(
    root: &Path,
    set: &str,
    n: usize,
    tip: &mn_brush::AbrTip,
    info: Option<&AbrPresetInfo>,
    tips_total: usize,
    sum: &mut ImportSummary,
) {
    // Tight ink bounds first (the padded-bbox wart), then bake the static
    // geometry the dynamics describe.
    let Some((gray, w, h)) = tight_crop(&tip.gray, tip.width, tip.height) else {
        sum.blank += 1;
        return;
    };
    let (gray, w, h) = match info {
        Some(i) => bake_geometry(gray, w, h, i.flip_x, i.flip_y, i.roundness_pct, i.angle_deg),
        None => (gray, w, h),
    };

    let natural = w.max(h) as f64;
    let (settings, extras, notes, translated) = match info {
        Some(i) => {
            // The desc diameter is honest but a terrible DEFAULT past a
            // point: Painter-style sets author kilo-pixel tips, and a brush
            // that selects at 985 px reads as broken (owner's eye test).
            // The preset size is only a default — the ladder still goes
            // anywhere — so cap it and say so.
            let authored = i.diameter_px.unwrap_or(natural);
            let (s, extras, mut notes) = translate(i, authored.min(MAX_DEFAULT_PX));
            if authored > MAX_DEFAULT_PX {
                notes.push(format!(
                    "authored at {authored:.0} px (default capped at {MAX_DEFAULT_PX:.0})"
                ));
            }
            (s, extras, notes, true)
        }
        None => (
            legacy_settings(natural),
            serde_json::Map::new(),
            Vec::new(),
            false,
        ),
    };

    let name = info
        .and_then(|i| i.name.clone())
        .unwrap_or_else(|| tip.name.clone());
    let desc = format!("Sampled tip imported from a Photoshop brush set ({tips_total} tips)");
    let ok = write_brush(
        root,
        "imported",
        set,
        n,
        &name,
        Some((&gray, w, h)),
        settings,
        extras,
        desc,
        &notes,
    );
    sum.imported += ok as usize;
    if ok && translated {
        sum.translated += 1;
        sum.notes += notes.len();
    }
}

/// One computed (parametric round) brush → preset with no texture.
fn write_computed(
    root: &Path,
    set: &str,
    n: usize,
    info: &AbrPresetInfo,
    hardness_pct: f64,
    sum: &mut ImportSummary,
) {
    // Same default-size cap as sampled tips (a 2000 px round brush is a
    // legal file and a useless default).
    let authored = info.diameter_px.unwrap_or(20.0);
    let (mut settings, extras, mut notes) = translate(info, authored.min(MAX_DEFAULT_PX));
    if authored > MAX_DEFAULT_PX {
        notes.push(format!(
            "authored at {authored:.0} px (default capped at {MAX_DEFAULT_PX:.0})"
        ));
    }
    set_base(&mut settings, "hardness", (hardness_pct / 100.0).clamp(0.05, 1.0));
    // Roundness R% = the dab squashed to R% of its diameter → engine ratio.
    let ratio = (100.0 / info.roundness_pct).clamp(1.0, 10.0);
    if ratio > 1.0 {
        set_base(&mut settings, "elliptical_dab_ratio", ratio);
        // Photoshop measures the angle counter-clockwise, the engine
        // clockwise ("45.0 … turned clockwise"): negate, wrap to 0..180.
        set_base(
            &mut settings,
            "elliptical_dab_angle",
            (-info.angle_deg).rem_euclid(180.0),
        );
    }
    let name = info.name.clone().unwrap_or_else(|| format!("{set}-round-{n}"));
    let desc = "Round brush imported from a Photoshop brush set".to_string();
    let ok = write_brush(root, "imported", set, n, &name, None, settings, extras, desc, &notes);
    sum.imported += ok as usize;
    if ok {
        sum.translated += 1;
        sum.notes += notes.len();
    }
}

/// Write one texture (optional) + preset pair into `root/<group>/`. Returns
/// success. `group` is both the subdirectory and the preset's `"group"`
/// field: "imported" for every file import, "mine" for tips captured off
/// the canvas (Edit ▸ Register selection as brush tip).
#[allow(clippy::too_many_arguments)] // a builder for one call site is noise
pub(super) fn write_brush(
    root: &Path,
    group: &str,
    set: &str,
    n: usize,
    name: &str,
    mask: Option<(&[u8], u32, u32)>,
    settings: serde_json::Map<String, Value>,
    extras: serde_json::Map<String, Value>,
    mut description: String,
    notes: &[String],
) -> bool {
    let _ = std::fs::create_dir_all(root.join(group));
    if mask.is_some() {
        let _ = std::fs::create_dir_all(root.join("textures"));
    }
    let slug = format!("{set}-{n}");
    let mut preset = json!({
        "comment": "MyPaint brush file",
        "name": name,
        "group": group,
        "settings": settings,
        "version": 3
    });
    for (k, v) in extras {
        preset[k] = v;
    }
    if let Some((gray, w, h)) = mask {
        let img = square_mask(gray, w, h);
        if img
            .save(root.join("textures").join(format!("{slug}.png")))
            .is_err()
        {
            return false;
        }
        preset["mn-texture"] = json!(slug);
        preset["mn-texture-scroll"] = json!(0.0);
    }
    if !notes.is_empty() {
        // The same honesty convention the CSP import uses (`mn.unmapped`,
        // read back by MyBrush for hints) — plus the human-readable line.
        preset["mn"] = json!({ "unmapped": notes });
        description.push_str(". Not translated: ");
        description.push_str(&notes.join("; "));
    }
    preset["description"] = json!(description);
    let path = root.join(group).join(format!("{slug}.myb"));
    serde_json::to_string_pretty(&preset)
        .ok()
        .and_then(|text| std::fs::write(&path, text).ok())
        .is_some()
}

// ---------------------------------------------------------------------------
// Dynamics → .myb settings
// ---------------------------------------------------------------------------

/// The engine's radius is `exp(radius_logarithmic)` — ln, NOT log2.
/// The largest DEFAULT size an import may select at. Only the default: the
/// authored size is preserved in a note and the Size control goes anywhere.
pub(super) const MAX_DEFAULT_PX: f64 = 300.0;

pub(super) fn rlog(diameter_px: f64) -> f64 {
    (diameter_px / 2.0).max(0.5).ln().clamp(-2.0, 6.2)
}

pub(super) fn set_base(s: &mut serde_json::Map<String, Value>, key: &str, v: f64) {
    s.insert(key.into(), json!({ "base_value": v }));
}

/// Settings shared by every import: size and a mild stabilizer.
pub(super) fn base_settings(diameter_px: f64) -> serde_json::Map<String, Value> {
    let mut s = serde_json::Map::new();
    set_base(&mut s, "radius_logarithmic", rlog(diameter_px));
    set_base(&mut s, "hardness", 0.9);
    set_base(&mut s, "slow_tracking", 0.3);
    // Default gap; spacing_settings overrides when the file says so.
    set_base(&mut s, "dabs_per_basic_radius", 6.0);
    set_base(&mut s, "dabs_per_actual_radius", 6.0);
    s
}

/// The pre-dynamics import behavior, for tips with no `desc` entry: a plain
/// pressure-opacity brush the artist retunes.
pub(super) fn legacy_settings(diameter_px: f64) -> serde_json::Map<String, Value> {
    let mut s = base_settings(diameter_px);
    set_base(&mut s, "opaque", 0.9);
    s.insert(
        "opaque_multiply".into(),
        json!({
            "base_value": 0.0,
            "inputs": { "pressure": [[0.0, 0.0], [0.5, 0.5], [1.0, 0.9]] }
        }),
    );
    s
}

/// Spacing % of tip diameter → dab density, exactly as `Interval::Percent`
/// converts it (`dabs_per_actual_radius = 100 / (2 × interval)`; the basic
/// term is zeroed so the gap tracks the live dab).
pub(super) fn spacing_settings(s: &mut serde_json::Map<String, Value>, pct: f64) {
    let dabs = (100.0 / (2.0 * pct.clamp(1.0, 1000.0))).clamp(0.05, 50.0);
    set_base(s, "dabs_per_actual_radius", dabs);
    set_base(s, "dabs_per_basic_radius", 0.0);
}

/// Translate one `desc` preset's dynamics. Returns the libmypaint settings,
/// the top-level `.myb` keys (the `mn-*` engine modes), and the honest
/// "could not translate" notes.
type Translated = (
    serde_json::Map<String, Value>,
    serde_json::Map<String, Value>,
    Vec<String>,
);

fn translate(info: &AbrPresetInfo, diameter_px: f64) -> Translated {
    let mut s = base_settings(diameter_px);
    let mut extras = serde_json::Map::new();
    let mut notes = Vec::new();
    let note_control = |what: &str, g: &DynGroup, notes: &mut Vec<String>| {
        if !matches!(g.control, Control::Off | Control::Pressure) {
            notes.push(format!("{what} by {}", g.control.label()));
        }
    };

    // -- spacing --
    match info.spacing_pct {
        Some(pct) => spacing_settings(&mut s, pct),
        None => notes.push("spacing off in Photoshop (per-event stamping); default gap used".into()),
    }

    // -- size dynamics --
    let mut radius = json!({ "base_value": rlog(diameter_px) });
    if info.size.control == Control::Pressure {
        // Pressure sweeps the diameter from the minimum up to 100 % — an ln
        // offset below the base. A 0 % minimum cannot exist at exp(): floor
        // at 5 %.
        let min = (info.minimum_diameter_pct.clamp(5.0, 100.0)) / 100.0;
        radius["inputs"]["pressure"] = json!([[0.0, min.ln()], [1.0, 0.0]]);
    }
    note_control("size", &info.size, &mut notes);
    if info.size.jitter_pct > 0.0 {
        // Jitter shrinks the dab at random, down to (100 − J) %.
        let low = ((100.0 - info.size.jitter_pct).clamp(5.0, 100.0)) / 100.0;
        radius["inputs"]["random"] = json!([[0.0, low.ln()], [1.0, 0.0]]);
    }
    s.insert("radius_logarithmic".into(), radius);

    // -- tip anchoring + angle dynamics. Photoshop stamps its tip PER DAB,
    //    and since PATCHES.md #10 amendment 2 so can the engine: sampled
    //    imports anchor to the dab (the faithful behaviour; hand-made
    //    presets keep the canvas-grain default). Direction-controlled
    //    angle translates for real — the stamp turns with the stroke.
    //    Static angle/flips/roundness are baked into the bitmap, and a
    //    live direction rotation composes with the baked base exactly as
    //    Photoshop composes them. --
    if matches!(info.kind, BrushKind::Sampled { .. }) {
        extras.insert("mn-texture-anchor".into(), json!("dab"));
    }
    if info.angle_dyn.control == Control::Direction {
        extras.insert("mn-texture-rotate".into(), json!("direction"));
        if info.angle_dyn.jitter_pct > 0.0 {
            notes.push(format!(
                "angle jitter {}%",
                info.angle_dyn.jitter_pct
            ));
        }
    } else if info.angle_dyn.is_active() {
        notes.push(format!("per-dab angle ({})", dyn_label(&info.angle_dyn)));
    }
    if info.roundness_dyn.is_active() {
        notes.push(format!(
            "per-dab roundness ({})",
            dyn_label(&info.roundness_dyn)
        ));
    }

    // -- scatter --
    if info.scatter.jitter_pct > 0.0 {
        // % of diameter → radius-relative (×2), engine cap 4 (= 200 %).
        // `mn-scatter` is a TOP-LEVEL .myb key (like the other mn-* modes),
        // not a libmypaint setting.
        let scatter = info.scatter.jitter_pct / 50.0;
        if scatter > 4.0 {
            notes.push(format!(
                "scatter {}% clamped to the engine's 200%",
                info.scatter.jitter_pct
            ));
        }
        extras.insert("mn-scatter".into(), json!(scatter.min(4.0)));
        note_control("scatter amount", &info.scatter, &mut notes);
    }
    if info.count > 1.0 {
        // N stamps per interval, approximated as N× dab density.
        if let Some(d) = s
            .get("dabs_per_actual_radius")
            .and_then(|v| v["base_value"].as_f64())
        {
            set_base(
                &mut s,
                "dabs_per_actual_radius",
                (d * info.count).min(50.0),
            );
        }
        notes.push(format!(
            "scatter count {} approximated as dab density",
            info.count
        ));
    }
    if info.count_dyn.is_active() {
        notes.push(format!("count dynamics ({})", dyn_label(&info.count_dyn)));
    }

    // -- transfer (opacity + flow) --
    let pressure_opacity = info.flow.control == Control::Pressure
        || info.opacity.control == Control::Pressure;
    if pressure_opacity {
        s.insert(
            "opaque_multiply".into(),
            json!({ "base_value": 0.0, "inputs": { "pressure": [[0.0, 0.0], [1.0, 1.0]] } }),
        );
    } else {
        // No pressure transfer: Photoshop stamps at full flow.
        set_base(&mut s, "opaque_multiply", 1.0);
    }
    let mut opaque = json!({ "base_value": 1.0 });
    if info.opacity.jitter_pct > 0.0 {
        let low = -(info.opacity.jitter_pct.min(100.0)) / 100.0;
        opaque["inputs"]["random"] = json!([[0.0, low], [1.0, 0.0]]);
    }
    s.insert("opaque".into(), opaque);
    if info.flow.jitter_pct > 0.0 && info.opacity.jitter_pct == 0.0 {
        notes.push("flow jitter (merged into opacity would double-count)".into());
    }
    note_control("opacity", &info.opacity, &mut notes);
    note_control("flow", &info.flow, &mut notes);

    // -- flatly unmodeled features --
    if info.wet_edges {
        notes.push("wet edges".into());
    }
    if info.airbrush {
        notes.push("airbrush build-up".into());
    }
    for u in &info.unmodeled {
        notes.push((*u).into());
    }

    (s, extras, notes)
}

/// "pen pressure, 25% jitter" — one line for a group's whole story.
fn dyn_label(g: &DynGroup) -> String {
    match (g.control, g.jitter_pct > 0.0) {
        (Control::Off, _) => format!("{}% jitter", g.jitter_pct),
        (c, false) => c.label(),
        (c, true) => format!("{}, {}% jitter", c.label(), g.jitter_pct),
    }
}

// ---------------------------------------------------------------------------
// Mask geometry
// ---------------------------------------------------------------------------

/// Crop to the tight ink bounding box. `None` = blank tip.
pub(super) fn tight_crop(gray: &[u8], w: u32, h: u32) -> Option<(Vec<u8>, u32, u32)> {
    let (w, h) = (w as usize, h as usize);
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            if gray[y * w + x] > INK {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if x0 > x1 {
        return None;
    }
    let (cw, ch) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut out = vec![0u8; cw * ch];
    for y in 0..ch {
        out[y * cw..(y + 1) * cw].copy_from_slice(&gray[(y0 + y) * w + x0..(y0 + y) * w + x0 + cw]);
    }
    Some((out, cw as u32, ch as u32))
}

/// Bake the static tip geometry Photoshop stores as numbers: flips, then the
/// roundness squash (scale the local Y axis to R %), then the base angle
/// (counter-clockwise, Photoshop's convention). Exact for static values —
/// the per-dab dynamics on the same axes are what the notes call
/// untranslatable.
fn bake_geometry(
    mut gray: Vec<u8>,
    w: u32,
    h: u32,
    flip_x: bool,
    flip_y: bool,
    roundness_pct: f64,
    angle_deg: f64,
) -> (Vec<u8>, u32, u32) {
    let (wi, hi) = (w as usize, h as usize);
    if flip_x {
        for row in gray.chunks_exact_mut(wi) {
            row.reverse();
        }
    }
    if flip_y {
        let (mut a, mut b) = (0, hi.saturating_sub(1));
        while a < b {
            for x in 0..wi {
                gray.swap(a * wi + x, b * wi + x);
            }
            a += 1;
            b -= 1;
        }
    }
    let squash = (roundness_pct / 100.0).clamp(0.01, 1.0);
    let angle = angle_deg.to_radians();
    if squash >= 0.999 && angle.abs() < 1e-3 {
        return (gray, w, h);
    }
    // Forward map: scale Y by `squash`, rotate by −angle in image space
    // (image Y points down, so a CCW art rotation is a CW image one).
    let (sin, cos) = (-angle).sin_cos();
    let (fw, fh) = (w as f64, h as f64 * squash);
    // Output bounds from the transformed corners.
    let (mut ow, mut oh) = (0f64, 0f64);
    for (cx, cy) in [(fw, fh), (fw, -fh)] {
        ow = ow.max((cx * cos - cy * sin).abs());
        oh = oh.max((cx * sin + cy * cos).abs());
    }
    // Trim the float epsilon a trig identity leaves (2.0000000000000004
    // must not ceil to 3).
    let (ow, oh) = (
        (((ow - 1e-6).ceil() as u32).max(1)),
        (((oh - 1e-6).ceil() as u32).max(1)),
    );
    let (ocx, ocy) = (ow as f64 / 2.0, oh as f64 / 2.0);
    let (icx, icy) = (w as f64 / 2.0, h as f64 / 2.0);
    let mut out = vec![0u8; (ow * oh) as usize];
    for oy in 0..oh {
        for ox in 0..ow {
            // Inverse map: un-rotate, un-squash, sample bilinear.
            let (dx, dy) = (ox as f64 + 0.5 - ocx, oy as f64 + 0.5 - ocy);
            let (rx, ry) = (dx * cos + dy * sin, -dx * sin + dy * cos);
            let (sx, sy) = (rx + icx, ry / squash + icy);
            out[(oy * ow + ox) as usize] = bilinear(&gray, w, h, sx - 0.5, sy - 0.5);
        }
    }
    (out, ow, oh)
}

fn bilinear(gray: &[u8], w: u32, h: u32, x: f64, y: f64) -> u8 {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let at = |xi: f64, yi: f64| -> f64 {
        if xi < 0.0 || yi < 0.0 || xi >= w as f64 || yi >= h as f64 {
            0.0
        } else {
            gray[yi as usize * w as usize + xi as usize] as f64
        }
    };
    let v = at(x0, y0) * (1.0 - fx) * (1.0 - fy)
        + at(x0 + 1.0, y0) * fx * (1.0 - fy)
        + at(x0, y0 + 1.0) * (1.0 - fx) * fy
        + at(x0 + 1.0, y0 + 1.0) * fx * fy;
    v.round().clamp(0.0, 255.0) as u8
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

// ---------------------------------------------------------------------------
// Naming
// ---------------------------------------------------------------------------

/// Never overwrite an existing set: the slug truncates at 24 chars, so
/// "…Inkers vol 1" and "…Inkers vol 2" collide — and imported/ holds presets
/// the artist may have RETUNED since. A colliding import suffixes -2, -3, …
pub(super) fn free_slug(root: &Path, set: &str) -> String {
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
    if !taken(set) {
        return set.to_string();
    }
    let mut n = 2usize;
    loop {
        let cand = format!("{set}-{n}");
        if !taken(&cand) {
            break cand;
        }
        n += 1;
    }
}

/// `set` name for files: lowercase ascii, filesystem- and picker-safe.
pub(super) fn set_slug(stem: &str) -> String {
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
        "brushes".into()
    } else {
        s.chars().take(24).collect()
    }
}

impl App {
    /// Import a brush file picked from the menu — Photoshop `.abr`, GIMP
    /// `.gbr`/`.gih` or Krita `.kpp`, by extension. Rescans presets and
    /// texture names so the new brushes appear without a restart.
    pub fn import_abr(&mut self, path: &Path) {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Some(root) = self.brushes_root.clone() else {
            return self.set_error("brush import: no brushes folder found");
        };
        let sum = match ext.as_str() {
            // Clip Studio exported sub tool (a SQLite .sut).
            "sut" => match mn_brush::sut::parse_sut_file(path) {
                Ok(b) => super::sut_import::write_sut_import(
                    &root,
                    &b,
                    &set_slug(&stem),
                    self.page.as_ref().map(|p| p.dpi).unwrap_or(0),
                ),
                Err(e) => return self.set_error(format!("brush import failed: {e}")),
            },
            "kpp" => match mn_brush::parse_kpp_file(path) {
                Ok(preset) => {
                    super::kpp_import::write_kpp_import(&root, &preset, &set_slug(&stem))
                }
                Err(e) => return self.set_error(format!("brush import failed: {e}")),
            },
            "gbr" | "gih" => match mn_brush::parse_gimp_brush_file(path) {
                Ok(brushes) if brushes.is_empty() => {
                    return self.set_error("brush import: no brushes in that file");
                }
                Ok(brushes) => write_gimp_import(&root, &brushes, &set_slug(&stem)),
                Err(e) => return self.set_error(format!("brush import failed: {e}")),
            },
            _ => {
                let bytes = match std::fs::read(path) {
                    Ok(b) => b,
                    Err(e) => return self.set_error(format!("brush import: {e}")),
                };
                match mn_brush::parse_abr_set(&bytes, &stem) {
                    Ok(set) if set.tips.is_empty() && set.presets.is_empty() => {
                        return self.set_error("brush import: no sampled tips in that file");
                    }
                    Ok(set) => write_import(&root, &set, &set_slug(&stem)),
                    Err(e) => return self.set_error(format!("brush import failed: {e}")),
                }
            }
        };
        println!(
            "[brush-import] {}: {} imported ({} with dynamics, {} notes), {} blank skipped",
            path.display(),
            sum.imported,
            sum.translated,
            sum.notes,
            sum.blank
        );
        // The new files are inside the discovered root: rescan picks them up.
        self.presets = super::scan_presets();
        self.texture_names = super::scan_textures(self.brushes_root.as_deref());
        let dynamics = if sum.translated > 0 {
            format!(", {} with dynamics translated", sum.translated)
        } else {
            String::new()
        };
        self.set_status(format!(
            "imported {} brushes from {} (group \"imported\"{dynamics})",
            sum.imported,
            path.display()
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mn_brush::AbrTip;

    fn tmp_root(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mn-abr-{tag}-{}", std::process::id()));
        let root = dir.join("brushes");
        std::fs::create_dir_all(&root).unwrap();
        root
    }
    fn tips_only(tips: Vec<AbrTip>) -> AbrSet {
        AbrSet {
            tips,
            presets: Vec::new(),
        }
    }
    fn read_myb(root: &Path, slug: &str) -> Value {
        serde_json::from_str(
            &std::fs::read_to_string(root.join("imported").join(format!("{slug}.myb"))).unwrap(),
        )
        .unwrap()
    }

    /// Round trip through a synthetic set: PNG + .myb on disk, both loadable
    /// by the exact code paths the app uses (load_texture, preset scan).
    #[test]
    fn tips_become_textures_and_presets() {
        let root = tmp_root("basic");
        let set = tips_only(vec![
            AbrTip {
                name: "Ink".into(),
                gray: vec![255, 0, 255, 255],
                width: 2,
                height: 2,
                sample_id: None,
            },
            AbrTip {
                name: "Blank".into(),
                gray: vec![0; 4],
                width: 2,
                height: 2,
                sample_id: None,
            },
        ]);
        let sum = write_import(&root, &set, "myset");
        assert_eq!((sum.imported, sum.blank, sum.translated), (1, 1, 0));

        let mask = mn_brush::load_texture(&root, "myset-1").expect("texture written");
        assert_eq!(mask.size, 2);
        assert_eq!(&mask.data[..], &[255, 0, 255, 255]);

        let myb = read_myb(&root, "myset-1");
        assert_eq!(myb["name"], "Ink");
        assert_eq!(myb["group"], "imported");
        assert_eq!(myb["mn-texture"], "myset-1");
        assert!(myb["settings"]["radius_logarithmic"]["base_value"].is_number());
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    /// The radius base is ln, not log2 — the engine's radius is exp(rlog).
    /// This FAILED against the old importer (`.log2()` drew every tip
    /// oversized: exp(log2(50)) ≈ 280 px instead of 50).
    #[test]
    fn default_size_is_the_tip_size_in_ln_units() {
        let root = tmp_root("ln");
        let set = tips_only(vec![AbrTip {
            name: "T".into(),
            gray: vec![255; 100 * 100],
            width: 100,
            height: 100,
            sample_id: None,
        }]);
        write_import(&root, &set, "ln");
        let myb = read_myb(&root, "ln-1");
        let rlog = myb["settings"]["radius_logarithmic"]["base_value"]
            .as_f64()
            .unwrap();
        assert!(
            (rlog - (50.0f64).ln()).abs() < 1e-6,
            "rlog {rlog}, want ln(50) ≈ 3.912"
        );
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    /// The ROADMAP wart: an extreme-aspect tip padded into a big bitmap must
    /// size (and crop) by its INK, not its padding. FAILED against the old
    /// importer, which kept the 400-px canvas and sized from it.
    #[test]
    fn padded_tip_crops_and_sizes_by_ink() {
        let root = tmp_root("pad");
        // A 400×400 bitmap whose ink is one 10×40 bar in a corner.
        let mut gray = vec![0u8; 400 * 400];
        for y in 0..40 {
            for x in 0..10 {
                gray[y * 400 + x] = 255;
            }
        }
        let set = tips_only(vec![AbrTip {
            name: "Bar".into(),
            gray,
            width: 400,
            height: 400,
            sample_id: None,
        }]);
        write_import(&root, &set, "pad");
        let mask = mn_brush::load_texture(&root, "pad-1").unwrap();
        assert_eq!(mask.size, 40, "tight-cropped to the ink, then squared");
        let myb = read_myb(&root, "pad-1");
        let rlog = myb["settings"]["radius_logarithmic"]["base_value"]
            .as_f64()
            .unwrap();
        assert!((rlog - (20.0f64).ln()).abs() < 1e-6, "sized by ink: {rlog}");
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    /// Full dynamics translation: spacing, pressure size with minimum,
    /// size jitter, scatter, count, pressure flow, opacity jitter — and the
    /// honest notes for what cannot map.
    #[test]
    fn dynamics_translate_and_the_rest_is_noted() {
        use mn_brush::{AbrPresetInfo, BrushKind, Control, DynGroup};
        let root = tmp_root("dyn");
        let info = AbrPresetInfo {
            name: Some("Scatter Ink".into()),
            kind: BrushKind::Sampled {
                sample_id: "u-1".into(),
            },
            diameter_px: Some(64.0),
            angle_deg: 0.0,
            roundness_pct: 100.0,
            flip_x: false,
            flip_y: false,
            spacing_pct: Some(25.0),
            size: DynGroup {
                control: Control::Pressure,
                jitter_pct: 20.0,
            },
            minimum_diameter_pct: 40.0,
            angle_dyn: DynGroup {
                control: Control::Direction,
                jitter_pct: 0.0,
            },
            roundness_dyn: DynGroup {
                control: Control::Off,
                jitter_pct: 0.0,
            },
            minimum_roundness_pct: 0.0,
            scatter: DynGroup {
                control: Control::Off,
                jitter_pct: 120.0,
            },
            scatter_both_axes: true,
            count: 2.0,
            count_dyn: DynGroup {
                control: Control::Off,
                jitter_pct: 0.0,
            },
            opacity: DynGroup {
                control: Control::Off,
                jitter_pct: 15.0,
            },
            flow: DynGroup {
                control: Control::Pressure,
                jitter_pct: 0.0,
            },
            wet_edges: true,
            airbrush: false,
            unmodeled: vec!["dual brush"],
        };
        let set = AbrSet {
            tips: vec![AbrTip {
                name: "tip".into(),
                gray: vec![255; 16],
                width: 4,
                height: 4,
                sample_id: Some("u-1".into()),
            }],
            presets: vec![info],
        };
        let sum = write_import(&root, &set, "dyn");
        assert_eq!((sum.imported, sum.translated), (1, 1));
        let myb = read_myb(&root, "dyn-1");
        let s = &myb["settings"];
        // Preset name wins over the tip name.
        assert_eq!(myb["name"], "Scatter Ink");
        // Size: from Dmtr 64 → ln(32); pressure sweeps from 40 %.
        assert!(
            (s["radius_logarithmic"]["base_value"].as_f64().unwrap() - (32.0f64).ln()).abs()
                < 1e-6
        );
        let pr = &s["radius_logarithmic"]["inputs"]["pressure"];
        assert!((pr[0][1].as_f64().unwrap() - (0.4f64).ln()).abs() < 1e-6);
        // Size jitter 20 % → random input down to ln(0.8).
        let rr = &s["radius_logarithmic"]["inputs"]["random"];
        assert!((rr[0][1].as_f64().unwrap() - (0.8f64).ln()).abs() < 1e-6);
        // Spacing 25 % → 2 dabs per radius, doubled by count 2 → 4.
        assert!(
            (s["dabs_per_actual_radius"]["base_value"].as_f64().unwrap() - 4.0).abs() < 1e-6
        );
        assert_eq!(s["dabs_per_basic_radius"]["base_value"], 0.0);
        // Scatter 120 % of diameter → 2.4 radii.
        assert!((myb["mn-scatter"].as_f64().unwrap() - 2.4).abs() < 1e-6);
        // Flow by pressure → the opaque_multiply pressure curve.
        assert_eq!(
            s["opaque_multiply"]["inputs"]["pressure"][1],
            json!([1.0, 1.0])
        );
        // Opacity jitter 15 % → opaque random dip to −0.15.
        assert!(
            (s["opaque"]["inputs"]["random"][0][1].as_f64().unwrap() + 0.15).abs() < 1e-6
        );
        // Photoshop stamps per dab: sampled imports anchor to the dab, and
        // the Direction-controlled angle translates into the live stamp
        // rotation instead of a note (#10 amendment 2).
        assert_eq!(myb["mn-texture-anchor"], "dab");
        assert_eq!(myb["mn-texture-rotate"], "direction");
        // The honest notes: count approximation, wet edges, dual brush —
        // in mn.unmapped AND the description. No per-dab-angle note: it
        // translated.
        let notes: Vec<String> = myb["mn"]["unmapped"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert!(!notes.iter().any(|n| n.contains("per-dab angle")));
        assert!(notes.iter().any(|n| n.contains("count 2")));
        assert!(notes.iter().any(|n| n.contains("wet edges")));
        assert!(notes.iter().any(|n| n.contains("dual brush")));
        assert!(myb["description"].as_str().unwrap().contains("Not translated:"));
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    /// Static geometry bakes: flip X mirrors, roundness squashes, and a 90°
    /// angle turns the tip — checked on an L-shaped asymmetric mask.
    #[test]
    fn static_geometry_bakes_into_the_mask() {
        // 4×2, ink on the left column + bottom row.
        let gray = vec![255, 0, 0, 0, 255, 255, 255, 255];
        // Flip X: ink moves to the right column.
        let (fx, w, h) = bake_geometry(gray.clone(), 4, 2, true, false, 100.0, 0.0);
        assert_eq!((w, h), (4, 2));
        assert_eq!(&fx[..4], &[0, 0, 0, 255]);
        // 90° CCW: the 4×2 becomes 2×4.
        let (rot, w, h) = bake_geometry(gray.clone(), 4, 2, false, false, 100.0, 90.0);
        assert_eq!((w, h), (2, 4));
        assert!(rot.iter().any(|&v| v > 128));
        // Roundness 50 %: height halves.
        let (sq, w, h) = bake_geometry(vec![255; 4 * 4], 4, 4, false, false, 50.0, 0.0);
        assert_eq!((w, h), (4, 2));
        assert!(sq.iter().any(|&v| v > 128));
        std::mem::drop(sq);
    }

    #[test]
    fn slug_is_safe_and_bounded() {
        assert_eq!(set_slug("My Brush Set!"), "my-brush-set");
        assert_eq!(set_slug("???"), "brushes");
        assert!(set_slug("ø").len() <= 24);
        assert!(set_slug(&"x".repeat(80)).len() <= 24);
    }

    /// The real vendored v6 set, end to end THROUGH the dynamics: parse →
    /// write_import → masks load through `load_texture`, presets carry
    /// translated spacing, and blank tips still skip.
    #[test]
    fn real_set_round_trips_with_dynamics() {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../brush/tests/data/abr_v6_sample.abr");
        let Ok(bytes) = std::fs::read(&fixture) else {
            return; // fixture not shipped: skip silently
        };
        let set = mn_brush::parse_abr_set(&bytes, "sample").unwrap();
        assert_eq!(set.tips.len(), 31);
        assert!(set.presets.len() >= 30, "desc parsed: {}", set.presets.len());
        let root = tmp_root("real");
        let sum = write_import(&root, &set, "sample");
        // Every preset with a live tip imports; the blank tip's presets and
        // the unreferenced blank both count as blank/skips, never a crash.
        assert!(sum.imported >= 30, "imported {}", sum.imported);
        assert!(sum.translated >= 30, "translated {}", sum.translated);
        // Sampled brushes wrote textures the engine loader can read (the
        // set leads with computed brushes, so slug numbers have gaps).
        let textures: Vec<String> = std::fs::read_dir(root.join("textures"))
            .unwrap()
            .flatten()
            .filter_map(|e| {
                e.path()
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
            })
            .collect();
        assert!(textures.len() >= 25, "textures written: {}", textures.len());
        let mask = mn_brush::load_texture(&root, &textures[0]).expect("texture loads");
        assert!(mask.size > 0 && mask.size <= 1024);
        // At least one preset carries translated spacing (Spcn was enabled
        // in the file) — dabs_per_basic_radius zeroed is the fingerprint.
        let translated = (1..=sum.imported).any(|i| {
            let p = root.join("imported").join(format!("sample-{i}.myb"));
            std::fs::read_to_string(p)
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .is_some_and(|v| {
                    v["settings"]["dabs_per_basic_radius"]["base_value"] == json!(0.0)
                })
        });
        assert!(translated, "no preset shows translated spacing");
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    /// A 2000-px-wide tip still downscales into the 1024 square.
    #[test]
    fn oversized_tip_downscales_into_the_square() {
        let root = tmp_root("big");
        let (w, h) = (2000u32, 1000u32);
        let set = tips_only(vec![AbrTip {
            name: "Wide".into(),
            gray: vec![200u8; (w * h) as usize],
            width: w,
            height: h,
            sample_id: None,
        }]);
        write_import(&root, &set, "wide");
        let mask = mn_brush::load_texture(&root, "wide-1").expect("texture written");
        assert_eq!(mask.size, 1024);
        assert_eq!(mask.data[0], 0);
        assert!(mask.data[(512 * 1024 + 512) as usize] > 100);
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    /// GIMP: spacing translates, alpha-coverage tips import, blanks skip.
    #[test]
    fn gimp_brushes_import_with_spacing() {
        let root = tmp_root("gimp");
        let brushes = vec![
            GbrBrush {
                name: "Pepper".into(),
                gray: vec![255; 64],
                width: 8,
                height: 8,
                spacing_pct: 50,
            },
            GbrBrush {
                name: "Empty".into(),
                gray: vec![0; 4],
                width: 2,
                height: 2,
                spacing_pct: 25,
            },
        ];
        let sum = write_gimp_import(&root, &brushes, "gimp");
        assert_eq!((sum.imported, sum.blank), (1, 1));
        let myb = read_myb(&root, "gimp-1");
        assert_eq!(myb["name"], "Pepper");
        // Spacing 50 % → 1 dab per actual radius.
        assert!(
            (myb["settings"]["dabs_per_actual_radius"]["base_value"]
                .as_f64()
                .unwrap()
                - 1.0)
                .abs()
                < 1e-6
        );
        assert!(myb["description"].as_str().unwrap().contains("GIMP"));
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }
}
