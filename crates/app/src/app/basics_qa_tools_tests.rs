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

const VK: &[(&str, u16)] = &[
    ("P", 0x50),
    ("B", 0x42),
    ("E", 0x45),
    ("G", 0x47),
    ("M", 0x4D),
    ("W", 0x57),
    ("O", 0x4F),
    ("U", 0x55),
    ("T", 0x54),
    ("F", 0x46),
    ("V", 0x56),
    ("I", 0x49),
    ("H", 0x48),
    ("R", 0x52),
    ("J", 0x4A),
    ("K", 0x4B),
    ("Y", 0x59),
    ("Z", 0x5A),
    ("D", 0x44),
    ("L", 0x4C),
];

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

#[test]
#[ignore = "survey"]
fn survey_tool_keys() {
    let Some(mut app) = headless() else { return };
    for (name, vk) in VK {
        let before = app.tool;
        app.status.clear();
        press(&mut app, *vk);
        println!(
            "{name}: {:?} -> {:?} pan={:?} status={:?} brush={} spring={:?}",
            before,
            app.tool,
            app.pan_mode,
            app.status,
            app.brush_name(),
            app.spring.as_ref().map(|s| s.borrowed),
        );
        app.spring = None;
        // back to the pen for the next probe
        dispatch(&mut app, AppCmd::SetTool(Tool::Pen));
    }
}

#[test]
#[ignore = "survey"]
fn survey_cycles_and_stepping() {
    let Some(mut app) = headless() else { return };
    for (name, vk) in [("E", 0x45u16), ("T", 0x54), ("P", 0x50), ("O", 0x4F)] {
        dispatch(&mut app, AppCmd::SetTool(Tool::Pen));
        let mut seen = Vec::new();
        for _ in 0..4 {
            press(&mut app, vk);
            app.spring = None;
            seen.push(app.tool);
        }
        println!("{name} x4: {seen:?}");
    }
    // `,` / `.` stepping per tool.
    for tool in [
        Tool::Pen,
        Tool::Fill,
        Tool::Select,
        Tool::Figure,
        Tool::Frame,
        Tool::Balloon,
        Tool::Gradient,
        Tool::Object,
        Tool::Wand,
        Tool::Eyedrop,
        Tool::Tone,
        Tool::Liquify,
        Tool::Text,
        Tool::Eraser,
    ] {
        dispatch(&mut app, AppCmd::SetTool(tool));
        let rows = crate::subtools::step_rows(&app);
        let names: Vec<&str> = rows.iter().map(|s| s.label()).collect();
        let mut walk = Vec::new();
        for _ in 0..3 {
            app.status.clear();
            press(&mut app, 0xBE); // .
            walk.push(format!("{:?}/{}", app.tool, app.status));
        }
        println!("{:?}: rows={names:?}\n    walk={walk:?}", tool);
    }
}

