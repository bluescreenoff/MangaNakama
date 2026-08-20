//! ROADMAP good-first-issue #1: the VERTICAL view flip — the other half of
//! Ctrl+9's drawing-error check.
//!
//! Three seams, one per file that had to learn about `flip_v`: the command
//! layer (toggle, compose, reset), the fit (a flipped view stays flipped
//! through it, CSP-style), and the BRUSH's view compensation — vendor patch
//! #12 knows only a horizontal mirror, so `Viewport::brush_view()` hands it
//! the equivalent mirror-plus-half-turn. Skip that last one and the strokes
//! are still drawn, just with every direction-mapped dynamic reading the
//! mirrored angle: subtly wrong ink, no error anywhere.

use super::*;
use crate::cmd::AppCmd;
use mn_brush::{MyBrush, RecordMode, settings};
use mn_core::{Document, PenSample, StrokeSink};

/// The toggle itself, through `dispatch`: each axis flips on its own, the
/// two compose into a half turn (NOT a mirror), and both view resets clear
/// both axes — a reset that left the page upside down would be a lie.
#[test]
fn flip_view_v_toggles_composes_and_resets() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (800, 600), 1.0);
    app.shell.set_canvas_rect_points(egui::Rect::from_min_size(
        egui::pos2(0.0, 0.0),
        egui::vec2(800.0, 600.0),
    ));
    crate::cmd::dispatch(&mut app, AppCmd::ZoomFit);

    let centre = app.canvas_center();
    let anchor = app.viewport.to_canvas(centre[0], centre[1]);
    crate::cmd::dispatch(&mut app, AppCmd::FlipViewV);
    assert!(app.viewport.flip_v, "Ctrl+Shift+9 flips vertically");
    assert!(!app.viewport.flip_h, "and leaves the other axis alone");
    assert!(app.viewport.mirrored(), "one flip IS a mirror");
    let s = app.viewport.to_screen(anchor.0, anchor.1);
    assert!(
        (s.0 - centre[0]).abs() < 0.5 && (s.1 - centre[1]).abs() < 0.5,
        "the flip must pivot about the view centre, not swing the page: {s:?}"
    );

    // Composed: H+V is a 180° turn, so the handedness is back to normal.
    crate::cmd::dispatch(&mut app, AppCmd::FlipView);
    assert!(app.viewport.flip_h && app.viewport.flip_v);
    assert!(
        !app.viewport.mirrored(),
        "two flips cancel: the page is upside down, not mirrored"
    );

    // Reset 2 of CV-035 clears BOTH axes and straightens the page.
    crate::cmd::dispatch(&mut app, AppCmd::RotateFlipReset);
    assert!(!app.viewport.flip_h && !app.viewport.flip_v);
    assert_eq!(app.viewport.rotate_rad, 0.0);

    // …and so does the full reset.
    crate::cmd::dispatch(&mut app, AppCmd::FlipViewV);
    crate::cmd::dispatch(&mut app, AppCmd::ViewReset);
    assert!(!app.viewport.flip_v, "ViewReset must not leave it flipped");
}

/// `fit_to_view_sized` deliberately CARRIES a flip through a fit (a flipped
/// view stays flipped, CSP-style). The vertical flip needs its own pan
/// correction for that — without it the page fits off the top of the window.
#[test]
fn a_vertical_flip_survives_a_fit_still_centred() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (800, 600), 1.0);
    let (w, h) = (app.doc.size.0 as f32, app.doc.size.1 as f32);

    app.fit_to_view_sized((800, 600));
    let upright = app.viewport.to_screen(w * 0.5, h * 0.5);

    app.viewport.flip_v = true;
    app.fit_to_view_sized((800, 600));
    assert!(app.viewport.flip_v, "the fit keeps the flip");
    let flipped = app.viewport.to_screen(w * 0.5, h * 0.5);
    assert!(
        (flipped.0 - upright.0).abs() < 0.5 && (flipped.1 - upright.1).abs() < 0.5,
        "the page centre must land where it lands upright: {flipped:?} vs {upright:?}"
    );

    // Both flips at once, same story — the two corrections are independent.
    app.viewport.flip_h = true;
    app.fit_to_view_sized((800, 600));
    assert!(app.viewport.flip_h && app.viewport.flip_v);
    let both = app.viewport.to_screen(w * 0.5, h * 0.5);
    assert!(
        (both.0 - upright.0).abs() < 0.5 && (both.1 - upright.1).abs() < 0.5,
        "H+V fit off centre: {both:?} vs {upright:?}"
    );
}

