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
    assert!(app.doc.layers[li].strokes.is_some(), "AddVectorLayer arms it");

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
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Undo);
    assert_eq!(layer_alpha(&app, li), 0);
}
