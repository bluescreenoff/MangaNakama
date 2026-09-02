//! Mangaka-basics QA, tool usage + switching family (2026-09-02 round).
//!
//! Everything here goes through the REAL key path — `crate::key_down`,
//! which is what `WM_KEYDOWN` calls — and the REAL pointer arms, so what
//! is measured is what a hand pressing the key gets, not what a field
//! poked directly would.

use super::new_document_tests::headless;
use crate::app::{App, PenSample, PointerKind};
use crate::cmd::{AppCmd, Tool, dispatch};

fn pump(app: &mut App) {
    while let Some(c) = app.cmds.pop_front() {
        dispatch(app, c);
    }
}

/// One key press through the whole shortcut path, then the queue drained.
fn press(app: &mut App, vk: u16) {
    crate::key_down(app, vk, false);
    pump(app);
}

fn mods(app: &mut App, ctrl: bool, shift: bool, alt: bool) {
    app.shell.test_modifiers = Some(egui::Modifiers {
        alt,
        ctrl,
        shift,
        mac_cmd: false,
        command: ctrl,
    });
}

/// One pen stroke through the real arms, from canvas a to canvas b.
fn stroke(app: &mut App, a: (f32, f32), b: (f32, f32)) {
    let (x0, y0) = app.viewport.to_screen(a.0, a.1);
    app.canvas_down(x0, y0, PointerKind::Mouse, &[]);
    for i in 1..=16 {
        let t = i as f32 / 16.0;
        let (mx, my) = app
            .viewport
            .to_screen(a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
        app.canvas_move(
            mx,
            my,
            &[PenSample {
                x: mx,
                y: my,
                pressure: 0.9,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: i as f64 * 8.0,
            }],
        );
    }
    let (x1, y1) = app.viewport.to_screen(b.0, b.1);
    app.canvas_up(x1, y1, &[]);
    pump(app);
}

/// Picking a Figure sub tool and dragging it must lay the shape WHERE IT
/// WAS DRAGGED — at whatever zoom and pan the page happens to be sitting.
///
/// This is the tool-switching family's version of the Dot Pen finding: the
/// row changes, the tool property changes, the status line says "rectangle
/// inked", and nothing appears on the page. `ink_figure` builds the shape
/// in CANVAS space and handed it straight to `push_batch`, which takes
/// CLIENT space and runs every sample through `viewport.to_canvas` — so
/// the shape was placed at `to_canvas(canvas_point)`, which is the point
/// itself only at the identity viewport. Every figure test in the suite
/// pins `Viewport::default()`, so the whole family agreed with the bug.
///
/// The assertion is differential on purpose: the SAME drag at zoom 1 and
/// at zoom 0.5 must ink the same rectangle on the page. A single-viewport
/// "did it ink" test would pass on the broken code at zoom 1.
#[test]
fn a_figure_lands_where_it_was_dragged_at_any_zoom() {
    use crate::cmd::{FigureMode, SubTool};
    let Some(mut app) = headless() else { return };
    let shot = |app: &mut App| -> Vec<u8> {
        app.doc = mn_core::Document::new(256, 256);
        dispatch(app, AppCmd::SetSubTool(SubTool::Figure(FigureMode::Rect)));
        dispatch(app, AppCmd::SetBrushSizePx(4.0));
        dispatch(app, AppCmd::SetSlotColor([0.0, 0.0, 0.0]));
        stroke(app, (60.0, 60.0), (190.0, 190.0));
        let (w, h) = app.doc.size;
        let App { renderer, doc, .. } = app;
        let img = super::pages::render_offscreen_drafts_off(renderer, doc, w, h);
        img.pixels().map(|p| p.0[0]).collect()
    };

    app.viewport = mn_gpu::Viewport::default();
    let at_100 = shot(&mut app);
    let ink = |v: &[u8]| v.iter().filter(|&&r| r < 128).count();
    assert!(ink(&at_100) > 1000, "the rectangle inked at all");
    // A corner of the dragged box is on the border; the middle is not.
    let at = |v: &[u8], x: usize, y: usize| v[y * 256 + x];
    assert!(at(&at_100, 60, 62) < 128, "the left edge is where it was dragged");
    assert!(at(&at_100, 125, 125) > 200, "and the box is empty inside");

    // Same drag, page shown at half size and panned — the artist's normal
    // working view, and the one the old code drew nothing at.
    app.viewport = mn_gpu::Viewport {
        pan: [140.0, 90.0],
        zoom: 0.5,
        ..mn_gpu::Viewport::default()
    };
    let zoomed = shot(&mut app);
    assert_eq!(
        ink(&zoomed) > 0,
        true,
        "the figure inked something at 50 % zoom"
    );
    assert!(
        at(&zoomed, 60, 62) < 128 && at(&zoomed, 125, 125) > 200,
        "…and it is the same rectangle, in the same place, not a scaled \
         copy pushed off the page"
    );
}

/// `,` / `.` must SAY where they landed, for every tool that has rows.
///
/// The Sub Tool palette's highlight moves, but `Tab` hides every palette
/// and the status bar carries no tool name, so stepping the Figure tool
/// from Straight line to Rectangle used to change nothing you could see.
/// Fill / Tone / Object said their row; Select, Figure, Frame, Balloon,
/// Gradient, Auto select and the Eyedropper said nothing.
#[test]
fn stepping_a_sub_tool_says_which_one_you_landed_on() {
    let Some(mut app) = headless() else { return };
    for tool in [
        Tool::Select,
        Tool::Figure,
        Tool::Frame,
        Tool::Balloon,
        Tool::Gradient,
        Tool::Wand,
        Tool::Eyedrop,
        Tool::Fill,
        Tool::Tone,
        Tool::Object,
    ] {
        dispatch(&mut app, AppCmd::SetTool(tool));
        app.status.clear();
        press(&mut app, 0xBE); // .
        assert!(
            !app.status.is_empty(),
            "{tool:?}: a sub tool step left the status bar silent"
        );
        // The row it landed on is the one the palette now lights.
        let landed = crate::subtools::step_rows(&app)
            .into_iter()
            .find(|&s| crate::subtools::is_lit(&app, s));
        assert!(
            landed.is_some(),
            "{tool:?}: the step landed on no row the palette lights"
        );
    }
}

/// CSP's Space family, all three of them. Space+drag pans (it always
/// did); Shift+Space+drag rotates; Ctrl+Space / Alt+Space click zoom in
/// and out about the point clicked. Only the pan was here, so rotating
/// the page mid-stroke meant letting go of the pen to reach R.
#[test]
fn the_space_modifiers_pan_rotate_and_zoom_like_csp() {
    let Some(mut app) = headless() else { return };
    app.doc = mn_core::Document::new(256, 256);
    let (x, y) = app.viewport.to_screen(128.0, 128.0);

    let mut with = |ctrl, shift, alt| {
        mods(&mut app, ctrl, shift, alt);
        app.space_down = true;
        app.canvas_down(x, y, PointerKind::Mouse, &[]);
        let state = (app.panning(), app.rotating(), app.viewport.zoom);
        app.canvas_up(x, y, &[]);
        app.end_pan();
        app.space_down = false;
        mods(&mut app, false, false, false);
        state
    };

    let (pan, rot, z0) = with(false, false, false);
    assert!(pan && !rot, "Space+drag pans");
    let (pan, rot, _) = with(false, true, false);
    assert!(rot && !pan, "Shift+Space+drag rotates");
    let (_, _, zin) = with(true, false, false);
    assert!(zin > z0, "Ctrl+Space+click zoomed in ({z0} -> {zin})");
    let (_, _, zout) = with(false, false, true);
    assert!(zout < zin, "Alt+Space+click zoomed out ({zin} -> {zout})");
}

/// Two tools we shipped with no key at all: CSP puts Liquify on `J`
/// (with Blend, which we do not have) and Operation ▸ Select layer on
/// `D`. Both were mouse-only. `D` must land on the ROW, not on whatever
/// row the Object tool was last left on.
#[test]
fn j_and_d_reach_liquify_and_select_layer() {
    use crate::cmd::ObjectMode;
    let Some(mut app) = headless() else { return };
    press(&mut app, 0x4A);
    assert_eq!(app.tool, Tool::Liquify, "J is Liquify");
    app.spring = None;

    // Leave the Object tool on its OTHER row first, so a bare-tool
    // binding would be caught landing back on it.
    dispatch(&mut app, AppCmd::SetTool(Tool::Object));
    app.object_mode = ObjectMode::Object;
    press(&mut app, 0x44);
    assert_eq!(app.tool, Tool::Object);
    assert_eq!(app.object_mode, ObjectMode::PickLayer, "D is Select layer");
    app.spring = None;

    // Ctrl+D still deselects — the tool keys sit after the Ctrl chords.
    mods(&mut app, true, false, false);
    crate::key_down(&mut app, 0x44, false);
    assert!(
        matches!(app.cmds.back(), Some(AppCmd::Deselect)),
        "Ctrl+D is still Deselect, not the tool key"
    );
    mods(&mut app, false, false, false);
}

/// Every row the Sub Tool palette draws must actually TAKE when it is
/// picked — the state-level form of the Dot Pen finding (`7e9578f`),
/// where a row was selectable, previewed correctly, and left the tool
/// doing what the previous row did.
///
/// The registry is the source of the list, so a row added later is
/// covered the day it appears rather than the day someone remembers to
/// extend a hand-written array.
#[test]
fn every_sub_tool_row_takes_when_you_pick_it() {
    let Some(mut app) = headless() else { return };
    let mut checked = 0;
    for tool in [
        Tool::Fill,
        Tool::Tone,
        Tool::Wand,
        Tool::Select,
        Tool::Frame,
        Tool::Balloon,
        Tool::Text,
        Tool::Object,
        Tool::Figure,
        Tool::Gradient,
        Tool::Eyedrop,
        Tool::Pan,
    ] {
        for g in crate::subtools::groups_of(tool) {
            for &row in &g.subs {
                dispatch(&mut app, AppCmd::SetSubTool(row));
                assert!(
                    crate::subtools::is_current(&app, row),
                    "{tool:?} ▸ {} ▸ {}: picked, and the tool did not move to it",
                    g.name,
                    row.label()
                );
                assert!(
                    crate::subtools::is_lit(&app, row),
                    "{tool:?} ▸ {} ▸ {}: took, but the palette lights a different row",
                    g.name,
                    row.label()
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 30, "the registry went quiet: only {checked} rows");
}

/// Undo grouping, as a hand experiences it: one stroke is one press, and
/// nothing you do to the TOOLS costs a press. A tool switch, a sub tool
/// step and a brush-size step are settings, not history — if any of them
/// pushed a step, Ctrl+Z after a mistake would undo the settings first
/// and leave the mistake on the page.
#[test]
fn one_stroke_is_one_press_and_changing_tools_costs_none() {
    let Some(mut app) = headless() else { return };
    app.doc = mn_core::Document::new(256, 256);
    dispatch(&mut app, AppCmd::SetTool(Tool::Pen));
    dispatch(&mut app, AppCmd::SetSlotColor([0.0, 0.0, 0.0]));
    let ink = |app: &mut App| -> u64 {
        let (w, h) = app.doc.size;
        let App { renderer, doc, .. } = app;
        let img = super::pages::render_offscreen_drafts_off(renderer, doc, w, h);
        img.pixels().map(|p| 255 - p.0[0] as u64).sum()
    };
    assert_eq!(app.doc.undo_len(), 0);
    stroke(&mut app, (40.0, 40.0), (200.0, 200.0));
    assert_eq!(app.doc.undo_len(), 1, "one stroke, one press");
    let inked = ink(&mut app);
    assert!(inked > 0, "the stroke landed");

    press(&mut app, 0x45); // E — the eraser
    app.spring = None;
    press(&mut app, 0x45); // E again — back to the pen
    app.spring = None;
    press(&mut app, 0xBE); // . — step a sub tool
    press(&mut app, 0xDD); // ] — step the size
    pump(&mut app);
    assert_eq!(
        app.doc.undo_len(),
        1,
        "tool switches, a sub tool step and a size step are settings, not history"
    );

    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(ink(&mut app), 0, "one press took the whole stroke back");
}

/// TL-013 across the door a hand actually uses: a LOCKED sub tool takes
/// a nudge live, and coming back to it after a switch restores the
/// snapshot rather than the nudge. Driven through the `E` key both ways,
/// because that is how the calibrated pen is left and picked up again.
#[test]
fn a_locked_tool_property_comes_home_after_a_tool_key_round_trip() {
    let Some(mut app) = headless() else { return };
    dispatch(&mut app, AppCmd::SetBrushSizePx(18.0));
    app.props_current.locked = true;
    app.snapshot_current_props();
    dispatch(&mut app, AppCmd::SetBrushSizePx(60.0));
    assert_eq!(app.props_current.size_px, 60.0, "a locked tool still takes it");

    press(&mut app, 0x45); // E
    app.spring = None;
    press(&mut app, 0x45); // E again
    app.spring = None;
    assert_eq!(app.tool, Tool::Pen, "E cycles back to the pen");
    assert_eq!(
        app.props_current.size_px, 18.0,
        "the locked snapshot came home, not the one-panel nudge"
    );
    assert!(app.props_current.locked, "and it is still locked");
}

/// `keys.json`'s three target lengths, through the REAL key path: a
/// three-part path lands on that exact row every time, and a list on one
/// key walks its targets in written order.
#[test]
fn a_keys_json_tool_target_reaches_its_exact_row() {
    use crate::cmd::FigureMode;
    let Some(mut app) = headless() else { return };
    app.keymap = crate::keymap::Keymap::parse(
        r#"{
            "q": "tool: Figure / Direct draw / Ellipse",
            "n": ["tool: Fill", "tool: Auto select"]
        }"#,
    );
    assert!(app.keymap.problems.is_empty(), "{:?}", app.keymap.problems);

    // Leave Figure on another row first: an exact target must not defer
    // to the tool's memory.
    dispatch(&mut app, AppCmd::SetSubTool(crate::cmd::SubTool::Figure(
        FigureMode::Rect,
    )));
    press(&mut app, 0x51);
    assert_eq!(app.tool, Tool::Figure);
    assert_eq!(app.figure_mode, FigureMode::Ellipse, "the exact row, always");
    app.spring = None;

    press(&mut app, 0x4E);
    assert_eq!(app.tool, Tool::Fill, "first target");
    app.spring = None;
    press(&mut app, 0x4E);
    assert_eq!(app.tool, Tool::Wand, "press again walks the cycle on");
}
