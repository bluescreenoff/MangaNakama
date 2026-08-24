//! Vector inking phase 1 (docs/VECTOR-INKING.md): drawing on a recording
//! layer captures the stroke beside the pixels, both halves undo as ONE
//! step, and the captured record reproduces the ink through the live
//! pipeline — the faithfulness later phases' edits rest on.

use super::*;
use mn_core::{PenSample, TileIdx};

fn vector_app() -> Option<App> {
    let mut app = App::new(super::headless_renderer()?, (600, 400), 1.0);
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::AddVectorLayer);
    // Deterministic pipeline for the replay comparison: no stabilizer pull,
    // no mouse smoothing floor.
    app.props_current.stabilizer = 0.0;
    app.props_current.size_px = 8.0;
    app.prefs.mouse_smooth_px = 0.0;
    app.apply_props();
    app.viewport.zoom = 1.0;
    app.viewport.pan = [0.0, 0.0];
    Some(app)
}

fn drag(app: &mut App, y: f32) {
    app.begin_stroke(PointerKind::Mouse);
    let batch: Vec<PenSample> = (0..30)
        .map(|i| PenSample {
            x: 60.0 + i as f32 * 6.0,
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

fn layer_alpha(app: &App, li: usize) -> u64 {
    let mut sum = 0u64;
    for (_, t) in app.doc.layers[li].tiles() {
        for p in 0..mn_core::TILE_PIXELS {
            sum += u64::from(t.pixel(p % 64, p / 64)[3]);
        }
    }
    sum
}

/// One drawn stroke = a recorded stroke; ONE undo takes ink and record
/// together; redo restores both.
#[test]
fn a_stroke_records_and_undoes_as_one_step() {
    let Some(mut app) = vector_app() else { return };
    let li = app.doc.active;
    assert!(
        app.doc.layers[li].strokes.is_some(),
        "AddVectorLayer arms it"
    );

    drag(&mut app, 200.0);
    assert!(layer_alpha(&app, li) > 0, "the stroke inked normally");
    let set_len = |app: &App| app.doc.layers[li].strokes.as_ref().unwrap().strokes.len();
    assert_eq!(set_len(&app), 1, "…and recorded");
    let rec = &app.doc.layers[li].strokes.as_ref().unwrap().strokes[0];
    assert!(rec.points.len() >= 30, "the samples are all there");
    assert!((rec.size_px - app.props_current.size_px).abs() < 1e-3);

    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Undo);
    assert_eq!(layer_alpha(&app, li), 0, "one undo removes the ink");
    assert_eq!(set_len(&app), 0, "…and the record, same step");
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Redo);
    assert!(layer_alpha(&app, li) > 0);
    assert_eq!(set_len(&app), 1);
}

/// The captured record REPRODUCES the ink: undo the stroke, feed the
/// recorded samples back through the live pipeline, and the tiles match
/// byte-for-byte. This is the property every later edit (move, trim,
/// re-width) replays on.
#[test]
fn the_record_reproduces_the_ink_exactly() {
    let Some(mut app) = vector_app() else { return };
    let li = app.doc.active;
    drag(&mut app, 220.0);

    let snapshot: std::collections::BTreeMap<TileIdx, Vec<u16>> = app.doc.layers[li]
        .tiles()
        .map(|(idx, t)| (idx, t.data().to_vec()))
        .collect();
    let recorded = app.doc.layers[li].strokes.as_ref().unwrap().strokes[0].clone();

    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Undo);
    assert_eq!(layer_alpha(&app, li), 0);

    // Replay ENTERS AT THE ENGINE — the exact stage the capture taps
    // (the samples are already canvas-space and resampled; `push_batch`
    // would transform and resample them a second time) — and on a FRESH
    // engine built from the recorded preset: libmypaint's brush states
    // (speed filters, radius smoothing) persist across strokes by design,
    // so a same-engine replay starts mid-state and inks measurably fatter.
    // This is the exact recipe later phases' re-derivation uses.
    let path = app.presets[app.selected_preset.unwrap()].1.clone();
    let fresh = mn_brush::MyBrush::load(&path).expect("preset reloads");
    *app.engine_mut() = Engine::new(EngineKind::My(Box::new(fresh)));
    app.apply_props();
    app.apply_draw_state();
    app.doc.begin_op();
    app.brush.begin(&mut app.doc);
    for s in recorded.samples() {
        app.brush.sample(&mut app.doc, s);
    }
    app.brush.end(&mut app.doc);
    app.doc.end_op();

    let replayed: std::collections::BTreeMap<TileIdx, Vec<u16>> = app.doc.layers[li]
        .tiles()
        .map(|(idx, t)| (idx, t.data().to_vec()))
        .collect();
    let mut max_diff = 0u16;
    let mut diff_channels = 0usize;
    for (idx, a) in &snapshot {
        let b = replayed.get(idx).cloned().unwrap_or_default();
        for (i, &av) in a.iter().enumerate() {
            let bv = b.get(i).copied().unwrap_or(0);
            let d = av.abs_diff(bv);
            if d > 0 {
                diff_channels += 1;
                max_diff = max_diff.max(d);
            }
        }
    }
    let bbox = |m: &std::collections::BTreeMap<TileIdx, Vec<u16>>| {
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for (idx, d) in m {
            let (ox, oy) = idx.origin();
            for p in 0..(64 * 64) {
                if d[p * 4 + 3] > 0 {
                    let (x, y) = (ox + (p % 64) as i32, oy + (p / 64) as i32);
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        (x0, y0, x1, y1)
    };
    eprintln!(
        "[dbg] tiles {} vs {}, max diff {max_diff}, channels {diff_channels}, bbox {:?} vs {:?}",
        snapshot.len(),
        replayed.len(),
        bbox(&snapshot),
        bbox(&replayed)
    );
    assert_eq!(replayed, snapshot, "replayed ink differs from the original");
}

/// Phase 2: grabbing a stroke's body with the Object tool and dragging
/// TRANSLATES it — geometry and re-derived ink together — and one undo
/// restores both exactly.
#[test]
fn translating_a_stroke_moves_ink_and_geometry_as_one_step() {
    let Some(mut app) = vector_app() else { return };
    let li = app.doc.active;
    drag(&mut app, 200.0);
    let tiles_before: std::collections::BTreeMap<TileIdx, Vec<u16>> = app.doc.layers[li]
        .tiles()
        .map(|(idx, t)| (idx, t.data().to_vec()))
        .collect();
    let geom_before = app.doc.layers[li].strokes.as_ref().unwrap().strokes[0].clone();
    let steps_before = app.doc.undo_len();

    // Grab the stroke mid-body (a sample point sits at x=120,y=200) and
    // drag 40 px right through the real press/move/release path.
    app.tool = Tool::Object;
    assert!(
        app.vector_hit(120.0, 200.0, false),
        "the stroke takes the press"
    );
    assert!(app.vector_drag_move(160.0, 200.0));
    assert!(app.vector_drag_release());

    let moved = &app.doc.layers[li].strokes.as_ref().unwrap().strokes[0];
    assert!(
        (moved.points[0].0 - (geom_before.points[0].0 + 40.0)).abs() < 1e-3,
        "geometry translated"
    );
    assert_eq!(app.doc.undo_len(), steps_before + 1, "one step per gesture");
    // The ink moved with it: the original left edge is now blank.
    let col = |app: &App, x: i32| -> u64 {
        let mut sum = 0;
        for y in 180..220 {
            let idx = TileIdx::of_pixel(x, y);
            if let Some(t) = app.doc.layers[li].tile(idx) {
                sum += u64::from(
                    t.pixel((x - idx.origin().0) as usize, (y - idx.origin().1) as usize)[3],
                );
            }
        }
        sum
    };
    assert_eq!(col(&app, 58), 0, "ink left the old start");
    assert!(col(&app, 98) > 0, "…and begins at the new one");

    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Undo);
    let tiles_after_undo: std::collections::BTreeMap<TileIdx, Vec<u16>> = app.doc.layers[li]
        .tiles()
        .map(|(idx, t)| (idx, t.data().to_vec()))
        .collect();
    assert_eq!(
        tiles_after_undo, tiles_before,
        "undo restores the ink exactly"
    );
    assert_eq!(
        app.doc.layers[li].strokes.as_ref().unwrap().strokes[0],
        geom_before,
        "…and the geometry, same step"
    );
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Redo);
    assert!(
        (app.doc.layers[li].strokes.as_ref().unwrap().strokes[0].points[0].0
            - (geom_before.points[0].0 + 40.0))
            .abs()
            < 1e-3
    );
    assert!(col(&app, 98) > 0);
}

/// Phase 2: dragging one POINT deforms locally — the grabbed sample moves
/// fully, distant samples stay put (raised-cosine falloff).
#[test]
fn a_point_drag_deforms_locally() {
    let Some(mut app) = vector_app() else { return };
    let li = app.doc.active;
    drag(&mut app, 200.0);
    app.tool = Tool::Object;
    // Grab exactly the first sample (x=60) and pull it up 30 px.
    assert!(app.vector_hit(60.0, 200.0, false));
    assert!(
        app.vector_drag.as_ref().unwrap().point.is_some(),
        "point grab"
    );
    assert!(app.vector_drag_move(60.0, 170.0));
    assert!(app.vector_drag_release());
    let s = &app.doc.layers[li].strokes.as_ref().unwrap().strokes[0];
    assert!(
        (s.points[0].1 - 170.0).abs() < 1.0,
        "the grabbed point followed"
    );
    let far = s.points.last().unwrap();
    assert!(
        (far.1 - 200.0).abs() < 1e-3,
        "the far end never moved (falloff): {}",
        far.1
    );
}

/// Phase 3, the headline: the eraser on a vector layer TRIMS — the touched
/// span dies up to the neighbouring crossings, the rest survives as split
/// strokes, and one undo restores geometry and ink exactly.
#[test]
fn the_eraser_trims_to_the_crossings_and_undoes_as_one() {
    let Some(mut app) = vector_app() else { return };
    let li = app.doc.active;
    // A horizontal crossed by two verticals.
    drag(&mut app, 200.0); // horizontal: x 60..234 at y 200
    let vert = |app: &mut App, x: f32| {
        app.begin_stroke(PointerKind::Mouse);
        let batch: Vec<PenSample> = (0..20)
            .map(|i| PenSample {
                x,
                y: 160.0 + i as f32 * 4.0,
                pressure: 0.9,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: i as f64 * 8.0,
            })
            .collect();
        app.push_batch(&batch);
        app.end_stroke();
    };
    vert(&mut app, 100.0);
    vert(&mut app, 190.0);
    assert_eq!(
        app.doc.layers[li].strokes.as_ref().unwrap().strokes.len(),
        3
    );
    let tiles_before: std::collections::BTreeMap<TileIdx, Vec<u16>> = app.doc.layers[li]
        .tiles()
        .map(|(idx, t)| (idx, t.data().to_vec()))
        .collect();
    let geom_before = app.doc.layers[li].strokes.clone().unwrap();
    let steps_before = app.doc.undo_len();

    // Eraser drag crossing the horizontal at x=150 — between the verticals.
    app.tool = Tool::Eraser;
    app.apply_draw_state();
    app.begin_stroke(PointerKind::Mouse);
    let batch: Vec<PenSample> = (0..10)
        .map(|i| PenSample {
            x: 150.0,
            y: 190.0 + i as f32 * 2.0,
            pressure: 0.9,
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_ms: i as f64 * 8.0,
        })
        .collect();
    app.push_batch(&batch);
    app.end_stroke();

    let set = app.doc.layers[li].strokes.as_ref().unwrap();
    assert_eq!(
        set.strokes.len(),
        4,
        "split into two pieces + two verticals"
    );
    assert_eq!(
        app.doc.undo_len(),
        steps_before + 1,
        "one step for the trim"
    );
    let alpha_at = |app: &App, x: i32, y: i32| -> u16 {
        let idx = TileIdx::of_pixel(x, y);
        app.doc.layers[li]
            .tile(idx)
            .map(|t| t.pixel((x - idx.origin().0) as usize, (y - idx.origin().1) as usize)[3])
            .unwrap_or(0)
    };
    assert_eq!(
        alpha_at(&app, 150, 200),
        0,
        "the trimmed span's ink is gone"
    );
    assert!(alpha_at(&app, 70, 200) > 0, "the left piece survives");
    assert!(alpha_at(&app, 220, 200) > 0, "the right piece survives");
    assert!(alpha_at(&app, 100, 170) > 0, "the verticals survive");

    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Undo);
    let tiles_after: std::collections::BTreeMap<TileIdx, Vec<u16>> = app.doc.layers[li]
        .tiles()
        .map(|(idx, t)| (idx, t.data().to_vec()))
        .collect();
    assert_eq!(tiles_after, tiles_before, "undo restores the ink exactly");
    assert_eq!(
        app.doc.layers[li].strokes.as_ref().unwrap(),
        &geom_before,
        "…and all three strokes"
    );
}

