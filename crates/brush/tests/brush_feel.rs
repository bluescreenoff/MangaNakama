//! The three "how does the brush FEEL" rows (CSP-TRIAGE 63/65/74):
//! Stroke ▸ Interval (`S-028`), Adjust brush density by gap (`B-029`) and the
//! four-level Anti-aliasing (`A-010`).
//!
//! These are all arithmetic, and the arithmetic is the whole feature — a
//! spacing control that is off by a factor of two still draws a plausible
//! stroke, which is exactly why it would never be noticed. So nothing here
//! asserts "it painted something": every test measures the RECORDED DABS of a
//! real stroke (`RecordMode::Tap`, the GPU-dabs P0 tap) and checks the number
//! the row promises — the gap in canvas pixels, the per-dab alpha, the edge
//! feather.
//!
//! The brush under test is the owner's Real G-Pen because it is the one preset
//! with nothing in the way: `dabs_per_actual_radius` 5 and no
//! `dabs_per_second`, so dab placement is purely distance-driven; no
//! `offset_by_random` / `radius_by_random`, so positions are deterministic;
//! and its pressure→radius curve is flat at the top, so a stroke held at
//! pressure 1.0 draws at exactly the base radius.

use std::path::{Path, PathBuf};

use mn_brush::settings::setting;
use mn_brush::{AntiAlias, DabParams, Interval, MyBrush, RecordMode};
use mn_core::{Document, PenSample, StrokeSink};

fn csp(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/brushes/csp")
        .join(name)
}

fn pen() -> MyBrush {
    MyBrush::load(&csp("real-g-pen.myb")).expect("real-g-pen.myb must load")
}

fn sample(x: f32, y: f32, p: f32, t: f64) -> PenSample {
    PenSample {
        x,
        y,
        pressure: p,
        tilt_x: 0.0,
        tilt_y: 0.0,
        t_ms: t,
    }
}

/// A long horizontal stroke held at full pressure, recorded rather than
/// judged by eye. 8 px per sample at 125 Hz — a real hand's cadence, and wide
/// enough that even the Wide interval gets several samples per dab.
fn record_stroke(brush: &mut MyBrush) -> Vec<DabParams> {
    let mut doc = Document::new(1024, 1024);
    brush.set_dab_recording(RecordMode::Tap);
    brush.begin(&mut doc);
    for i in 0..=100 {
        brush.sample(
            &mut doc,
            sample(100.0 + i as f32 * 8.0, 512.0, 1.0, i as f64 * 8.0),
        );
    }
    brush.end(&mut doc);
    brush.take_dab_record().dabs
}

/// Mean gap between consecutive dabs, measured over the MIDDLE of the stroke
/// only. The first segment is where the C interpolates pressure up from 0, so
/// its dabs are small and bunched; that ramp is real behaviour, not the thing
/// under test.
fn mean_gap(dabs: &[DabParams]) -> f32 {
    let mid: Vec<f32> = dabs
        .iter()
        .filter(|d| (300.0..=700.0).contains(&d.x))
        .map(|d| d.x)
        .collect();
    assert!(
        mid.len() >= 4,
        "only {} dabs in the measured span — stroke too short to measure",
        mid.len()
    );
    let gaps: Vec<f32> = mid.windows(2).map(|w| w[1] - w[0]).collect();
    gaps.iter().sum::<f32>() / gaps.len() as f32
}

/// The dab alpha the engine settled on, taken from the steady middle of the
/// stroke (same reason as `mean_gap`).
fn steady_opaque(dabs: &[DabParams]) -> f32 {
    let d = dabs
        .iter()
        .find(|d| d.x > 400.0)
        .expect("no dab past x=400");
    d.opaque
}

// --- S-028: interval ----------------------------------------------------

/// The percent modes are a fraction of the tip DIAMETER, which is CSP's own
/// unit for this setting. At the Real G-Pen's 50 px radius that makes Normal
/// a 10 px gap, Narrow half of it and Wide double it — and those three
/// numbers are the entire user-visible meaning of the control.
#[test]
fn percent_interval_is_a_fraction_of_the_tip_diameter() {
    let diameter = pen().radius_px() * 2.0;

    for (pct, name) in [
        (Interval::NARROW_PCT, "narrow"),
        (Interval::NORMAL_PCT, "normal"),
        (Interval::WIDE_PCT, "wide"),
    ] {
        let mut b = pen();
        b.set_interval(Interval::Percent(pct));
        let want = diameter * pct / 100.0;
        let got = mean_gap(&record_stroke(&mut b));
        assert!(
            (got - want).abs() < want * 0.05,
            "{name} ({pct} %): dabs {got:.2} px apart, expected {want:.2} px"
        );
        // The panel readout must agree with the pixels, or it is lying.
        assert!(
            (b.dab_gap_px() - want).abs() < want * 0.02,
            "{name}: readout {:.2} px vs measured {got:.2} px",
            b.dab_gap_px()
        );
    }
}

