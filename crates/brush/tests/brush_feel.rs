//! The "how does the brush FEEL" rows. Originally the three of CSP-TRIAGE
//! 63/65/74 — Stroke ▸ Interval (`S-028`), Adjust brush density by gap
//! (`B-029`) and the four-level Anti-aliasing (`A-010`) — joined by the ink
//! group (rows 56/57/60: density of paint, colour stretch, blur intensity)
//! and colour jitter (row 61), which are the same KIND of claim: a number
//! the panel promises and the dabs have to keep.
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

// --- Rows 56/57/60: the ink group (I-010/011/013) -----------------------
//
// These three are the colour-mixing knobs, and CSP's names for them are not
// libmypaint's: "density of paint" is the OTHER END of `smudge`, "color
// stretch" is `smudge_length`, "intensity of blur" is `smudge_radius_log`.
// The whole risk of the row is a rename that silently inverts or rescales,
// so the assertions read the base value the engine actually holds.

/// Each knob lands on its `.myb` key, in the right direction and the right
/// unit — including the pinned-pixel mode, which is a conversion against the
/// live radius rather than a stored number.
#[test]
fn ink_options_land_on_their_myb_keys() {
    let mut b = pen();

    // A pen ships neat: density must READ 1.0, not 0.0, or the row shows
    // "no paint on the brush" for every preset in the tree.
    assert!(
        (b.paint_density() - 1.0).abs() < 1e-6,
        "an inking pen starts at neat paint, got {}",
        b.paint_density()
    );
    assert!(!b.smudge(), "and does not sample the canvas");

    b.set_paint_density(0.25);
    assert!(
        (b.base_value(setting::SMUDGE) - 0.75).abs() < 1e-6,
        "density is the picked-up share INVERTED: {}",
        b.base_value(setting::SMUDGE)
    );
    assert!((b.paint_density() - 0.25).abs() < 1e-6, "and reads back");
    // The GPU-routing flag has to follow, or a brush the artist mixed with
    // mid-session gets a smudge sampler that is never served.
    assert!(b.smudge(), "mixing at runtime must set the routing flag");

    b.set_color_stretch(0.8);
    assert!((b.base_value(setting::SMUDGE_LENGTH) - 0.8).abs() < 1e-6);
    assert!((b.color_stretch() - 0.8).abs() < 1e-6);

    // Relative blur: the setting is the LOGARITHM of the multiple.
    b.set_blur(4.0, false);
    assert!(
        (b.base_value(setting::SMUDGE_RADIUS_LOG) - 4.0f32.ln()).abs() < 1e-5,
        "relative blur is stored as ln(multiple)"
    );
    assert_eq!(b.blur(), (4.0, false));

    // Pinned blur: 10 px on a 40 px brush is half a radius.
    b.set_size_px(40.0);
    b.set_blur(10.0, true);
    let want = (10.0f32 / b.radius_px()).ln();
    assert!(
        (b.base_value(setting::SMUDGE_RADIUS_LOG) - want).abs() < 1e-4,
        "pinned blur must convert against the live radius"
    );
    let (amount, absolute) = b.blur();
    assert!(absolute && (amount - 10.0).abs() < 0.01, "{amount} px");

    // Nothing non-finite reaches the C: `ln(0)` and NaN are both a smudge
    // sampler reading garbage off the canvas.
    b.set_paint_density(f32::NAN);
    assert!((b.paint_density() - 1.0).abs() < 1e-6);
    b.set_color_stretch(f32::INFINITY);
    assert!(b.color_stretch().is_finite());
    for bad in [0.0, f32::NAN, f32::NEG_INFINITY] {
        b.set_blur(bad, false);
        assert!(
            b.base_value(setting::SMUDGE_RADIUS_LOG).is_finite(),
            "blur {bad} produced a non-finite radius"
        );
    }

    // And the neutral state is a true no-op: back to neat paint, and the
    // pen's dabs are the pen's dabs again.
    let untouched = record_stroke(&mut pen());
    b.set_paint_density(1.0);
    b.set_size_px(pen().base_size_px());
    let back = record_stroke(&mut b);
    assert_eq!(untouched.len(), back.len(), "dab count changed");
}

