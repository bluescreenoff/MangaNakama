//! TRIAGE row 36 (`L-001`/`L-002`): the magnetic lasso end to end through
//! `canvas_down` / `canvas_move` / `canvas_up`, and row 38 (`S-001`): what
//! the Select layer pick refuses to land on.
//!
//! The cost function itself is measured in `mn_core::magnetic`'s own tests —
//! synthetic pages with a known edge. What is tested here is the WIRING:
//! that a rough drag becomes a snapped selection, that the trace can always
//! be got out of, and that the Exclude switches do what they say.

use super::{App, PointerKind, headless_renderer};
use crate::cmd::{AppCmd, ObjectMode, SelectMode, Tool, dispatch};
use mn_core::PenSample;
use mn_core::tile::TileIdx;

fn ink(app: &mut App, x: i32, y: i32) {
    let idx = TileIdx::of_pixel(x, y);
    let (ox, oy) = idx.origin();
    app.doc.active_layer_mut().tile_mut(idx).set_pixel(
        (x - ox) as usize,
        (y - oy) as usize,
        [0, 0, 0, 32768],
    );
}

fn pump(app: &mut App) {
    while let Some(c) = app.cmds.pop_front() {
        dispatch(app, c);
    }
}

fn magnetic_app() -> Option<App> {
    let mut app = App::new(headless_renderer()?, (256, 256), 1.0);
    app.viewport = mn_gpu::Viewport::default(); // canvas == client
    app.tool = Tool::Select;
    dispatch(&mut app, AppCmd::SetSelectMode(SelectMode::Magnetic));
    Some(app)
}

/// The whole gesture: press, drag roughly around a black block, lift, close.
/// The traced outline has to hug the block — a drag that stays 6 px outside
/// it comes back as a selection that ends AT the ink, which is the entire
/// reason the tool exists.
#[test]
fn a_rough_magnetic_drag_closes_on_the_shape_it_traced() {
    let Some(mut app) = magnetic_app() else {
        return;
    };
    app.doc.begin_op();
    for x in 60..=180 {
        for y in 60..=180 {
            ink(&mut app, x, y);
        }
    }
    app.doc.end_op();

    let empty: [PenSample; 0] = [];
    // A deliberately sloppy loop: every corner is 6 px outside the block.
    let corners = [(54.0, 54.0), (186.0, 54.0), (186.0, 186.0), (54.0, 186.0)];
    app.canvas_down(corners[0].0, corners[0].1, PointerKind::Pen, &empty);
    assert!(app.magnetic.is_some(), "the press opened a trace");
    for w in corners.windows(2) {
        let (a, b) = (w[0], w[1]);
        for i in 1..=16 {
            let t = i as f32 / 16.0;
            app.canvas_move(a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, &empty);
        }
    }
    app.canvas_up(corners[3].0, corners[3].1, &empty);
    assert!(
        app.magnetic.as_ref().is_some_and(|l| l.anchors().len() > 1),
        "the drag auto-anchored on the way round"
    );
    app.magnetic_close();
    pump(&mut app);
    assert!(app.magnetic.is_none(), "closing ends the trace");

    let sel = app.doc.selection.as_ref().expect("the trace selected");
    let on = |x: i32, y: i32| mn_core::selection::selected(sel.coverage(x, y));
    assert!(on(120, 120), "the block's middle is inside");
    assert!(on(120, 62), "and so is ink just inside its top edge");
    assert!(!on(20, 20), "the paper outside is not");
    assert!(
        !on(120, 220),
        "nor is paper well below it — the wire came home along the edge"
    );
}

/// Esc throws the trace away; Backspace walks it back one anchor at a time
/// and cancels at the first, so the key is never dead.
#[test]
fn backspace_walks_the_anchors_back_and_escape_drops_the_trace() {
    let Some(mut app) = magnetic_app() else {
        return;
    };
    let empty: [PenSample; 0] = [];
    app.canvas_down(40.0, 40.0, PointerKind::Pen, &empty);
    app.canvas_up(40.0, 40.0, &empty);
    app.canvas_down(120.0, 40.0, PointerKind::Pen, &empty);
    app.canvas_up(120.0, 40.0, &empty);
    assert_eq!(app.magnetic.as_ref().unwrap().anchors().len(), 2);

    app.magnetic_undo_anchor();
    assert_eq!(app.magnetic.as_ref().unwrap().anchors().len(), 1);
    app.magnetic_undo_anchor();
    assert!(app.magnetic.is_none(), "the last Backspace cancelled");

    app.canvas_down(40.0, 40.0, PointerKind::Pen, &empty);
    app.magnetic_cancel();
    assert!(app.magnetic.is_none(), "Esc drops it");
    assert!(app.doc.selection.is_none(), "and selects nothing");
}

