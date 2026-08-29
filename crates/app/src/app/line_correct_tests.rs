//! Row 169 — line correction on a vector-ink layer (`E-001`…`E-007`,
//! `VL-021`…`VL-027`). Four passes over a whole recorded layer: the record
//! is what changes, the raster comes back through the same replay every
//! other vector edit uses, and each pass is ONE undo press.
//!
//! The geometry half tests pure (no engine, no GPU, so it runs everywhere);
//! the app half pins the two things that can only go wrong in the app — the
//! raster actually re-derives, and the step count is one.

use super::*;
use crate::app::vector_edit::{LineCorrect, correct_lines};
use mn_core::{PenSample, StrokeSet, TileIdx, VectorStroke};

// --- the pure half ---------------------------------------------------------

/// A straight recorded stroke from (x0,y0) to (x1,y1) in `n` samples.
fn line(x0: f32, y0: f32, x1: f32, y1: f32, n: usize) -> VectorStroke {
    let pts: Vec<PenSample> = (0..n)
        .map(|i| {
            let t = i as f32 / (n - 1) as f32;
            PenSample {
                x: x0 + (x1 - x0) * t,
                y: y0 + (y1 - y0) * t,
                pressure: 1.0,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: i as f64 * 8.0,
            }
        })
        .collect();
    VectorStroke::from_samples(&pts, "pen", 8.0, [0, 0, 0], false)
}

fn ends(s: &VectorStroke) -> ((f32, f32), (f32, f32)) {
    let (a, b) = (s.points[0], s.points[s.points.len() - 1]);
    ((a.0, a.1), (b.0, b.1))
}

/// `E-007` / `VL-024`: the stub goes, the line stays, and the threshold is
/// a real length — a stroke exactly at it survives.
#[test]
fn delete_short_lines_takes_the_stubs_and_nothing_else() {
    let mut set = StrokeSet {
        strokes: vec![
            line(0.0, 10.0, 300.0, 10.0, 61), // 300 px
            line(0.0, 40.0, 6.0, 40.0, 3),    // 6 px stub
            line(0.0, 70.0, 50.0, 70.0, 11),  // 50 px, exactly the threshold
        ],
    };
    assert!(correct_lines(&mut set, LineCorrect::DeleteShort { px: 50.0 }));
    assert_eq!(set.strokes.len(), 2, "only the 6 px stub died");
    assert!((ends(&set.strokes[0]).1.0 - 300.0).abs() < 1e-3);
    assert!(
        (ends(&set.strokes[1]).1.0 - 50.0).abs() < 1e-3,
        "at the threshold is not under it"
    );
    // A second pass at the same threshold has nothing left to do, so it
    // must report no change — that `false` is what stops the caller
    // spending an undo step on nothing.
    assert!(!correct_lines(&mut set, LineCorrect::DeleteShort { px: 50.0 }));
}

/// `E-005` / `VL-025`: near-touching ends become ONE stroke, and the join is
/// DIRECTION-aware — the second line is drawn back towards the first, so a
/// naive concatenation would leave a path that doubles back through the
/// seam. It must come out as one continuous run.
#[test]
fn connect_joins_near_ends_and_reverses_the_run_that_needs_it() {
    // A ends at (100,0); B is drawn from (200,0) BACK to (101,0), so it is
    // B's END that meets A's end.
    let mut set = StrokeSet {
        strokes: vec![line(0.0, 0.0, 100.0, 0.0, 21), line(200.0, 0.0, 101.0, 0.0, 21)],
    };
    assert!(correct_lines(
        &mut set,
        LineCorrect::Connect {
            px: 4.0,
            across: false
        }
    ));
    assert_eq!(set.strokes.len(), 1, "the two became one");
    let s = &set.strokes[0];
    assert_eq!(ends(s).0, (0.0, 0.0), "it still starts where A started");
    assert!((ends(s).1.0 - 200.0).abs() < 1e-3, "and ends where B started");
    // Continuous: no step in the joined path is longer than the gap plus a
    // sample's worth of travel. A non-reversing join would show a ~99 px
    // jump right at the seam.
    let biggest = s
        .points
        .windows(2)
        .map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1))
        .fold(0.0f32, f32::max);
    assert!(biggest <= 10.0, "the path doubles back: {biggest} px step");
    // The clock survives the reversal: the replay's stabilizer reads it.
    assert!(
        s.points.windows(2).all(|w| w[1].5 > w[0].5),
        "sample times must stay monotonic across a reversed run"
    );

    // Out of tolerance, nothing happens.
    let mut far = StrokeSet {
        strokes: vec![line(0.0, 0.0, 100.0, 0.0, 21), line(140.0, 0.0, 240.0, 0.0, 21)],
    };
    assert!(!correct_lines(
        &mut far,
        LineCorrect::Connect {
            px: 4.0,
            across: false
        }
    ));
    assert_eq!(far.strokes.len(), 2);
}