/// Patch #12's flip extension, the vertical case — shaped exactly like
/// `mn-brush`'s `direction_inputs_are_view_flip_compensated`, one axis over
/// and driven through the VIEWPORT (which is what the app does live).
///
/// The DIRECTION input cannot be read back, so a steep DIRECTION→Size curve
/// makes the RADIUS carry it: the same screen-space motion must produce the
/// same dab radii whatever the view is doing. A vertically flipped view that
/// forwarded `flip_h` (false) verbatim feeds the C the mirrored doc
/// direction — 45° read as 135° — and the dabs collapse.
#[test]
fn stroke_radii_do_not_drift_under_a_vertically_flipped_view() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/brushes/classic/pen.myb");
    if !path.exists() {
        println!("[test] SKIP: no pen.myb on disk");
        return;
    }
    let run = |flip_h: bool, flip_v: bool| -> Vec<f32> {
        let vp = Viewport {
            pan: [0.0, 0.0],
            zoom: 1.0,
            rotate_rad: 0.0,
            flip_h,
            flip_v,
        };
        let mut b = MyBrush::load(&path).expect("preset load must succeed");
        b.set_mapping(
            settings::setting::RADIUS_LOGARITHMIC,
            settings::input::DIRECTION,
            &[(0.0, 0.0), (180.0, -2.5)],
        );
        b.set_dab_recording(RecordMode::Tap);
        let (rot, mirrored) = vp.brush_view();
        b.set_view(vp.zoom, rot, mirrored);
        let mut doc = Document::default();
        b.begin(&mut doc);
        for i in 0..150 {
            let t = i as f32 / 149.0;
            // The SAME SCREEN path in every run, at a steady OFF-AXIS angle
            // (45° plus a perpendicular wobble so the direction filter has
            // work): DIRECTION is mod 180, so an axis-aligned path would be
            // flip-invariant BY ACCIDENT and prove nothing.
            let a = t * 320.0;
            let wob = (t * 7.0).sin() * 30.0;
            let (sx, sy) = (100.0 + a - wob, 200.0 + a + wob);
            let (cx, cy) = vp.to_canvas(sx, sy);
            b.sample(
                &mut doc,
                PenSample {
                    x: cx,
                    y: cy,
                    pressure: 0.8,
                    tilt_x: 0.0,
                    tilt_y: 0.0,
                    t_ms: i as f64 * 8.0,
                },
            );
        }
        b.end(&mut doc);
        b.take_dab_record().dabs.iter().map(|d| d.radius).collect()
    };

    let normal = run(false, false);
    assert!(normal.len() > 30, "too few dabs to compare");
    let mean = |v: &[f32]| v.iter().skip(20).sum::<f32>() / (v.len() - 20) as f32;
    let m1 = mean(&normal);
    for (fh, fv, what) in [
        (false, true, "vertical"),
        (true, true, "both axes (a half turn)"),
    ] {
        let m2 = mean(&run(fh, fv));
        assert!(
            (m1 - m2).abs() / m1 < 0.05,
            "{what}-flipped stroke radii drifted: normal mean {m1} vs flipped mean {m2} \
             (the brush was told the wrong view — see Viewport::brush_view)"
        );
    }
}

/// The live stroke path, end to end: screen samples pushed at a vertically
/// flipped view must land on the canvas points the view says they are over.
/// This is the `to_canvas` consumer that actually inks.
#[test]
fn a_stroke_under_a_vertical_flip_inks_where_the_view_says() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (800, 600), 1.0);
    app.viewport = Viewport {
        pan: [0.0, 400.0],
        zoom: 1.0,
        rotate_rad: 0.0,
        flip_h: false,
        flip_v: true,
    };

    app.begin_stroke(PointerKind::Mouse);
    app.engine_mut().set_dab_recording_all(RecordMode::Tap);
    let batch: Vec<PenSample> = (0..30)
        .map(|i| {
            // Canvas points along y = 200, sent as the SCREEN coordinates
            // the flipped view puts them at.
            let (sx, sy) = app.viewport.to_screen(100.0 + i as f32 * 10.0, 200.0);
            PenSample {
                x: sx,
                y: sy,
                pressure: 0.8,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: i as f64 * 8.0,
            }
        })
        .collect();
    app.push_batch(&batch);
    app.end_stroke();
    let dabs = app.engine_mut().drain_dab_records();
    assert!(!dabs.is_empty(), "the stroke painted");
    for d in &dabs {
        assert!(
            (d.y - 200.0).abs() < 2.0 && (90.0..=400.0).contains(&d.x),
            "dab ({}, {}) is not on the flipped view's canvas line \
             (to_canvas ignoring flip_v puts it at y ≈ {})",
            d.x,
            d.y,
            -200.0
        );
    }
}