#[test]
#[ignore = "survey"]
fn survey_modifiers() {
    let Some(mut app) = headless() else { return };
    app.doc = mn_core::Document::new(256, 256);
    let (x, y) = app.viewport.to_screen(128.0, 128.0);

    // Space = pan.
    app.space_down = true;
    app.canvas_down(x, y, PointerKind::Mouse, &[]);
    println!("space+drag: panning={} rotating={}", app.panning(), app.rotating());
    app.canvas_up(x, y, &[]);
    app.space_down = false;

    // CSP: Shift+Space+drag = rotate.
    mods(&mut app, false, true, false);
    app.space_down = true;
    app.canvas_down(x, y, PointerKind::Mouse, &[]);
    println!(
        "shift+space+drag: panning={} rotating={}",
        app.panning(),
        app.rotating()
    );
    app.canvas_up(x, y, &[]);
    app.space_down = false;
    mods(&mut app, false, false, false);

    // CSP: Ctrl+Space+click = zoom in, Alt+Space+click = zoom out.
    let z0 = app.viewport.zoom;
    mods(&mut app, true, false, false);
    app.space_down = true;
    app.canvas_down(x, y, PointerKind::Mouse, &[]);
    app.canvas_up(x, y, &[]);
    pump(&mut app);
    println!("ctrl+space+click: zoom {z0} -> {}", app.viewport.zoom);
    app.space_down = false;
    mods(&mut app, false, false, false);

    // Alt = eyedropper while a brush is in hand.
    dispatch(&mut app, AppCmd::SetTool(Tool::Pen));
    let before = app.doc.layers[0].tiles().count();
    mods(&mut app, false, false, true);
    app.canvas_down(x, y, PointerKind::Mouse, &[]);
    app.canvas_up(x, y, &[]);
    let picked = matches!(app.cmds.front(), Some(AppCmd::PickColor(..)));
    pump(&mut app);
    println!("alt+click on the pen: PickColor queued={picked} tiles {before}");
    mods(&mut app, false, false, false);

    // Shift = straight line while a brush is in hand (CSP "Draw straight line").
    dispatch(&mut app, AppCmd::SetTool(Tool::Pen));
    app.doc = mn_core::Document::new(256, 256);
    // The Alt probe above picked white off the paper — ink black again.
    dispatch(&mut app, AppCmd::SetSlotColor([0.0, 0.0, 0.0]));
    mods(&mut app, false, true, false);
    let (ax, ay) = app.viewport.to_screen(40.0, 40.0);
    app.canvas_down(ax, ay, PointerKind::Mouse, &[]);
    // A deliberately curved drag: straight-line mode must flatten it.
    for i in 1..=24 {
        let t = i as f32 / 24.0;
        let cx = 40.0 + t * 160.0;
        let cy = 40.0 + (t * std::f32::consts::PI).sin() * 60.0;
        let (mx, my) = app.viewport.to_screen(cx, cy);
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
    let (ux, uy) = app.viewport.to_screen(200.0, 40.0);
    app.canvas_up(ux, uy, &[]);
    pump(&mut app);
    // Ink well above the straight chord means the bow survived.
    let bowed = ink_in_band(&mut app, 90, 120);
    println!("(paper is opaque, so this is a luma sum)");
    println!("shift+drag with the pen: ink in the bowed band = {bowed}");
    mods(&mut app, false, false, false);
}

/// Ink on rows `y0..y1` of the EXPORTED page — a curved stroke leaves ink
/// there, a straight one between two points on y=40 does not.
fn ink_in_band(app: &mut App, y0: u32, y1: u32) -> u64 {
    let (w, h) = app.doc.size;
    let App { renderer, doc, .. } = app;
    let img = super::pages::render_offscreen_drafts_off(renderer, doc, w, h);
    let mut sum = 0u64;
    for y in y0..y1.min(h) {
        for x in 0..w {
            sum += 255 - img.get_pixel(x, y).0[0] as u64;
        }
    }
    sum
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

#[test]
#[ignore = "survey"]
fn survey_undo_grouping() {
    let Some(mut app) = headless() else { return };
    app.doc = mn_core::Document::new(256, 256);
    let n0 = app.doc.undo_len();
    stroke(&mut app, (40.0, 40.0), (200.0, 200.0));
    let n1 = app.doc.undo_len();
    println!("one stroke: {n0} -> {n1}");
    press(&mut app, 0x45); // E
    app.spring = None;
    press(&mut app, 0x50); // P
    app.spring = None;
    press(&mut app, 0xBE); // .
    press(&mut app, 0xDD); // ]
    pump(&mut app);
    println!(
        "two tool switches + a sub tool step + a size step: {n1} -> {}",
        app.doc.undo_len()
    );
    // A tool-property slider drag: many size commands, how many steps?
    for s in [20.0f32, 24.0, 28.0, 32.0] {
        dispatch(&mut app, AppCmd::SetBrushSizePx(s));
    }
    println!("size slider x4: {}", app.doc.undo_len());
    // Undo once: the stroke must be the thing that goes.
    let ink = |a: &mut App| -> u64 {
        let (w, h) = a.doc.size;
        let App { renderer, doc, .. } = a;
        let img = super::pages::render_offscreen_drafts_off(renderer, doc, w, h);
        img.pixels().map(|p| 255 - p.0[0] as u64).sum()
    };
    let before = ink(&mut app);
    dispatch(&mut app, AppCmd::Undo);
    println!("ink {before} -> {} after ONE undo", ink(&mut app));
}

#[test]
#[ignore = "survey"]
fn survey_subtool_changes_the_gesture() {
    let Some(mut app) = headless() else { return };
    use crate::cmd::{FigureMode, SelectMode, SubTool};
    let fitted = app.viewport;
    // Picking a Figure sub tool must change what a drag DRAWS.
    for m in [FigureMode::Line, FigureMode::Rect, FigureMode::Ellipse] {
        app.doc = mn_core::Document::new(256, 256);
        dispatch(&mut app, AppCmd::SetSubTool(SubTool::Figure(m)));
        assert_eq!(app.tool, Tool::Figure);
        stroke(&mut app, (60.0, 60.0), (190.0, 190.0));
        let (w, h) = app.doc.size;
        let App { renderer, doc, .. } = &mut app;
        let img = super::pages::render_offscreen_drafts_off(renderer, doc, w, h);
        let dark = img.pixels().filter(|p| p.0[0] < 128).count();
        // A pixel on the top-right corner of the bounding box: only the
        // rectangle inks there.
        let corner = img.get_pixel(190, 62).0[0];
        println!("{m:?}: dark={dark} corner={corner} mode={:?} undo={} drag={:?}", app.figure_mode, app.doc.undo_len(), app.figure_drag.is_some());
    }
    // The differential: the SAME drag at the identity viewport.
    for identity in [true, false] {
        app.doc = mn_core::Document::new(256, 256);
        if identity {
            app.viewport = mn_gpu::Viewport::default();
        } else {
            app.viewport = fitted;
        }
        dispatch(&mut app, AppCmd::SetSubTool(SubTool::Figure(FigureMode::Rect)));
        stroke(&mut app, (60.0, 60.0), (190.0, 190.0));
        let (w, h) = app.doc.size;
        let App { renderer, doc, .. } = &mut app;
        let img = super::pages::render_offscreen_drafts_off(renderer, doc, w, h);
        let dark = img.pixels().filter(|p| p.0[0] < 128).count();
        println!("identity={identity}: dark={dark} zoom={}", app.viewport.zoom);
    }

    // Selection shapes.
    for m in [SelectMode::Rect, SelectMode::Lasso] {
        app.doc = mn_core::Document::new(256, 256);
        dispatch(&mut app, AppCmd::SetSubTool(SubTool::Select(m)));
        println!("{m:?}: tool={:?} select_mode={:?}", app.tool, app.select_mode);
    }
}

#[test]
#[ignore = "survey"]
fn survey_locked_props_across_a_switch() {
    let Some(mut app) = headless() else { return };
    dispatch(&mut app, AppCmd::SetBrushSizePx(18.0));
    app.props_current.locked = true;
    app.snapshot_current_props();
    dispatch(&mut app, AppCmd::SetBrushSizePx(60.0));
    println!("locked, nudged: {}", app.props_current.size_px);
    press(&mut app, 0x45); // E — the eraser
    app.spring = None;
    press(&mut app, 0x45); // E again — back to the pen
    app.spring = None;
    println!(
        "after E,E: tool={:?} size={} locked={}",
        app.tool, app.props_current.size_px, app.props_current.locked
    );
}

#[test]
#[ignore = "survey"]
fn survey_a_keys_json_tool_target() {
    let Some(mut app) = headless() else { return };
    app.keymap = crate::keymap::Keymap::parse(
        r#"{ "q": "tool: Figure / Direct draw / Ellipse", "n": ["tool: Fill", "tool: Auto select"] }"#,
    );
    println!("problems: {:?}", app.keymap.problems);
    press(&mut app, 0x51);
    println!("q: tool={:?} figure_mode={:?}", app.tool, app.figure_mode);
    app.spring = None;
    press(&mut app, 0x4E);
    let a = app.tool;
    app.spring = None;
    press(&mut app, 0x4E);
    println!("n,n: {a:?} -> {:?}", app.tool);
}