/// An eraser that touches NO stroke reverts its own live erase and spends
/// no undo step.
#[test]
fn an_eraser_miss_spends_nothing() {
    let Some(mut app) = vector_app() else { return };
    let li = app.doc.active;
    drag(&mut app, 200.0);
    let alpha_before = layer_alpha(&app, li);
    let steps_before = app.doc.undo_len();
    app.tool = Tool::Eraser;
    app.apply_draw_state();
    app.begin_stroke(PointerKind::Mouse);
    let batch: Vec<PenSample> = (0..10)
        .map(|i| PenSample {
            x: 60.0 + i as f32 * 6.0,
            y: 320.0,
            pressure: 0.9,
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_ms: i as f64 * 8.0,
        })
        .collect();
    app.push_batch(&batch);
    app.end_stroke();
    assert_eq!(app.doc.undo_len(), steps_before, "no step spent");
    assert_eq!(layer_alpha(&app, li), alpha_before, "no ink changed");
    assert_eq!(
        app.doc.layers[li].strokes.as_ref().unwrap().strokes.len(),
        1
    );
}

/// Phase 4: Alt-drag re-widths — dragging DOWN thins the pressure channel
/// around the grab (raised-cosine, three brush-widths), the far end keeps
/// its width, the re-derived ink thins where the pressure did, and one
/// undo restores geometry and ink exactly.
#[test]
fn alt_drag_rewidths_locally_and_undoes_as_one() {
    let Some(mut app) = vector_app() else { return };
    let li = app.doc.active;
    drag(&mut app, 200.0);
    let tiles_before: std::collections::BTreeMap<TileIdx, Vec<u16>> = app.doc.layers[li]
        .tiles()
        .map(|(idx, t)| (idx, t.data().to_vec()))
        .collect();
    let col_alpha = |app: &App, x: i32| -> u64 {
        let mut sum = 0;
        for y in 180..220 {
            let idx = TileIdx::of_pixel(x, y);
            if let Some(t) = app.doc.layers[li].tile(idx) {
                sum += u64::from(
                    t.pixel((x - idx.origin().0) as usize, (y - idx.origin().1) as usize)[3],
                );
            }
        }
        sum
    };
    let (mid_before, end_before) = (col_alpha(&app, 150), col_alpha(&app, 62));
    let steps_before = app.doc.undo_len();

    app.tool = Tool::Object;
    assert!(
        app.vector_hit(150.0, 200.0, true),
        "alt grab takes the stroke"
    );
    assert!(app.vector_drag.as_ref().unwrap().width);
    assert!(app.vector_drag_move(150.0, 300.0)); // 100 px down = half width
    assert!(app.vector_drag_release());

    let s = &app.doc.layers[li].strokes.as_ref().unwrap().strokes[0];
    let near = s
        .points
        .iter()
        .min_by(|a, b| (a.0 - 150.0).abs().total_cmp(&(b.0 - 150.0).abs()));
    assert!(near.unwrap().2 < 0.55, "pressure halved at the grab");
    assert!(
        (s.points[0].2 - 0.9).abs() < 1e-3,
        "the far end kept its width"
    );
    assert_eq!(app.doc.undo_len(), steps_before + 1);
    assert!(
        col_alpha(&app, 150) < mid_before,
        "the ink thinned where the pressure did"
    );

    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Undo);
    let tiles_after: std::collections::BTreeMap<TileIdx, Vec<u16>> = app.doc.layers[li]
        .tiles()
        .map(|(idx, t)| (idx, t.data().to_vec()))
        .collect();
    assert_eq!(tiles_after, tiles_before);
    let _ = end_before;
}