/// The behavioural half: below full density the dabs stop being the drawing
/// colour and start carrying what is underneath. Measured on the RECORDED
/// dab colours, because "it looks blended" is exactly the claim a wrong sign
/// would also satisfy.
#[test]
fn low_paint_density_picks_up_the_colour_underneath() {
    // Lay a wide red band first, with a fat opaque brush.
    let mut doc = Document::new(1024, 1024);
    let mut under = pen();
    under.set_size_px(80.0);
    under.set_color_rgb([1.0, 0.0, 0.0]);
    under.begin(&mut doc);
    for i in 0..=24 {
        under.sample(&mut doc, sample(100.0 + i as f32 * 8.0, 512.0, 1.0, i as f64 * 8.0));
    }
    under.end(&mut doc);

    let mut blue_over_red = |density: f32| -> Vec<[u16; 3]> {
        let mut b = pen();
        b.set_color_rgb([0.0, 0.0, 1.0]);
        b.set_paint_density(density);
        b.set_color_stretch(0.5);
        b.set_dab_recording(RecordMode::Tap);
        b.begin(&mut doc);
        for i in 0..=24 {
            b.sample(&mut doc, sample(100.0 + i as f32 * 8.0, 512.0, 1.0, i as f64 * 8.0));
        }
        b.end(&mut doc);
        b.take_dab_record().dabs.iter().map(|d| d.color).collect()
    };

    // Neat paint: every dab is the drawing colour, whatever is under it.
    let neat = blue_over_red(1.0);
    assert!(
        neat.iter().all(|c| c[0] == 0),
        "neat paint must not pick anything up: {:?}",
        &neat[..neat.len().min(4)]
    );
    // Mixed: the red under the stroke reaches the dab colour.
    let mixed = blue_over_red(0.0);
    assert!(
        mixed.iter().any(|c| c[0] > 0),
        "no red reached the dabs at density 0: {:?}",
        &mixed[..mixed.len().min(4)]
    );
}

// --- Row 61: colour jitter (C-010..012) ---------------------------------

/// The row's promise is that a stroke is not one flat value — and the
/// house rule is that it is still the SAME not-flat value every time, so a
/// replay, an undo/redo or a test can reproduce it (the M4 tip-variation
/// precedent). Both halves, on the recorded dab colours.
#[test]
fn color_jitter_varies_along_the_stroke_and_repeats_exactly() {
    let colors = |j: mn_brush::ColorJitter| -> Vec<[u16; 3]> {
        let mut b = pen();
        b.set_color_rgb([0.2, 0.5, 0.9]);
        b.set_color_jitter(j);
        record_stroke(&mut b).iter().map(|d| d.color).collect()
    };
    let amounts = mn_brush::ColorJitter {
        hue: 0.4,
        sat: 0.3,
        bri: 0.3,
        per_dab: true,
    };

    // Off: one flat colour, exactly as before this row existed.
    let off = colors(mn_brush::ColorJitter::default());
    assert!(!off.is_empty());
    assert!(
        off.windows(2).all(|w| w[0] == w[1]),
        "jitter off must leave the drawing colour alone"
    );

    // Along the stroke: the colour keeps moving.
    let along = colors(amounts);
    let distinct = along.iter().collect::<std::collections::HashSet<_>>().len();
    assert!(distinct > 3, "only {distinct} colours along the stroke");
    assert_eq!(along, colors(amounts), "the same stroke must repeat exactly");

    // Per stroke: internally even, but not the drawing colour itself.
    let per_stroke = colors(mn_brush::ColorJitter {
        per_dab: false,
        ..amounts
    });
    assert!(
        per_stroke.windows(2).all(|w| w[0] == w[1]),
        "per-stroke jitter must not vary WITHIN the stroke"
    );
    assert_ne!(per_stroke[0], off[0], "...but it must move off the colour");
}

// --- Row 71: the watercolour edge (W-001..005) --------------------------
//
// The claim here is not arithmetic on a dab, it is "the pixels outside the
// stroke changed and the pixels inside it did not". So these run a REAL
// stroke twice — once with the rim off, once on — and compare the two
// documents. The off run is the pin: every brush that has never heard of
// this row must paint the bytes it always painted.