/// `E-006`: two lines with different properties do NOT join unless you say
/// so — and when they do, the LONGER one's properties win.
#[test]
fn connect_respects_properties_until_told_otherwise() {
    let build = || {
        let mut a = line(0.0, 0.0, 20.0, 0.0, 5); // short, red
        a.color = [255, 0, 0];
        let mut b = line(21.0, 0.0, 300.0, 0.0, 60); // long, blue, fatter
        b.color = [0, 0, 255];
        b.size_px = 20.0;
        StrokeSet {
            strokes: vec![a, b],
        }
    };
    let mut set = build();
    assert!(
        !correct_lines(
            &mut set,
            LineCorrect::Connect {
                px: 4.0,
                across: false
            }
        ),
        "a 2 px red pen must not silently absorb a fat blue one"
    );

    let mut set = build();
    assert!(correct_lines(
        &mut set,
        LineCorrect::Connect {
            px: 4.0,
            across: true
        }
    ));
    assert_eq!(set.strokes.len(), 1);
    assert_eq!(set.strokes[0].color, [0, 0, 255], "the longer line's colour");
    assert!((set.strokes[0].size_px - 20.0).abs() < 1e-3, "…and its tip");
}

/// `E-001` / `VL-021`: redundant points go, the ends stay, and a corner is
/// never rounded off — it is by construction the point furthest from the
/// chord, so it survives every tolerance below its own depth.
#[test]
fn simplify_drops_the_redundant_points_and_keeps_the_corner() {
    let straight = line(0.0, 0.0, 300.0, 0.0, 61);
    let mut set = StrokeSet {
        strokes: vec![straight.clone()],
    };
    assert!(correct_lines(&mut set, LineCorrect::Simplify { px: 1.0 }));
    assert_eq!(set.strokes[0].points.len(), 2, "a straight line is 2 points");
    assert_eq!(ends(&set.strokes[0]), ends(&straight), "the ends are kept");

    // An L: 30 samples out, 30 samples down. The corner is 100 px off the
    // chord, so it survives a 5 px tolerance while the runs collapse.
    let mut l = line(0.0, 0.0, 100.0, 0.0, 31);
    let down = line(100.0, 0.0, 100.0, 100.0, 31);
    l.points.extend(down.points.into_iter().skip(1));
    let mut set = StrokeSet {
        strokes: vec![l],
    };
    assert!(correct_lines(&mut set, LineCorrect::Simplify { px: 5.0 }));
    let p = &set.strokes[0].points;
    assert_eq!(p.len(), 3, "start, corner, end — {p:?}");
    assert!((p[1].0 - 100.0).abs() < 1e-3 && p[1].1.abs() < 1e-3, "{p:?}");

    // Idempotent: a second pass has nothing to drop.
    assert!(!correct_lines(&mut set, LineCorrect::Simplify { px: 5.0 }));

    // The whole SAMPLE survives, not just its position — pressure rides
    // along, which is what keeps a simplified stroke's taper.
    let mut tapered = line(0.0, 0.0, 300.0, 0.0, 61);
    for (i, p) in tapered.points.iter_mut().enumerate() {
        p.2 = 0.2 + 0.8 * (i as f32 / 60.0);
    }
    let mut set = StrokeSet {
        strokes: vec![tapered],
    };
    assert!(correct_lines(&mut set, LineCorrect::Simplify { px: 1.0 }));
    let p = &set.strokes[0].points;
    assert!((p[0].2 - 0.2).abs() < 1e-3 && (p[p.len() - 1].2 - 1.0).abs() < 1e-3, "{p:?}");
}

