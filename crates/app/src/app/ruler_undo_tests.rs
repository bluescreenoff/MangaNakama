//! ROADMAP good-first-issue: **undo for ruler creation and moves.**
//!
//! Rulers used to mutate app state outside every recording bracket, so
//! neither a creation drag nor the newer move gesture could be taken back.
//! They live on the `Document` now — one undo history, no app-level undo
//! species beside it — and these tests pin the four rules that decision
//! carries: one step per gesture, exact geometry on the way back, the
//! frame-PUBLISHED curves are derived state and spend no step at all, and
//! a page switch (which decodes a fresh `Document` from bytes rulers never
//! ride) does not silently wipe the set.

use super::*;
use crate::cmd::{AppCmd, RulerKind, dispatch};
use mn_core::PenSample;

const NONE: [PenSample; 0] = [];

/// Anchors within a hair of the wanted ones — the creation drag goes
/// through screen coordinates, so its canvas px come back with float
/// rounding on them (the part-1 ruler tests use the same tolerance).
fn anchors_near(r: &mn_core::Ruler, want: &[[f32; 2]]) -> bool {
    let got = r.anchors();
    got.len() == want.len()
        && got
            .iter()
            .zip(want)
            .all(|(g, w)| (g[0] - w[0]).abs() < 0.05 && (g[1] - w[1]).abs() < 0.05)
}

/// Draw the part-1 line ruler from `a` to `b`, canvas px.
fn create_line_ruler(app: &mut App, a: [f32; 2], b: [f32; 2]) {
    dispatch(app, AppCmd::RulerArm(RulerKind::Line));
    let (x0, y0) = app.viewport.to_screen(a[0], a[1]);
    let (x1, y1) = app.viewport.to_screen(b[0], b[1]);
    app.canvas_down(x0, y0, PointerKind::Mouse, &NONE);
    app.canvas_up(x1, y1, &NONE);
}

/// One creation drag = one undo step, labelled for the History palette;
/// undo takes the ruler away again and redo brings it back.
#[test]
fn creating_a_ruler_is_one_undoable_step() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    app.viewport.zoom = 1.0;
    let steps = app.doc.undo_len();

    create_line_ruler(&mut app, [100.0, 200.0], [400.0, 200.0]);
    assert_eq!(app.doc.rulers.items.len(), 1, "the drag made one ruler");
    let made = app.doc.rulers.items[0];
    assert!(anchors_near(&made, &[[100.0, 200.0], [400.0, 200.0]]));
    assert_eq!(app.doc.undo_len(), steps + 1, "and spent exactly one step");
    assert_eq!(app.doc.undo_labels().last().unwrap(), "Add ruler");

    dispatch(&mut app, AppCmd::Undo);
    assert!(
        app.doc.rulers.items.is_empty(),
        "undo removes the created ruler: {:?}",
        app.doc.rulers.items
    );
    assert!(!app.doc.rulers.on, "and the snap switch it turned on");

    dispatch(&mut app, AppCmd::Redo);
    assert_eq!(app.doc.rulers.items, vec![made], "redo puts it back");
    assert!(app.doc.rulers.on);
}