/// Ink one stroke through a fresh Real G-Pen and hand back the document.
/// `tune` gets the brush before `begin`, which is where the rim is armed.
fn inked(tune: impl FnOnce(&mut MyBrush)) -> Document {
    let mut b = pen();
    b.set_size_px(20.0);
    b.set_color_rgb([0.0, 0.0, 0.0]);
    tune(&mut b);
    let mut doc = Document::new(512, 256);
    b.begin(&mut doc);
    for i in 0..=40 {
        b.sample(&mut doc, sample(80.0 + i as f32 * 8.0, 128.0, 1.0, i as f64 * 8.0));
    }
    b.end(&mut doc);
    doc
}

fn px_at(doc: &Document, x: i32, y: i32) -> [u16; 4] {
    let i = mn_core::TileIdx::of_pixel(x, y);
    let (ox, oy) = i.origin();
    doc.active_layer()
        .tile(i)
        .map(|t| t.pixel((x - ox) as usize, (y - oy) as usize))
        .unwrap_or([0; 4])
}

/// How far the ink reaches above the stroke's centre line at x=240.
fn top_reach(doc: &Document) -> i32 {
    (0..128i32)
        .filter(|dy| px_at(doc, 240, 128 - dy)[3] > 0)
        .max()
        .unwrap_or(-1)
}

/// The differential. With the rim armed the stroke reaches further and the
/// pixels it gains are ink; with it off the stroke is exactly the stroke it
/// has always been, byte for byte, and its opaque body is untouched either
/// way.
#[test]
fn watercolour_edge_rims_the_stroke_and_spares_its_body() {
    let off = inked(|_| {});
    let off_again = inked(|b| b.set_water_edge(mn_core::edge::WaterEdge::default()));
    for x in 60..460 {
        for y in 100..156 {
            assert_eq!(
                px_at(&off, x, y),
                px_at(&off_again, x, y),
                "({x},{y}): the default rim is not off"
            );
        }
    }

    let rim = mn_core::edge::WaterEdge {
        px: 4.0,
        opacity: 1.0,
        darkness: 0.0,
        blur_px: 0.0,
    };
    let on = inked(|b| b.set_water_edge(rim));

    let (r_off, r_on) = (top_reach(&off), top_reach(&on));
    assert!(
        r_on > r_off,
        "the rim must reach further than the ink: {r_on} vs {r_off}"
    );
    assert!(
        r_on <= r_off + 5,
        "...and not further than the 4 px it was asked for: {r_on} vs {r_off}"
    );

    // The body: CSP puts the rim OUTSIDE the stroke, so every pixel the
    // stroke inked keeps its exact bytes — not "almost", and not only where
    // it went fully opaque. A G-Pen's antialiased skirt is inked too.
    let mut body = 0;
    for x in 60..460 {
        for y in 100..156 {
            if px_at(&off, x, y)[3] >= 200 {
                body += 1;
                assert_eq!(px_at(&on, x, y), px_at(&off, x, y), "body pixel ({x},{y})");
            }
        }
    }
    assert!(body > 1000, "only {body} inked body pixels to check");
}

/// An eraser stroke rims nothing: its coverage went the other way, and a
/// fringed hole is the classic way this feature ruins an erase.
#[test]
fn an_eraser_grows_no_watercolour_rim() {
    let rim = mn_core::edge::WaterEdge {
        px: 4.0,
        opacity: 1.0,
        ..mn_core::edge::WaterEdge::default()
    };
    // Ink a band, then erase across it twice — once with the rim armed.
    let erased = |armed: bool| {
        let mut doc = inked(|_| {});
        let mut b = pen();
        b.set_size_px(20.0);
        b.set_eraser(true);
        if armed {
            b.set_water_edge(rim);
        }
        b.begin(&mut doc);
        for i in 0..=20 {
            b.sample(&mut doc, sample(240.0, 60.0 + i as f32 * 8.0, 1.0, i as f64 * 8.0));
        }
        b.end(&mut doc);
        doc
    };
    let (plain, armed) = (erased(false), erased(true));
    for x in 200..280 {
        for y in 60..200 {
            assert_eq!(
                px_at(&plain, x, y),
                px_at(&armed, x, y),
                "({x},{y}): the eraser grew a rim"
            );
        }
    }
}