/// Narrow / Normal / Wide are a doubling ladder, so the dab COUNT doubles at
/// each rung. Asserted separately from the absolute gap because this is the
/// property a user feels when they step through the dropdown.
#[test]
fn the_interval_ladder_doubles_the_dab_count_at_each_rung() {
    let count = |pct: f32| {
        let mut b = pen();
        b.set_interval(Interval::Percent(pct));
        record_stroke(&mut b).len() as f32
    };
    let (narrow, normal, wide) = (
        count(Interval::NARROW_PCT),
        count(Interval::NORMAL_PCT),
        count(Interval::WIDE_PCT),
    );
    assert!(
        (narrow / normal - 2.0).abs() < 0.15,
        "narrow/normal = {:.3}, expected ~2",
        narrow / normal
    );
    assert!(
        (normal / wide - 2.0).abs() < 0.15,
        "normal/wide = {:.3}, expected ~2",
        normal / wide
    );
}

/// The point of the Fixed mode: the gap is a CANVAS distance and does not
/// follow the Size slider, where a percent gap does. This is the half that
/// silently breaks — Fixed is expressed to the engine as dabs-per-basic-
/// radius, which is derived FROM the radius, so resizing the brush has to
/// re-derive it or Fixed quietly becomes relative again.
#[test]
fn fixed_interval_survives_the_size_slider_and_percent_does_not() {
    let gap_at = |size: f32, iv: Interval| {
        let mut b = pen();
        b.set_size_multiplier(size);
        b.set_interval(iv);
        mean_gap(&record_stroke(&mut b))
    };

    let small = gap_at(0.5, Interval::FixedPx(6.0));
    let large = gap_at(2.0, Interval::FixedPx(6.0));
    assert!(
        (small - 6.0).abs() < 0.4 && (large - 6.0).abs() < 0.4,
        "fixed 6 px gap moved with size: {small:.2} px at 0.5x, {large:.2} px at 2x"
    );

    // Same slider travel, percent mode: 4x the brush is 4x the gap.
    let p_small = gap_at(0.5, Interval::Percent(Interval::NORMAL_PCT));
    let p_large = gap_at(2.0, Interval::Percent(Interval::NORMAL_PCT));
    assert!(
        (p_large / p_small - 4.0).abs() < 0.2,
        "percent gap should scale 4x with a 4x brush, got {:.3}x",
        p_large / p_small
    );
}

/// Setting the size AFTER the interval must land in the same place as setting
/// it before: the app pushes both on every property apply and the order is
/// not guaranteed to be stable forever.
#[test]
fn fixed_interval_is_order_independent() {
    let mut before = pen();
    before.set_interval(Interval::FixedPx(6.0));
    before.set_size_multiplier(2.0);

    let mut after = pen();
    after.set_size_multiplier(2.0);
    after.set_interval(Interval::FixedPx(6.0));

    let (a, b) = (
        mean_gap(&record_stroke(&mut before)),
        mean_gap(&record_stroke(&mut after)),
    );
    assert!(
        (a - b).abs() < 0.1,
        "interval-then-size gave {a:.3} px, size-then-interval gave {b:.3} px"
    );
}

/// `AsPreset` is the default and has to be a BYTE-identical no-op, or every
/// brush the owner has used for months starts drawing differently the day
/// this control shipped. Checked on dab positions, not on a checksum of the
/// settings, because the settings are the means and the dabs are the claim.
#[test]
fn as_preset_leaves_the_preset_drawing_exactly_as_it_did() {
    let untouched = record_stroke(&mut pen());

    let mut round_tripped = pen();
    round_tripped.set_interval(Interval::Percent(Interval::WIDE_PCT));
    round_tripped.set_anti_alias(AntiAlias::Strong);
    round_tripped.set_density_by_gap(false);
    // ...and all the way back.
    round_tripped.set_interval(Interval::AsPreset);
    round_tripped.set_anti_alias(AntiAlias::AsPreset);
    round_tripped.set_density_by_gap(true);
    let back = record_stroke(&mut round_tripped);

    assert_eq!(untouched.len(), back.len(), "dab count changed");
    for (u, b) in untouched.iter().zip(&back) {
        assert!(
            (u.x - b.x).abs() < 1e-4 && (u.radius - b.radius).abs() < 1e-4,
            "dab moved: ({:.4}, r{:.4}) -> ({:.4}, r{:.4})",
            u.x,
            u.radius,
            b.x,
            b.radius
        );
        assert!((u.opaque - b.opaque).abs() < 1e-6, "dab alpha changed");
        assert!(
            (u.hardness - b.hardness).abs() < 1e-6,
            "dab hardness changed"
        );
    }
}