/// Leaving the tool mid-trace must not strand an outline on the overlay that
/// no gesture can close (it also holds the edge cache, so it is memory too).
#[test]
fn leaving_the_tool_mid_trace_drops_the_outline() {
    let Some(mut app) = magnetic_app() else {
        return;
    };
    let empty: [PenSample; 0] = [];
    app.canvas_down(40.0, 40.0, PointerKind::Pen, &empty);
    assert!(app.magnetic.is_some());
    dispatch(&mut app, AppCmd::SetTool(Tool::Pen));
    assert!(app.magnetic.is_none(), "the tool switch dropped it");

    app.tool = Tool::Select;
    dispatch(&mut app, AppCmd::SetSelectMode(SelectMode::Magnetic));
    app.canvas_down(40.0, 40.0, PointerKind::Pen, &empty);
    dispatch(&mut app, AppCmd::SetSelectMode(SelectMode::Lasso));
    assert!(app.magnetic.is_none(), "so did the sub-tool switch");
}

/// S-001: the pick answers with the TOPMOST eligible layer, and the Exclude
/// switches are what make it usable on a finished page.
#[test]
fn the_layer_pick_skips_the_kinds_it_is_told_to_exclude() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (256, 256), 1.0);
    // Three layers all drawing the same pixel: plain, draft, locked.
    ink(&mut app, 100, 100);
    let plain = app.doc.active;
    let draft = app.doc.add_layer("rough");
    ink(&mut app, 100, 100);
    app.doc.set_layer_draft(draft, true);
    let locked = app.doc.add_layer("finished");
    ink(&mut app, 100, 100);
    app.doc.layers[locked].lock = true;

    assert_eq!(
        app.layer_at(100, 100),
        Some(plain),
        "the defaults skip the draft AND the locked layer"
    );
    app.pick_exclude.locked = false;
    assert_eq!(
        app.layer_at(100, 100),
        Some(locked),
        "allow locked and the topmost wins"
    );
    app.doc.layers[locked].visible = false;
    assert_eq!(
        app.layer_at(100, 100),
        Some(plain),
        "a hidden layer is never picked, whatever the switches say"
    );
    app.pick_exclude.draft = false;
    assert_eq!(
        app.layer_at(100, 100),
        Some(draft),
        "allow drafts and the rough layer is reachable again"
    );
    assert_eq!(app.layer_at(10, 10), None, "blank paper picks nothing");
}

/// The pick is a CLICK, not a drag: it moves the active layer and starts no
/// object gesture, and a miss says why rather than doing nothing.
#[test]
fn clicking_with_select_layer_jumps_the_palette_to_that_layer() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (256, 256), 1.0);
    app.viewport = mn_gpu::Viewport::default();
    ink(&mut app, 100, 100); // layer 0
    let top = app.doc.add_layer("upper");
    ink(&mut app, 180, 180); // layer 1, somewhere else
    app.doc.active = top;

    app.tool = Tool::Object;
    app.object_mode = ObjectMode::PickLayer;
    let empty: [PenSample; 0] = [];
    app.canvas_down(100.0, 100.0, PointerKind::Pen, &empty);
    pump(&mut app);
    assert_eq!(app.doc.active, 0, "the click chose the layer under it");
    assert!(app.object_drag.is_none(), "no object gesture was started");

    app.canvas_down(10.0, 10.0, PointerKind::Pen, &empty);
    pump(&mut app);
    assert_eq!(app.doc.active, 0, "a miss leaves the active layer alone");
    assert!(
        app.status.contains("Exclude"),
        "and says why: {}",
        app.status
    );
}
