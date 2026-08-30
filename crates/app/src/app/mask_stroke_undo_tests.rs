//! ROADMAP good-first-issue: undo for mask strokes (LM-004).
//!
//! Painting on a layer mask writes the mask's coverage tiles LIVE, per dab,
//! from inside the brush engine's surface callback — it never touches the
//! layer's tiles, so the ordinary `begin_op`/`end_op` pre-image recording
//! sees nothing. The mask bracket (`mask_op_begin`/`mask_op_end`) is what
//! makes the gesture undoable, and it only works if the snapshot is taken
//! BEFORE the first dab: opened at `end_stroke` it captured the finished
//! stroke and "undo" restored the very thing it was asked to remove.

use super::*;
use mn_core::{PenSample, TileIdx};

/// A 600x400 draft with white ink across the top two tiles and a mask that
/// hides everything outside the left-hand 64x128 box (the `tests.rs`
/// mask-edit fixture).
fn masked_app() -> Option<App> {
    let renderer = super::headless_renderer()?;
    let mut app = App::new(renderer, (600, 400), 1.0);
    const W: u16 = mn_core::FIX15_ONE as u16;
    app.doc.begin_op();
    for idx in [TileIdx::new(0, 0), TileIdx::new(1, 0)] {
        let t = app.doc.active_layer_mut().tile_mut(idx);
        for p in 0..mn_core::TILE_PIXELS {
            t.set_pixel(p % 64, p / 64, [W, W, W, W]);
        }
    }
    app.doc.end_op();
    app.doc.selection = Some(mn_core::Selection::from_rect(
        &app.doc, 0.0, 0.0, 64.0, 128.0,
    ));
    assert!(app.doc.mask_outside_selection(0));
    app.doc.selection = None;
    app.viewport.zoom = 1.0;
    app.viewport.pan = [0.0, 0.0];
    // No stabilizer, no mouse-smoothing floor: a smoothed stroke can hold
    // every dab in the pull string until `end_stroke` drains it, which
    // would hide the very bug these tests exist for.
    app.props_current.stabilizer = 0.0;
    app.prefs.mouse_smooth_px = 0.0;
    Some(app)
}

/// The active layer's whole coverage field, canonically ordered — the
/// bit-identity oracle. `None` = the layer carries no mask at all.
fn mask_bits(app: &App) -> Option<Vec<((i32, i32), Vec<u16>)>> {
    let m = app.doc.active_layer().mask.as_ref()?;
    let mut v: Vec<_> = m
        .tiles
        .iter()
        .map(|(idx, t)| ((idx.x, idx.y), t.data().to_vec()))
        .collect();
    v.sort_by_key(|(k, _)| *k);
    Some(v)
}

/// The active layer's own pixels, same shape — for "the mask stroke never
/// touched the art" and for the plain-stroke control.
fn layer_bits(app: &App) -> Vec<((i32, i32), Vec<u16>)> {
    let mut v: Vec<_> = app
        .doc
        .active_layer()
        .tiles()
        .map(|(idx, t)| ((idx.x, idx.y), t.data().to_vec()))
        .collect();
    v.sort_by_key(|(k, _)| *k);
    v
}

/// One batch of a long horizontal drag starting at a canvas point. The
/// stabilizer is turned off by `masked_app` so these dabs land WHILE the
/// stroke is live — which is the whole point: the mask snapshot has to
/// predate them, and the pen never buffers a whole stroke for us.
fn drag_batch(app: &mut App, x: f32, y: f32, from: usize, to: usize) {
    let (sx, sy) = app.viewport.to_screen(x, y);
    app.push_batch(
        &(from..to)
            .map(|i| PenSample {
                x: sx + i as f32 * 4.0,
                y: sy,
                pressure: 1.0,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: i as f64 * 16.0,
            })
            .collect::<Vec<_>>(),
    );
}

/// A whole gesture: pen down, two batches of drag, pen up.
fn drag(app: &mut App, x: f32, y: f32) {
    app.begin_stroke(PointerKind::Pen);
    drag_batch(app, x, y, 0, 20);
    drag_batch(app, x, y, 20, 40);
    app.end_stroke();
}

/// The issue itself: one mask stroke is ONE undo step, undo puts the
/// coverage back bit for bit, and redo paints it again. Against the old
/// code (bracket opened inside `end_stroke`) the undo assert fails — the
/// snapshot was taken after the dabs had already landed in the mask.
#[test]
fn a_mask_stroke_undoes_to_the_exact_coverage_it_started_from() {
    let Some(mut app) = masked_app() else {
        return;
    };
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::MaskEdit);
    assert!(app.mask_edit, "mask editing armed");

    let before = mask_bits(&app).expect("the fixture layer has a mask");
    let art_before = layer_bits(&app);
    let steps_before = app.doc.undo_len();

    // Paint on the hidden right-hand half, and check MID-STROKE that the
    // coverage has already moved. This is the premise the bracket has to
    // respect: the engine writes the mask live, per dab, so a snapshot
    // taken at `end_stroke` is a snapshot of the finished stroke.
    app.begin_stroke(PointerKind::Pen);
    drag_batch(&mut app, 100.0, 10.0, 0, 20);
    assert_ne!(
        mask_bits(&app).expect("masked"),
        before,
        "the engine paints the mask while the stroke is still open"
    );
    drag_batch(&mut app, 100.0, 10.0, 20, 40);
    app.end_stroke();

    let after = mask_bits(&app).expect("still masked");
    assert_ne!(before, after, "the stroke moved the coverage");
    assert_eq!(
        layer_bits(&app),
        art_before,
        "a mask stroke leaves the layer's own pixels alone"
    );
    assert_eq!(
        app.doc.undo_len(),
        steps_before + 1,
        "one gesture, one undo step"
    );
    assert_eq!(
        app.doc.undo_labels().last().map(String::as_str),
        Some("Mask stroke"),
        "labelled for the History palette"
    );

    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Undo);
    assert_eq!(
        mask_bits(&app).expect("the mask survives its own undo"),
        before,
        "undo restored the coverage bit for bit"
    );
    assert_eq!(layer_bits(&app), art_before, "and left the art alone");

    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Redo);
    assert_eq!(
        mask_bits(&app).expect("still masked after redo"),
        after,
        "redo repainted the stroke"
    );
}