/// Out-of-range and non-finite values must never reach the engine: a NaN dab
/// count is an infinite loop in the C, and a 0.01 % interval is a stroke that
/// stamps five thousand dabs per radius.
#[test]
fn interval_input_is_clamped_and_nan_falls_back_to_the_preset() {
    let mut b = pen();

    b.set_interval(Interval::Percent(0.0));
    assert_eq!(b.interval(), Interval::Percent(Interval::MIN_PCT));
    b.set_interval(Interval::Percent(1e6));
    assert_eq!(b.interval(), Interval::Percent(Interval::MAX_PCT));
    b.set_interval(Interval::FixedPx(0.0));
    assert_eq!(b.interval(), Interval::FixedPx(Interval::MIN_PX));
    b.set_interval(Interval::FixedPx(1e6));
    assert_eq!(b.interval(), Interval::FixedPx(Interval::MAX_PX));

    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        b.set_interval(Interval::Percent(bad));
        assert_eq!(b.interval(), Interval::AsPreset, "NaN percent leaked");
        b.set_interval(Interval::FixedPx(bad));
        assert_eq!(b.interval(), Interval::AsPreset, "NaN pixel gap leaked");
    }

    // And whatever it clamped to, the engine's dab count stays finite.
    b.set_interval(Interval::FixedPx(Interval::MIN_PX));
    let dabs = b.base_value(setting::DABS_PER_ACTUAL_RADIUS)
        + b.base_value(setting::DABS_PER_BASIC_RADIUS);
    assert!(
        dabs.is_finite() && dabs <= 50.0,
        "dabs per radius ran away: {dabs}"
    );
    assert!(!record_stroke(&mut b).is_empty());
}

// --- B-029: adjust brush density by gap ---------------------------------

/// The row's promise: with compensation ON the interval stops deciding how
/// dark the stroke comes out. Measured as total laid-down alpha per unit of
/// travel — dab count times per-dab alpha — because that product IS the
/// darkness, and it is the product the compensation holds still.
///
/// The pen is dropped to 50 % opacity first: at `opaque` 1.0 the correction
/// is mathematically a no-op (`1 - 0^(1/n) == 1`), which would make a green
/// test that proves nothing.
#[test]
fn density_by_gap_holds_the_stroke_darkness_across_the_interval() {
    let ink = |pct: f32, on: bool| -> (f32, f32) {
        let mut b = pen();
        b.set_base_opacity(0.5);
        b.set_density_by_gap(on);
        b.set_interval(Interval::Percent(pct));
        let dabs = record_stroke(&mut b);
        (dabs.len() as f32, steady_opaque(&dabs))
    };

    // OFF: every dab paints the same alpha, so 4x the dabs is 4x the ink.
    let (n_off, a_off) = ink(Interval::NARROW_PCT, false);
    let (w_off, b_off) = ink(Interval::WIDE_PCT, false);
    assert!(
        (a_off - b_off).abs() < 1e-4,
        "compensation off must leave per-dab alpha alone: {a_off} vs {b_off}"
    );
    let raw_ratio = (n_off * a_off) / (w_off * b_off);
    assert!(
        raw_ratio > 3.0,
        "expected raw build-up to be ~4x darker at the narrow interval, got {raw_ratio:.2}x"
    );

    // ON: the same two intervals land within a fifth of each other.
    let (n_on, a_on) = ink(Interval::NARROW_PCT, true);
    let (w_on, b_on) = ink(Interval::WIDE_PCT, true);
    let fixed_ratio = (n_on * a_on) / (w_on * b_on);
    assert!(
        (fixed_ratio - 1.0).abs() < 0.2,
        "compensation should flatten the interval's effect on darkness, got {fixed_ratio:.2}x"
    );

    // It only ever LOWERS per-dab alpha (the C clamps dabs-per-pixel at 1
    // first) — it must never brighten a wide-gap stroke to compensate.
    assert!(
        a_on < a_off && b_on < b_off,
        "compensation raised dab alpha"
    );
}