/// A ruler MOVE is one step for the whole drag, not one per pointer
/// event: the drag applies deltas live, the snapshot is taken at the grab
/// and pushed at release. Undo restores the exact geometry, redo reapplies
/// it — and a press that grabs without travelling records nothing.
#[test]
fn moving_a_ruler_is_one_step_and_undo_restores_the_geometry() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    app.viewport.zoom = 1.0;
    create_line_ruler(&mut app, [100.0, 200.0], [400.0, 200.0]);
    let made = app.doc.rulers.items[0];
    app.tool = crate::cmd::Tool::Object;
    let steps = app.doc.undo_len();

    // A press on the body that never moves: a gesture, but not an edit.
    let (bx, by) = app.viewport.to_screen(250.0, 200.0);
    app.canvas_down(bx, by, PointerKind::Mouse, &NONE);
    app.canvas_up(bx, by, &NONE);
    assert_eq!(
        app.doc.undo_len(),
        steps,
        "a grab that moved nothing is free"
    );

    // Carry it 100 px down in three pointer moves — still ONE step.
    app.canvas_down(bx, by, PointerKind::Mouse, &NONE);
    for dy in [30.0, 70.0, 100.0] {
        let (mx, my) = app.viewport.to_screen(250.0, 200.0 + dy);
        app.canvas_move(mx, my, &NONE);
    }
    let (ux, uy) = app.viewport.to_screen(250.0, 300.0);
    app.canvas_up(ux, uy, &NONE);
    let moved = app.doc.rulers.items[0];
    assert!(
        anchors_near(&moved, &[[100.0, 300.0], [400.0, 300.0]]),
        "the whole ruler carried 100 px down: {moved:?}"
    );
    assert_eq!(
        app.doc.undo_len(),
        steps + 1,
        "the whole drag is one undo step"
    );
    assert_eq!(app.doc.undo_labels().last().unwrap(), "Move ruler");

    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(
        app.doc.rulers.items,
        vec![made],
        "back to where it was drawn"
    );
    assert!(
        app.ruler_move.is_none(),
        "no in-flight grab survives a restore"
    );
    assert!(
        app.ruler_lock.ruler.is_none(),
        "and no sticky lock into the replaced set"
    );
    dispatch(&mut app, AppCmd::Redo);
    assert_eq!(app.doc.rulers.items, vec![moved], "redo reapplies the move");

    // Undo twice more: the move, then the creation.
    dispatch(&mut app, AppCmd::Undo);
    dispatch(&mut app, AppCmd::Undo);
    assert!(app.doc.rulers.items.is_empty());
}

/// `Clear rulers` is one step called "Delete rulers", and undo brings the
/// hand-made curves back — while the frame-PUBLISHED outline, which the
/// clear deliberately leaves alone (`sync_frame_rulers` retracts it by
/// value), is neither dropped nor doubled.
#[test]
fn undoing_a_ruler_clear_restores_the_hand_made_curves_only() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let mine = mn_core::CurveRuler {
        pts: vec![[0.0, 0.0], [10.0, 10.0]],
    };
    app.doc.rulers.curves.push(mine.clone());
    app.doc.rulers.items.push(mn_core::Ruler::Line {
        a: [0.0, 0.0],
        b: [50.0, 50.0],
    });
    let h = app.doc.add_frame_folder(
        "Frame 1",
        mn_core::FrameSet::single_rect([20.0, 20.0, 380.0, 280.0], 4.0),
    );
    dispatch(&mut app, AppCmd::FrameBorderRuler { layer: h });
    let published = app.frame_rulers.clone();
    assert_eq!(published.len(), 1, "the panel outline is published");
    let steps = app.doc.undo_len();

    dispatch(&mut app, AppCmd::RulerClear);
    assert_eq!(app.doc.undo_len(), steps + 1);
    assert_eq!(app.doc.undo_labels().last().unwrap(), "Delete rulers");
    assert_eq!(
        app.doc.rulers.curves, published,
        "only the frame's own left"
    );

    dispatch(&mut app, AppCmd::Undo);
    assert!(
        app.doc.rulers.curves.contains(&mine),
        "undo brings the hand-made curve back: {:?}",
        app.doc.rulers.curves
    );
    assert_eq!(
        app.doc.rulers.curves.len(),
        2,
        "hand-made + the one published outline, not a second copy"
    );
    assert_eq!(app.doc.rulers.curves, vec![mine, published[0].clone()]);
    assert_eq!(app.doc.rulers.items.len(), 1, "and the line family with it");
    // The bookkeeping still describes the live set, so the frame can take
    // its curve back with nothing stranded.
    dispatch(&mut app, AppCmd::FrameBorderRuler { layer: h });
    assert!(app.frame_rulers.is_empty());
    assert!(
        !app.doc.rulers.curves.iter().any(|c| *c == published[0]),
        "retraction still found its own curve after the undo"
    );
}