/// Del with a selected stroke deletes it — ink and record, one step.
#[test]
fn deleting_a_selected_stroke_is_one_step() {
    let Some(mut app) = vector_app() else { return };
    let li = app.doc.active;
    drag(&mut app, 200.0);
    app.tool = Tool::Object;
    assert!(app.vector_hit(120.0, 200.0, false));
    app.vector_drag = None; // press selected; no drag
    let si = app.vector_sel.unwrap();
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::VectorDelete { stroke: si });
    assert_eq!(
        app.doc.layers[li].strokes.as_ref().unwrap().strokes.len(),
        0
    );
    assert_eq!(layer_alpha(&app, li), 0);
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Undo);
    assert_eq!(
        app.doc.layers[li].strokes.as_ref().unwrap().strokes.len(),
        1
    );
    assert!(layer_alpha(&app, li) > 0);
}

/// Drawing selects what you JUST drew: reaching for the Object tool after
/// inking must not light up a stroke picked earlier, and undo must never
/// leave the selection pointing at a stroke that is gone.
#[test]
fn drawing_selects_the_newest_stroke() {
    let Some(mut app) = vector_app() else { return };
    let li = app.doc.active;
    drag(&mut app, 200.0);
    assert_eq!(app.vector_sel, Some(0), "the first stroke selects itself");

    // Select the first stroke by hand, then draw a second: the stale pick
    // must not survive (the owner-reported bug).
    app.tool = Tool::Object;
    assert!(app.vector_hit(120.0, 200.0, false));
    app.vector_drag = None;
    assert_eq!(app.vector_sel, Some(0));
    app.tool = Tool::Pen;
    app.apply_draw_state();
    drag(&mut app, 260.0);
    let count = |app: &App| app.doc.layers[li].strokes.as_ref().unwrap().strokes.len();
    assert_eq!(count(&app), 2);
    assert_eq!(
        app.vector_sel,
        Some(1),
        "the newest stroke is the selected one"
    );

    // Undo the newest stroke: the selection must not dangle past the set.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Undo);
    assert_eq!(count(&app), 1);
    assert!(
        app.vector_sel.is_none_or(|si| si < count(&app)),
        "selection dangles after undo: {:?}",
        app.vector_sel
    );
}

/// Ordinary layers keep ordinary strokes: nothing records, undo behaves
/// exactly as before.
#[test]
fn plain_layers_do_not_record() {
    let Some(mut app) = vector_app() else { return };
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::AddLayer);
    let li = app.doc.active;
    assert!(app.doc.layers[li].strokes.is_none());
    drag(&mut app, 240.0);
    assert!(layer_alpha(&app, li) > 0);
    assert_eq!(
        app.vector_sel, None,
        "a non-recording stroke selects nothing"
    );
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Undo);
    assert_eq!(layer_alpha(&app, li), 0);
}