/// An aborted gesture — pen down, pen up, no samples — must not cost the
/// user an undo press. `mask_op_end` pushes only when the coverage
/// revision actually moved.
#[test]
fn an_empty_mask_stroke_spends_no_undo_step() {
    let Some(mut app) = masked_app() else {
        return;
    };
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::MaskEdit);
    assert!(app.mask_edit);

    let before = mask_bits(&app).expect("masked");
    let steps_before = app.doc.undo_len();
    app.begin_stroke(PointerKind::Mouse);
    app.end_stroke();
    assert_eq!(
        app.doc.undo_len(),
        steps_before,
        "an empty stroke is not an edit"
    );
    assert_eq!(mask_bits(&app).expect("masked"), before, "nothing moved");
}

/// Row 105 edge: a brush stroke on a MASKLESS correction layer used to be
/// refused with a status line ("make one from a selection first") — the
/// maskless-live-fill rule, applied to a layer where it makes no sense.
/// CSP's user reaches for the eraser to take the correction off a face, so
/// the stroke ARMS an all-visible window instead and carves it.
///
/// The window is arm-and-carve in one gesture, so it is also one undo
/// press: the bracket's pre-image is "there was no mask".
#[test]
fn a_stroke_on_a_maskless_correction_arms_its_window_in_one_undo_press() {
    let Some(mut app) = masked_app() else {
        return;
    };
    // A correction over the fixture's ink, maskless (no selection).
    app.doc.selection = None;
    let ci = app
        .doc
        .add_correction_layer(mn_core::Adjust::Invert, false);
    app.doc.set_active(ci);
    assert!(app.doc.active_layer().mask.is_none(), "maskless to start");
    let steps_before = app.doc.undo_len();

    app.tool = crate::cmd::Tool::Eraser;
    drag(&mut app, 100.0, 10.0);

    let m = app
        .doc
        .active_layer()
        .mask
        .as_ref()
        .expect("the stroke armed a window instead of being refused");
    assert!(m.full && m.enabled, "and it is the all-visible kind");
    assert!(
        !m.tiles.is_empty(),
        "the eraser carved coverage into it"
    );
    assert!(
        m.tiles.len() < 8,
        "only the tiles the stroke touched materialised, not the canvas: {}",
        m.tiles.len()
    );
    assert_eq!(
        app.doc.undo_len(),
        steps_before + 1,
        "the arm and the carve are one press"
    );
    assert_eq!(
        app.doc.undo_labels().last().map(String::as_str),
        Some("Correction window"),
    );

    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Undo);
    assert!(
        app.doc.layers[ci].mask.is_none(),
        "one undo took the window back with the stroke"
    );
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Redo);
    assert!(
        app.doc.layers[ci].mask.as_ref().is_some_and(|m| m.full),
        "and redo put it back"
    );
}

/// The other half of the same chokepoint: a maskless live FILL is still
/// refused. Its window is what it DRAWS, so arming one full-canvas would
/// flood the page with the fill colour before the first dab lands.
#[test]
fn a_stroke_on_a_maskless_live_fill_is_still_refused() {
    let Some(mut app) = masked_app() else {
        return;
    };
    app.doc.selection = None;
    let fi = app.doc.add_layer("fill");
    app.doc.layers[fi].kind = mn_core::LayerKind::Fill(mn_core::fill_layer::FillKind::Flat {
        color: [1.0, 0.0, 0.0, 1.0],
    });
    app.doc.set_active(fi);
    let steps_before = app.doc.undo_len();
    drag(&mut app, 100.0, 10.0);
    assert!(
        app.doc.active_layer().mask.is_none(),
        "a live fill must not arm a full-canvas window"
    );
    assert_eq!(app.doc.undo_len(), steps_before, "and spends no undo step");
}

/// The control: with mask editing disarmed the stroke is an ordinary
/// layer edit, recorded by the ordinary op bracket. Its undo behaviour
/// must be exactly what it always was — one step, pixels restored.
#[test]
fn a_plain_stroke_still_undoes_its_own_pixels() {
    let Some(mut app) = masked_app() else {
        return;
    };
    assert!(!app.mask_edit, "not armed");

    let art_before = layer_bits(&app);
    let mask_before = mask_bits(&app).expect("masked");
    let steps_before = app.doc.undo_len();

    drag(&mut app, 300.0, 300.0);

    let art_after = layer_bits(&app);
    assert_ne!(art_after, art_before, "the stroke inked the layer");
    assert_eq!(
        app.doc.undo_len(),
        steps_before + 1,
        "still one step per stroke"
    );
    assert_eq!(
        mask_bits(&app).expect("masked"),
        mask_before,
        "a plain stroke leaves the mask alone"
    );

    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Undo);
    assert_eq!(layer_bits(&app), art_before, "undo took the ink back");
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Redo);
    assert_eq!(layer_bits(&app), art_after, "redo put it back");
}