/// The frame-published curves are DERIVED state, not user state: toggling
/// a panel's border-as-ruler republishes them through `sync_frame_rulers`,
/// which writes the ruler set directly and records nothing. The border
/// toggle costs its own frame step and not one keystroke more.
#[test]
fn publishing_a_frame_border_ruler_spends_no_ruler_undo_step() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let h = app.doc.add_frame_folder(
        "Frame 1",
        mn_core::FrameSet::single_rect([20.0, 20.0, 380.0, 280.0], 4.0),
    );
    let steps = app.doc.undo_len();

    dispatch(&mut app, AppCmd::FrameBorderRuler { layer: h });
    assert_eq!(app.doc.rulers.curves.len(), 1, "the outline is a ruler now");
    assert_eq!(
        app.doc.undo_len(),
        steps + 1,
        "one step, and it is the FRAME's"
    );
    assert!(
        !app.doc
            .undo_labels()
            .iter()
            .any(|l| l == "Add ruler" || l == "Move ruler" || l == "Delete rulers"),
        "the publish spends no ruler step: {:?}",
        app.doc.undo_labels()
    );

    // And the retraction is just as free.
    let steps = app.doc.undo_len();
    dispatch(&mut app, AppCmd::FrameBorderRuler { layer: h });
    assert!(app.doc.rulers.curves.is_empty());
    assert_eq!(app.doc.undo_len(), steps + 1);
}

/// The trap of moving rulers into the `Document`: a page switch DECODES a
/// fresh document from bytes, and rulers are session-only — they are not
/// in those bytes. Before the move they lived on the App and survived a
/// page switch; they still must (`App::adopt_page_doc` carries them), or
/// building a perspective set and flipping to the facing page to check it
/// would throw the set away.
#[test]
fn rulers_survive_a_page_switch() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    app.viewport.zoom = 1.0;
    create_line_ruler(&mut app, [100.0, 200.0], [400.0, 200.0]);
    let made = app.doc.rulers.items[0];

    // A second page, exactly as the page palette makes one.
    let blank = mn_core::Document::new(app.doc.size.0, app.doc.size.1);
    let bytes = mn_core::project::doc_to_bytes(&blank).unwrap();
    let e = app.fresh_page(Some(bytes), None);
    app.pages.push(e);

    app.switch_page(1);
    assert_eq!(app.page_index, 1, "the switch happened: {}", app.status);
    assert_eq!(
        app.doc.rulers.items,
        vec![made],
        "the ruler set followed the tab across the page switch"
    );
    assert!(app.doc.rulers.on, "snapping with it");

    app.switch_page(0);
    assert_eq!(app.doc.rulers.items, vec![made], "and back again");
}

/// CSP: "Hold Shift while dragging to draw a straight line ruler in 45
/// degree increments." Every ruler the drag AIMS obeys it — a parallel
/// ruler for a 流線 block wants to be dead horizontal, and eyeballing a
/// 1600 px drag to within a tenth of a degree is not a thing hands do.
#[test]
fn shift_snaps_a_ruler_creation_drag_to_45_degrees() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let empty: [PenSample; 0] = [];
    app.shell.test_modifiers = Some(egui::Modifiers::SHIFT);
    dispatch(&mut app, AppCmd::RulerArm(RulerKind::Parallel));
    // 5° off horizontal over 400 px: 35 px of rise, well past any epsilon.
    let (x0, y0) = app.viewport.to_screen(100.0, 300.0);
    let (x1, y1) = app.viewport.to_screen(500.0, 335.0);
    app.canvas_down(x0, y0, PointerKind::Mouse, &empty);
    app.canvas_up(x1, y1, &empty);
    let Some(mn_core::Ruler::Parallel { a, b }) = app.doc.rulers.items.last().copied() else {
        panic!("the drag made a parallel ruler: {:?}", app.doc.rulers.items);
    };
    assert!(
        (b[1] - a[1]).abs() < 0.5,
        "shift did not flatten the drag: {a:?} -> {b:?}"
    );
    assert!(
        (b[0] - a[0] - 400.0).abs() < 2.0,
        "the drag's LENGTH is kept, only its angle snaps: {a:?} -> {b:?}"
    );

    // …and without shift the same drag keeps its 5°.
    app.shell.test_modifiers = Some(egui::Modifiers::default());
    dispatch(&mut app, AppCmd::RulerArm(RulerKind::Parallel));
    app.canvas_down(x0, y0, PointerKind::Mouse, &empty);
    app.canvas_up(x1, y1, &empty);
    let Some(mn_core::Ruler::Parallel { a, b }) = app.doc.rulers.items.last().copied() else {
        panic!("the second drag made a parallel ruler");
    };
    assert!((b[1] - a[1] - 35.0).abs() < 2.0, "{a:?} -> {b:?}");
}