/// The toggle is a tri-state at the app layer but a plain amount here: ON
/// restores the preset's own `opaque_linearize`, OFF is a flat zero. A brush
/// nobody has touched reads as ON because every CSP-derived preset ships 0.9.
#[test]
fn density_by_gap_round_trips_to_the_presets_own_amount() {
    let mut b = pen();
    let shipped = b.base_value(setting::OPAQUE_LINEARIZE);
    assert!(shipped > 0.0, "fixture must ship the compensation on");
    assert!(b.density_by_gap());

    b.set_density_by_gap(false);
    assert!(!b.density_by_gap());
    assert_eq!(b.base_value(setting::OPAQUE_LINEARIZE), 0.0);

    b.set_density_by_gap(true);
    assert_eq!(
        b.base_value(setting::OPAQUE_LINEARIZE),
        shipped,
        "ON must restore the preset's amount, not a house value"
    );
}

// --- A-010: four-level anti-aliasing ------------------------------------

/// CSP's anti-aliasing is a four-rung ladder, not a checkbox, and hard
/// aliased lineart is a deliberate choice in manga rather than a limitation —
/// so `None` has to mean a genuinely untouched hard edge, not "a bit less".
///
/// The engine's knob is a MINIMUM edge fadeout in pixels: it softens hardness
/// and grows the radius together so the OPTICAL radius is preserved. Both
/// halves are asserted, because softening without the radius correction would
/// look identical in a thumbnail and quietly thin every line.
#[test]
fn anti_alias_ladder_softens_the_edge_and_keeps_the_optical_radius() {
    let edge = |aa: AntiAlias| -> (f32, f32) {
        let mut b = pen();
        b.set_anti_alias(aa);
        let dabs = record_stroke(&mut b);
        let d = dabs.iter().find(|d| d.x > 400.0).expect("no steady dab");
        (d.radius, d.hardness)
    };

    // None: the preset's hardness is 1.0 and must arrive at the dab as 1.0.
    let (r_off, h_off) = edge(AntiAlias::Off);
    assert!(
        (h_off - 1.0).abs() < 1e-6,
        "None must leave a hard tip fully hard, got hardness {h_off}"
    );

    let optical = |r: f32, h: f32| r - (1.0 - h) * r / 2.0;
    let optical_off = optical(r_off, h_off);

    let mut prev_hardness = h_off;
    let mut prev_radius = r_off;
    for level in [AntiAlias::Weak, AntiAlias::Middle, AntiAlias::Strong] {
        let feather = level.feather_px().expect("a rung has a feather");
        let (r, h) = edge(level);

        // The ladder is monotonic: each rung is softer and a shade wider.
        assert!(
            h < prev_hardness && r > prev_radius,
            "{level:?} did not soften: hardness {h} (prev {prev_hardness}), radius {r} (prev {prev_radius})"
        );

        // The fadeout the rung asked for is the fadeout the dab got.
        let got_feather = r * (1.0 - h);
        assert!(
            (got_feather - feather).abs() < feather * 0.02,
            "{level:?}: asked for {feather} px of feather, dab has {got_feather:.4} px"
        );

        // ...and the line did not get thinner to pay for it.
        assert!(
            (optical(r, h) - optical_off).abs() < 0.05,
            "{level:?} moved the optical radius: {:.4} vs {optical_off:.4}",
            optical(r, h)
        );

        prev_hardness = h;
        prev_radius = r;
    }
}

/// `AsPreset` reports the file's own feather rather than a rung, and the
/// Real G-Pen ships 0.5 px — which is CSP's own Weak, so a CSP-derived preset
/// starts life on the ladder even though the control says "as preset".
#[test]
fn as_preset_anti_alias_is_the_files_own_feather() {
    let mut b = pen();
    assert_eq!(b.anti_alias(), AntiAlias::AsPreset);
    let shipped = b.anti_alias_px();
    assert!(shipped > 0.0, "fixture must ship a feather");

    b.set_anti_alias(AntiAlias::Strong);
    assert!(b.anti_alias_px() > shipped);
    b.set_anti_alias(AntiAlias::AsPreset);
    assert_eq!(b.anti_alias_px(), shipped);
    assert_eq!(b.anti_alias(), AntiAlias::AsPreset);
}