/// The rim decides the stroke's routing: it is derived from the CPU tiles at
/// stroke end, and under GPU BYPASS those were never written — so an armed
/// brush must refuse the compute path rather than silently draw no rim.
#[test]
fn the_watercolour_rim_routes_the_stroke_cpu() {
    let mut b = pen();
    assert!(b.gpu_ready(), "a plain pen is GPU-ready");
    b.set_water_edge(mn_core::edge::WaterEdge {
        px: 3.0,
        opacity: 0.5,
        ..mn_core::edge::WaterEdge::default()
    });
    assert!(!b.gpu_ready(), "an armed rim must force the CPU path");
    b.set_water_edge(mn_core::edge::WaterEdge::default());
    assert!(b.gpu_ready(), "and give it back when the rim goes off");
}

/// The four knobs survive a `.myb` round trip, clamped, and a preset that
/// says nothing about them loads OFF.
#[test]
fn watercolour_edge_loads_from_a_preset() {
    let plain = pen();
    assert!(
        !plain.water_edge().on(),
        "a preset with no rim keys must load off"
    );

    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(csp("real-g-pen.myb")).unwrap()).unwrap();
    json["mn-water-edge"] = serde_json::json!(2.5);
    json["mn-water-edge-opacity"] = serde_json::json!(0.4);
    json["mn-water-edge-darkness"] = serde_json::json!(0.75);
    json["mn-water-edge-blur"] = serde_json::json!(1.5);
    let dir = std::env::temp_dir().join(format!("mn-we-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rim.myb");
    std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    let loaded = MyBrush::load(&path).expect("the rim preset must load");
    let e = loaded.water_edge();
    assert!(e.on());
    assert!((e.px - 2.5).abs() < 1e-6, "{e:?}");
    assert!((e.opacity - 0.4).abs() < 1e-6, "{e:?}");
    assert!((e.darkness - 0.75).abs() < 1e-6, "{e:?}");
    assert!((e.blur_px - 1.5).abs() < 1e-6, "{e:?}");
    let _ = std::fs::remove_dir_all(&dir);

    // Nonsense is clamped, never carried into the tile loop as a NaN reach.
    let mut b = pen();
    b.set_water_edge(mn_core::edge::WaterEdge {
        px: f32::NAN,
        opacity: 9.0,
        darkness: -3.0,
        blur_px: 1e9,
    });
    let e = b.water_edge();
    assert_eq!(e.px, 0.0, "NaN width degrades to off");
    assert_eq!(e.opacity, 1.0);
    assert_eq!(e.darkness, 0.0);
    assert_eq!(e.blur_px, mn_core::edge::WIDTH_MAX);
}

// --- Rows 58 + 167: Ink ▸ Mixing mode (I-014) ---------------------------
//
// The pigment model, not an amount — so unlike the rows above these are
// measured on PIXELS, not on the recorded dab parameters. The whole change
// lives between the dab and the tile (libmypaint's spectral blend, vendored
// and reached through PATCHES.md #21); a dab record would look identical in
// both modes and prove nothing.

/// Straight (un-premultiplied) 0..1 RGB at a canvas pixel, or `None` where
/// nothing was painted.
fn straight_rgb(doc: &Document, x: i32, y: i32) -> Option<[f32; 3]> {
    let ti = mn_core::TileIdx::of_pixel(x, y);
    let t = doc.layers[0].tile(ti)?;
    let p = t.pixel((x - ti.x * 64) as usize, (y - ti.y * 64) as usize);
    if p[3] == 0 {
        return None;
    }
    let a = p[3] as f32;
    Some([p[0] as f32 / a, p[1] as f32 / a, p[2] as f32 / a])
}

/// A fat opaque band of `under` across y = 512, then a second pass of `over`
/// through the mixing mode being tested. `flow` thins the second pass so the
/// two colours actually have to meet — at full opacity every mixing model
/// agrees, which is the trap this helper exists to avoid.
fn band_over_band(mix: mn_brush::BrushMix, under: [f32; 3], over: [f32; 3], flow: f32) -> Document {
    let mut doc = Document::new(1024, 1024);
    fn run(doc: &mut Document, b: &mut MyBrush) {
        b.begin(doc);
        for i in 0..=24 {
            b.sample(
                doc,
                sample(100.0 + i as f32 * 8.0, 512.0, 1.0, i as f64 * 8.0),
            );
        }
        b.end(doc);
    }
    let mut base = pen();
    base.set_size_px(80.0);
    base.set_color_rgb(under);
    run(&mut doc, &mut base);

    let mut top = pen();
    top.set_size_px(60.0);
    top.set_color_rgb(over);
    top.set_flow(flow);
    top.set_color_mixing(mix);
    run(&mut doc, &mut top);
    doc
}