/// `VL-026` scales the recorded width; `VL-027`'s floor means narrowing can
/// never erase a line, however hard you pull the slider.
#[test]
fn width_scales_and_never_narrows_below_one_pixel() {
    let mut set = StrokeSet {
        strokes: vec![line(0.0, 0.0, 100.0, 0.0, 11)],
    };
    assert!((set.strokes[0].size_px - 8.0).abs() < 1e-4);
    assert!(correct_lines(&mut set, LineCorrect::Width { scale: 2.0 }));
    assert!((set.strokes[0].width_scale - 2.0).abs() < 1e-4);
    // Scaling compounds — it multiplies the record, it does not set it.
    assert!(correct_lines(&mut set, LineCorrect::Width { scale: 0.5 }));
    assert!((set.strokes[0].width_scale - 1.0).abs() < 1e-4);

    // VL-027: 8 px × 0.01 would be 0.08 px — invisible. The floor is on the
    // DERIVED width, so it lands at exactly 1 px and stays there.
    assert!(correct_lines(&mut set, LineCorrect::Width { scale: 0.01 }));
    let derived = set.strokes[0].size_px * set.strokes[0].width_scale;
    assert!((derived - 1.0).abs() < 1e-3, "{derived} px");
    assert!(
        !correct_lines(&mut set, LineCorrect::Width { scale: 0.01 }),
        "already at the floor: no change, no undo step"
    );
    // ×1 is the identity and must never spend a step.
    assert!(!correct_lines(&mut set, LineCorrect::Width { scale: 1.0 }));
}

// --- the app half ----------------------------------------------------------

fn vector_app() -> Option<App> {
    let mut app = App::new(super::headless_renderer()?, (600, 400), 1.0);
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::AddVectorLayer);
    app.props_current.stabilizer = 0.0;
    app.props_current.size_px = 8.0;
    app.prefs.mouse_smooth_px = 0.0;
    app.apply_props();
    app.viewport.zoom = 1.0;
    app.viewport.pan = [0.0, 0.0];
    Some(app)
}

/// One straight drag along `y`, from `x0` for `n` samples 6 px apart.
fn drag(app: &mut App, x0: f32, y: f32, n: usize) {
    app.begin_stroke(PointerKind::Mouse);
    let batch: Vec<PenSample> = (0..n)
        .map(|i| PenSample {
            x: x0 + i as f32 * 6.0,
            y,
            pressure: 0.9,
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_ms: i as f64 * 8.0,
        })
        .collect();
    app.push_batch(&batch);
    app.end_stroke();
}

fn tiles(app: &App, li: usize) -> std::collections::BTreeMap<TileIdx, Vec<u16>> {
    app.doc.layers[li]
        .tiles()
        .map(|(idx, t)| (idx, t.data().to_vec()))
        .collect()
}

fn layer_alpha(app: &App, li: usize) -> u64 {
    let mut sum = 0u64;
    for (_, t) in app.doc.layers[li].tiles() {
        for p in 0..mn_core::TILE_PIXELS {
            sum += u64::from(t.pixel(p % 64, p / 64)[3]);
        }
    }
    sum
}

fn records(app: &App, li: usize) -> usize {
    app.doc.layers[li].strokes.as_ref().unwrap().strokes.len()
}

/// The headline: a stub swept off a real layer takes its INK with it (the
/// raster re-derives), it costs exactly one undo press, and that press puts
/// both halves back.
#[test]
fn a_correction_pass_re_derives_the_raster_in_one_undo_press() {
    let Some(mut app) = vector_app() else { return };
    let li = app.doc.active;
    drag(&mut app, 60.0, 120.0, 30); // ~174 px line
    drag(&mut app, 60.0, 220.0, 3); // ~12 px stub
    assert_eq!(records(&app, li), 2);
    let inked_both = layer_alpha(&app, li);
    assert!(inked_both > 0);
    let steps = app.doc.undo_labels().len();

    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::LineCorrect(LineCorrect::DeleteShort { px: 40.0 }),
    );
    assert_eq!(records(&app, li), 1, "the stub's record is gone");
    let after = layer_alpha(&app, li);
    assert!(
        after > 0 && after < inked_both,
        "…and so is its INK: {after} vs {inked_both}"
    );
    assert_eq!(
        app.doc.undo_labels().len(),
        steps + 1,
        "one press for the whole layer"
    );
    assert_eq!(app.doc.undo_labels().last().map(String::as_str), Some("Delete short lines"));

    // The pixels are exactly what the surviving record replays — the whole
    // point of editing records instead of the raster.
    let corrected = tiles(&app, li);
    app.doc.begin_op();
    app.rederive_vector_layer(li);
    app.doc.end_op();
    assert_eq!(tiles(&app, li), corrected, "the raster IS the replay");

    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Undo); // the bare re-derive
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Undo); // the correction
    assert_eq!(records(&app, li), 2, "one press restores the record");
    assert_eq!(layer_alpha(&app, li), inked_both, "…and the ink, same step");
}