/// The default, and the routing claim: an untouched preset mixes additively
/// and stays on the GPU dab path.
#[test]
fn mixing_defaults_to_standard_and_stays_gpu_ready() {
    let b = pen();
    assert_eq!(b.color_mixing(), mn_brush::BrushMix::Standard);
    assert!(b.gpu_ready(), "a stock pen must still take the GPU path");
}

/// Row 58's routing, RETARGETED by the wave-4 spectral port: `dab.wgsl`
/// carries the `*_Paint` arms now (parity-pinned in dab_parity.rs), so
/// Perceptual keeps the GPU path — routing it CPU again would silently
/// throw away the port on every Paint-mode stroke.
#[test]
fn perceptual_mixing_keeps_the_gpu_path_since_the_spectral_port() {
    let mut b = pen();
    b.set_color_mixing(mn_brush::BrushMix::Perceptual);
    assert_eq!(b.color_mixing(), mn_brush::BrushMix::Perceptual);
    assert!(
        b.gpu_ready(),
        "static spectral mixing has a GPU arm since wave 4 — it must ride"
    );
    b.set_color_mixing(mn_brush::BrushMix::Standard);
    assert!(b.gpu_ready(), "and Standard stays GPU-ready as ever");
}

/// THE BYTE-PIN. Standard is not "close to" the old behaviour, it IS the old
/// behaviour: the patched C reads a published weight where it used to read
/// the literal `0.0`, and Standard publishes exactly `0.0`.
///
/// Three strokes that must be identical to the byte — never touched, set to
/// Standard, and toggled through Perceptual and back — because the switch
/// writes a libmypaint base value, and a one-way write is the classic way a
/// "revert" quietly is not one.
#[test]
fn standard_mixing_leaves_the_stroke_byte_identical() {
    fn row(doc: &Document) -> Vec<[u16; 4]> {
        (100..300)
            .map(|x| {
                let ti = mn_core::TileIdx::of_pixel(x, 512);
                doc.layers[0]
                    .tile(ti)
                    .map(|t| t.pixel((x - ti.x * 64) as usize, (512 - ti.y * 64) as usize))
                    .unwrap_or([0; 4])
            })
            .collect()
    }
    let draw = |setup: &dyn Fn(&mut MyBrush)| -> Document {
        let mut doc = Document::new(1024, 1024);
        let mut b = pen();
        b.set_size_px(60.0);
        b.set_color_rgb([0.1, 0.4, 0.9]);
        b.set_flow(0.5);
        setup(&mut b);
        b.begin(&mut doc);
        for i in 0..=24 {
            b.sample(
                &mut doc,
                sample(100.0 + i as f32 * 8.0, 512.0, 1.0, i as f64 * 8.0),
            );
        }
        b.end(&mut doc);
        doc
    };
    let untouched = row(&draw(&|_b| {}));
    let explicit = row(&draw(&|b| b.set_color_mixing(mn_brush::BrushMix::Standard)));
    let round_trip = row(&draw(&|b| {
        b.set_color_mixing(mn_brush::BrushMix::Perceptual);
        b.set_color_mixing(mn_brush::BrushMix::Standard);
    }));
    assert!(untouched.iter().any(|p| p[3] > 0), "the stroke painted");
    assert_eq!(untouched, explicit, "setting Standard changed the pixels");
    assert_eq!(
        untouched, round_trip,
        "Perceptual → Standard did not restore the original path"
    );
}

/// The feature actually reaching the pixels — the fail-before-fix half.
/// Before PATCHES.md #21 the legacy stroke entry forced the spectral weight
/// to zero, so this assertion could not have passed no matter what the row
/// was set to.
#[test]
fn perceptual_mixing_changes_the_pixels() {
    let yellow = [1.0, 0.9, 0.0];
    let blue = [0.0, 0.1, 0.9];
    let std_doc = band_over_band(mn_brush::BrushMix::Standard, yellow, blue, 0.5);
    let perc_doc = band_over_band(mn_brush::BrushMix::Perceptual, yellow, blue, 0.5);
    let mut differing = 0;
    for x in 120..280 {
        let a = straight_rgb(&std_doc, x, 512);
        let b = straight_rgb(&perc_doc, x, 512);
        if let (Some(a), Some(b)) = (a, b)
            && (0..3).any(|c| (a[c] - b[c]).abs() > 1e-3)
        {
            differing += 1;
        }
    }
    assert!(
        differing > 100,
        "spectral mixing changed only {differing} pixels — the weight is not reaching the blend"
    );
}

/// The CLAIM, not just a difference — triage row 58's own words: *"Standard
/// mixes in raw RGB, which drives blends toward grey mud. Perceptual mixes
/// the way paint looks like it should."*
///
/// So the measure is CHANNEL SPREAD, not channel level. Subtractive mixing
/// is darker by definition (two pigments each subtract), so asserting "more
/// green" in absolute terms would be asserting the wrong physics and would
/// fail on a correct implementation — it did, on the first run: additive
/// left `[0.469, 0.475, 0.478]`, which is grey to three decimals, and
/// spectral left `[0.056, 0.343, 0.228]`, which is a green. What separates
/// them is that one has a dominant channel and the other has none.
#[test]
fn blue_over_yellow_goes_green_under_pigment_mixing() {
    let mid = |mix| {
        let doc = band_over_band(mix, [1.0, 0.9, 0.0], [0.0, 0.1, 0.9], 0.5);
        let mut acc = [0.0f32; 3];
        let mut n = 0.0f32;
        for x in 150..250 {
            if let Some(p) = straight_rgb(&doc, x, 512) {
                for c in 0..3 {
                    acc[c] += p[c];
                }
                n += 1.0;
            }
        }
        assert!(n > 0.0, "nothing painted to measure");
        [acc[0] / n, acc[1] / n, acc[2] / n]
    };
    let s = mid(mn_brush::BrushMix::Standard);
    let p = mid(mn_brush::BrushMix::Perceptual);
    let spread = |c: [f32; 3]| c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2]);
    assert!(
        spread(s) < 0.05,
        "additive blue-over-yellow should be grey mud, got {s:?}"
    );
    assert!(
        p[1] > p[0] && p[1] > p[2],
        "pigment blue-over-yellow must read GREEN: {p:?}"
    );
    assert!(
        spread(p) > 4.0 * spread(s),
        "pigment mixing must keep far more colour than additive: {p:?} vs {s:?}"
    );
}

/// "Smudge sampling uses the chosen mix": with paint density below 1 the
/// brush picks colour up off the canvas, and the sampler has to weight
/// spectrally too — otherwise the dab mixes pigment with a colour that was
/// averaged additively, a blend that is half one model and half the other
/// and looks like neither.
#[test]
fn the_smudge_sampler_follows_the_mixing_mode() {
    let picked = |mix: mn_brush::BrushMix| -> Vec<[u16; 3]> {
        let mut doc = Document::new(1024, 1024);
        let mut under = pen();
        under.set_size_px(90.0);
        under.set_color_rgb([1.0, 0.9, 0.0]);
        under.begin(&mut doc);
        for i in 0..=24 {
            under.sample(
                &mut doc,
                sample(100.0 + i as f32 * 8.0, 512.0, 1.0, i as f64 * 8.0),
            );
        }
        under.end(&mut doc);

        let mut b = pen();
        b.set_size_px(60.0);
        b.set_color_rgb([0.0, 0.1, 0.9]);
        b.set_paint_density(0.35);
        b.set_color_stretch(0.5);
        b.set_color_mixing(mix);
        b.set_dab_recording(RecordMode::Tap);
        b.begin(&mut doc);
        for i in 0..=24 {
            b.sample(
                &mut doc,
                sample(100.0 + i as f32 * 8.0, 512.0, 1.0, i as f64 * 8.0),
            );
        }
        b.end(&mut doc);
        b.take_dab_record().dabs.iter().map(|d| d.color).collect()
    };
    let s = picked(mn_brush::BrushMix::Standard);
    let p = picked(mn_brush::BrushMix::Perceptual);
    assert_eq!(s.len(), p.len(), "the mode must not change dab placement");
    assert!(
        s.iter().zip(&p).any(|(a, b)| a != b),
        "the picked-up colour is identical in both modes — the sampler ignored the weight"
    );
}