/// The other three passes reach the raster too, each for one press. Connect
/// is the one that would silently do nothing if the record were joined but
/// the pixels never replayed — so it is checked by ink, not by count.
#[test]
fn connect_simplify_and_width_all_reach_the_pixels() {
    let Some(mut app) = vector_app() else { return };
    let li = app.doc.active;
    // Two collinear runs with a 6 px gap between them.
    drag(&mut app, 60.0, 160.0, 20); // 60 → 174
    drag(&mut app, 180.0, 160.0, 20); // 180 → 294
    assert_eq!(records(&app, li), 2);
    let steps = app.doc.undo_labels().len();

    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::LineCorrect(LineCorrect::Connect {
            px: 8.0,
            across: false,
        }),
    );
    assert_eq!(records(&app, li), 1, "the gap closed into one line");
    assert_eq!(app.doc.undo_labels().len(), steps + 1);
    let joined = layer_alpha(&app, li);
    assert!(joined > 0);

    // Width: same geometry, visibly more ink, one press.
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::LineCorrect(LineCorrect::Width { scale: 2.0 }),
    );
    assert_eq!(app.doc.undo_labels().len(), steps + 2);
    assert!(
        layer_alpha(&app, li) > joined,
        "a ×2 width pass must actually re-ink fatter"
    );
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Undo);
    assert_eq!(layer_alpha(&app, li), joined, "and undo takes the ink back");

    // Simplify: a mouse-straight drag is all redundant points.
    let before_pts = app.doc.layers[li].strokes.as_ref().unwrap().strokes[0]
        .points
        .len();
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::LineCorrect(LineCorrect::Simplify { px: 1.0 }),
    );
    let after_pts = app.doc.layers[li].strokes.as_ref().unwrap().strokes[0]
        .points
        .len();
    assert!(after_pts < before_pts, "{after_pts} vs {before_pts}");
    assert_eq!(app.doc.undo_labels().len(), steps + 2);
    let simplified = tiles(&app, li);
    app.doc.begin_op();
    app.rederive_vector_layer(li);
    app.doc.end_op();
    assert_eq!(tiles(&app, li), simplified, "still exactly the replay");
}

/// Refusals: a layer that records nothing has no geometry to correct, and a
/// pass that changes nothing spends nothing. Neither may touch the history.
#[test]
fn line_correction_refuses_plain_layers_and_no_op_passes() {
    let Some(mut app) = vector_app() else { return };
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::AddLayer);
    let plain = app.doc.active;
    assert!(app.doc.layers[plain].strokes.is_none());
    drag(&mut app, 60.0, 300.0, 20);
    let ink = layer_alpha(&app, plain);
    let steps = app.doc.undo_labels().len();

    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::LineCorrect(LineCorrect::Simplify { px: 4.0 }),
    );
    assert_eq!(app.doc.undo_labels().len(), steps, "no step spent");
    assert_eq!(layer_alpha(&app, plain), ink, "and not one pixel moved");
    assert!(app.status.contains("vector layer"), "{}", app.status);

    // Back on the vector layer, a threshold that catches nothing is also a
    // no-op — the guard that keeps the History palette honest.
    let li = app
        .doc
        .layers
        .iter()
        .position(|l| l.strokes.is_some())
        .expect("the vector layer");
    app.doc.set_active(li);
    let steps = app.doc.undo_labels().len();
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::LineCorrect(LineCorrect::DeleteShort { px: 0.5 }),
    );
    assert_eq!(app.doc.undo_labels().len(), steps, "nothing short: no step");
}