/// `I-014`'s second clause, which CSP's manual states outright: the mixing
/// mode ALSO governs colour jitter. Under Perceptual the offsets are applied
/// in Oklab through `mn_core::mix::shift_oklab` — the SAME implementation the
/// gradient's Perceptual ramp uses — instead of libmypaint's HSV.
///
/// Both halves: it changes the colours when jitter is on, and it changes
/// NOTHING when jitter is off (the mode must not tint a plain stroke).
#[test]
fn the_mixing_mode_also_governs_colour_jitter() {
    let colors = |mix: mn_brush::BrushMix, jitter: mn_brush::ColorJitter| -> Vec<[u16; 3]> {
        let mut b = pen();
        b.set_color_rgb([0.2, 0.5, 0.9]);
        b.set_color_mixing(mix);
        b.set_color_jitter(jitter);
        record_stroke(&mut b).iter().map(|d| d.color).collect()
    };
    let on = mn_brush::ColorJitter {
        hue: 0.2,
        sat: 0.3,
        bri: 0.3,
        per_dab: true,
    };
    let off = mn_brush::ColorJitter {
        hue: 0.0,
        sat: 0.0,
        bri: 0.0,
        per_dab: true,
    };
    assert_ne!(
        colors(mn_brush::BrushMix::Standard, on),
        colors(mn_brush::BrushMix::Perceptual, on),
        "the mixing mode must reach the jitter"
    );
    assert_eq!(
        colors(mn_brush::BrushMix::Standard, off),
        colors(mn_brush::BrushMix::Perceptual, off),
        "with jitter off, the mode must not touch the drawing colour"
    );
}

/// A preset authored with libmypaint's own `paint_mode` setting loads as
/// Perceptual — and, since the wave-4 spectral port, a STATIC base value
/// keeps the GPU path while an INPUT-MAPPED one still routes CPU (the one
/// spectral shape `dab.wgsl` does not carry; nothing in the tree ships one,
/// this pins the import door).
#[test]
fn a_preset_authored_with_paint_mode_loads_spectral_and_routes_by_mapping() {
    let raw = std::fs::read_to_string(csp("real-g-pen.myb")).unwrap();
    let mut json: serde_json::Value = serde_json::from_str(&raw).unwrap();
    json["settings"]["paint_mode"] = serde_json::json!({ "base_value": 1.0 });
    let dir = std::env::temp_dir().join(format!("mn-mix-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("pigment.myb");
    std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    let b = MyBrush::load(&path).expect("the pigment preset must load");
    assert_eq!(b.color_mixing(), mn_brush::BrushMix::Perceptual);
    assert!(
        b.gpu_ready(),
        "a static authored spectral preset rides the GPU arms"
    );

    // The mapped shape: a pressure->paint_mode curve. Dynamic weight, no
    // GPU expression — and switching to Standard must NOT hand the GPU
    // back, because zeroing a base value does not switch a mapping off.
    json["settings"]["paint_mode"] = serde_json::json!({
        "base_value": 0.5,
        "inputs": { "pressure": [[0.0, 0.0], [1.0, 1.0]] }
    });
    let path2 = dir.join("pigment-mapped.myb");
    std::fs::write(&path2, serde_json::to_vec(&json).unwrap()).unwrap();
    let mut m = MyBrush::load(&path2).expect("the mapped preset must load");
    assert!(!m.gpu_ready(), "a MAPPED spectral preset routes CPU");
    m.set_color_mixing(mn_brush::BrushMix::Standard);
    assert!(
        !m.gpu_ready(),
        "Standard cannot un-map the preset — it stays CPU-bound"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The default the OTHER way round: `paint_mode`'s own default in
/// `brushsettings.json` is 1.0 — MyPaint 2 ships spectral mixing ON. Every
/// preset in this tree must still load Standard, or the row would have
/// silently re-inked the whole brush library the day the C patch landed.
#[test]
fn stock_presets_do_not_inherit_libmypaints_spectral_default() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/brushes/csp");
    let mut checked = 0;
    for e in std::fs::read_dir(&dir).expect("the csp preset folder must exist") {
        let p = e.unwrap().path();
        if p.extension().and_then(|s| s.to_str()) != Some("myb") {
            continue;
        }
        let Ok(b) = MyBrush::load(&p) else { continue };
        assert_eq!(
            b.color_mixing(),
            mn_brush::BrushMix::Standard,
            "{p:?} inherited libmypaint's spectral default"
        );
        checked += 1;
    }
    assert!(checked > 0, "no presets were checked");
}
