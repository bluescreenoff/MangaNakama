use super::*;

/// TODO #7 part 2: the hairy/curve/dyna `mn-engine` keys select their
/// engines end to end, and each paints a stroke.
#[test]
fn mn_engine_keys_select_the_krita_engines() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/brushes/krita");
    for (file, expected) in [
        ("hairy-bristles.myb", "hairy"),
        ("curve-brush.myb", "curve"),
        ("dyna-spring.myb", "dyna"),
    ] {
        crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::SelectBrush(root.join(file)));
        let selected = match app.engine().kind() {
            EngineKind::Hairy(_) => "hairy",
            EngineKind::Curve(_) => "curve",
            EngineKind::Dyna(_) => "dyna",
            _ => "other",
        };
        assert_eq!(selected, expected, "{file} selected {selected}");

        // A short stroke paints.
        let empty: [PenSample; 0] = [];
        let (x0, y0) = app.viewport.to_screen(120.0, 1024.0);
        let (x1, y1) = app.viewport.to_screen(400.0, 1024.0);
        app.canvas_down(x0, y0, PointerKind::Mouse, &empty);
        let batch: Vec<PenSample> = (0..30)
            .map(|i| {
                let (sx, sy) = app.viewport.to_screen(120.0 + i as f32 * 10.0, 1024.0);
                PenSample {
                    x: sx,
                    y: sy,
                    pressure: 1.0,
                    tilt_x: 0.0,
                    tilt_y: 0.0,
                    t_ms: i as f64 * 8.0,
                }
            })
            .collect();
        app.push_batch(&batch);
        app.canvas_up(x1, y1, &empty);
        let alpha: u64 = app
            .doc
            .active_layer()
            .tiles()
            .map(|(_, t)| t.alpha_sum())
            .sum();
        assert!(alpha > 0, "{file} painted a stroke");
    }
}

/// TRIAGE 172 (owner HIGH): KB-020 — Ctrl+Alt+drag resizes the brush
/// live (the multiplier the Tool Property slider shares); KB-022 —
/// Ctrl+drag grabs the object under the pen WITHOUT changing tools,
/// and a release keeps drawing. Nothing under the pen declines.
#[test]
fn size_drag_grows_brush_and_temp_grab_moves_without_tool_change() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    fn pump(app: &mut App) {
        while let Some(c) = app.cmds.pop_front() {
            crate::cmd::dispatch(app, c);
        }
    }
    app.tool = crate::cmd::Tool::Pen;

    // KB-020: 240 px of rightward drag doubles the px diameter.
    let m0 = app.props_current.size_px;
    app.size_drag_begin(100.0);
    assert!(app.size_drag.is_some(), "the drag is armed");
    app.canvas_move(340.0, 200.0, &[]);
    pump(&mut app);
    let m1 = app.props_current.size_px;
    assert!(m1 > m0, "the size grew ({m0} px -> {m1} px)");
    app.canvas_up(340.0, 200.0, &[]);
    assert!(app.size_drag.is_none(), "release ends the drag");
    pump(&mut app);

    // KB-022: a balloon under the pen moves; the tool stays Pen.
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::BalloonAdd {
            balloon: mn_core::Balloon {
                // BIG, grabbed at its centre: at the default
                // fit-zoom the Object tool's handle tolerance is ~73
                // canvas px and a small balloon's handle ring covers
                // its whole interior — this grab must be MoveWhole.
                shape: mn_core::BalloonShape::Ellipse {
                    center: [1024.0, 1024.0],
                    radii: [250.0, 200.0],
                },
                tails: Vec::new(),
                ..Default::default()
            },
        },
    );
    let (li, bi) = app.balloon_sel.expect("the fresh balloon is selected");
    let before = match &app.doc.layers[li].balloons().unwrap().balloons[bi].shape {
        mn_core::BalloonShape::Ellipse { center, .. } => *center,
        _ => [0.0; 2],
    };
    let (sx, sy) = app.viewport.to_screen(1024.0, 1024.0);
    assert!(app.temp_object_try(sx, sy), "the grab consumed the press");
    assert!(app.temp_object && app.balloon_obj_drag.is_some());
    assert_eq!(app.tool, crate::cmd::Tool::Pen, "the tool never changes");
    let (mx, my) = app.viewport.to_screen(1084.0, 1074.0);
    app.canvas_move(mx, my, &[]);
    app.canvas_up(mx, my, &[]);
    pump(&mut app);
    let after = match &app.doc.layers[li].balloons().unwrap().balloons[bi].shape {
        mn_core::BalloonShape::Ellipse { center, .. } => *center,
        _ => [0.0; 2],
    };
    assert!(
        (after[0] - before[0] - 60.0).abs() < 1.0 && (after[1] - before[1] - 50.0).abs() < 1.0,
        "the balloon moved by the drag ({before:?} -> {after:?})"
    );
    assert!(!app.temp_object, "release clears the temp flag");

    // Nothing under the pen: the grab declines and the pen would draw.
    let (ex, ey) = app.viewport.to_screen(20.0, 20.0);
    assert!(!app.temp_object_try(ex, ey), "empty space declines");
    assert!(!app.temp_object);
}

/// KB-022 second pass: the pre-check gate used zero-tolerance
/// containment while the hit test it gated accepts a border within
/// ~10 screen px — a Ctrl+drag starting on the GUTTER side of a frame
/// border drew ink over it instead of grabbing. And with the gate
/// gone, a true miss must still keep the standing selection (keeping
/// it was the only thing the gate did).
#[test]
fn temp_grab_reaches_border_from_gutter_side_and_miss_keeps_selection() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    app.tool = crate::cmd::Tool::Pen;
    app.doc.add_frame_folder(
        "Frame 1",
        mn_core::FrameSet::single_rect([300.0, 300.0, 800.0, 800.0], 4.0),
    );
    let li = (0..app.doc.layers.len())
        .find(|&i| app.doc.layers[i].frames().is_some())
        .expect("the frame folder exists");

    // 30 canvas px OUTSIDE the left border, mid-height: inside the hit
    // test's tolerance band (~10 screen px / zoom), outside the panel.
    let (sx, sy) = app.viewport.to_screen(270.0, 550.0);
    assert!(
        app.temp_object_try(sx, sy),
        "the gutter-side press grabs the border, not the pen"
    );
    assert!(
        matches!(
            app.object_drag.as_ref().map(|d| d.mode),
            Some(crate::app::canvas_input::ObjectDragMode::Edge(_))
        ),
        "the grab armed an edge drag"
    );
    app.canvas_up(sx, sy, &[]);
    assert!(!app.temp_object, "release clears the temp flag");

    // A miss far from everything keeps the standing selection.
    app.object_sel = Some((li, 0));
    let (ex, ey) = app.viewport.to_screen(20.0, 20.0);
    assert!(!app.temp_object_try(ex, ey), "empty space declines");
    assert_eq!(
        app.object_sel,
        Some((li, 0)),
        "the declined press left the selection standing"
    );
}

/// r113: the eye solo the r102 hover promised — Alt+click hides every
/// other layer, the second press restores the snapshot, a page switch
/// drops it, and the manual's door resolves a real file.
#[test]
fn eye_solo_hides_restores_and_clears_on_switch() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    app.doc.add_layer("L2");
    app.doc.add_layer("L3");
    app.doc.set_layer_visible(1, false); // [true, false, true]

    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::SetLayerEyeSolo(1));
    assert!(app.doc.only_visible(1), "the solo hides every other layer");
    assert!(app.eye_solo_backup.is_some(), "the snapshot is kept");

    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::SetLayerEyeSolo(1));
    assert!(
        app.doc.layers[0].visible && !app.doc.layers[1].visible && app.doc.layers[2].visible,
        "the second press restores the exact snapshot"
    );
    assert!(app.eye_solo_backup.is_none());

    // Solo, then leave the page: the snapshot belongs to the page.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::SetLayerEyeSolo(2));
    assert!(app.doc.only_visible(2));
    let (sw, sh) = (app.doc.size.0, app.doc.size.1);
    let d = mn_core::Document::new(sw, sh);
    let b = mn_core::project::doc_to_bytes(&d).unwrap();
    let e = app.fresh_page(Some(b), None);
    app.pages.push(e);
    app.switch_page(1);
    assert!(
        app.eye_solo_backup.is_none(),
        "page switch drops the snapshot"
    );

    // The manual's door resolves a real file on this machine (dev
    // fallback; the shipped layout is exe-relative).
    assert!(
        crate::cmd::manual_path().is_some(),
        "manual_path finds docs/manual"
    );
}

/// Owner item 2026-08-19 (top of the text arc): pressing the
/// Object-tool key AGAIN cycles the stacked objects under the pick
/// (text → balloon → frame, wraparound, Shift back), and switching
/// Text→Object hands over the BALLOON — CSP's behaviour, the part he
/// likes.
#[test]
fn object_key_cycles_the_stack_and_text_handover_gives_the_balloon() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    // The stack, bottom-up: a frame layer, a balloon, a text layer —
    // all containing (300, 200).
    app.doc.add_frame_layer(
        "Frame 1",
        mn_core::FrameSet {
            frames: vec![mn_core::Frame::rect(50.0, 50.0, 550.0, 350.0)],
            border_px: 3.0,
            slot: None,
            reading_pin: None,
            border_ruler: false,
        },
    );
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::BalloonAdd {
            balloon: mn_core::Balloon {
                shape: mn_core::BalloonShape::Ellipse {
                    center: [300.0, 200.0],
                    radii: [140.0, 90.0],
                },
                tails: Vec::new(),
                ..Default::default()
            },
        },
    );
    let mut item =
        mn_core::TextItem::new([220.0, 170.0], "Arial".to_owned(), 24.0, [0, 0, 0], false);
    item.size = [160.0, 60.0];
    let ts = mn_core::TextSet { texts: vec![item] };
    let tli = app.doc.add_text_layer("Text", ts);

    // Click the stack in the Object tool: the text claims the press
    // (topmost, texts first).
    app.tool = crate::cmd::Tool::Object;
    let empty: [PenSample; 0] = [];
    let (x, y) = app.viewport.to_screen(300.0, 200.0);
    app.canvas_down(x, y, PointerKind::Mouse, &empty);
    app.canvas_up(x, y, &empty);
    assert_eq!(app.text_sel, Some((tli, 0)), "the text claims the click");
    assert!(app.object_pick.is_some(), "the pick point was stored");

    // Cycle forward: text → balloon → frame → wrap to text.
    app.object_cycle(true);
    assert!(
        app.text_sel.is_none() && app.balloon_sel.is_some(),
        "→ balloon"
    );
    app.object_cycle(true);
    assert!(
        app.balloon_sel.is_none() && app.object_sel.is_some(),
        "→ frame"
    );
    app.object_cycle(true);
    assert_eq!(app.text_sel, Some((tli, 0)), "wraps to the text");
    // Shift (backward) goes back to the frame.
    app.object_cycle(false);
    assert!(
        app.object_sel.is_some() && app.text_sel.is_none(),
        "back to the frame"
    );
    // Selection only: no document mutation, no undo step.
    assert_eq!(app.doc.undo_len(), 0, "cycling is not an undo step");

    // Text→Object handover: with the text selected from the TEXT tool,
    // switching to Object selects the balloon under it.
    app.object_sel = None;
    app.balloon_sel = None;
    app.text_sel = Some((tli, 0));
    app.tool = crate::cmd::Tool::Text;
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::SetTool(crate::cmd::Tool::Object),
    );
    assert!(
        app.balloon_sel.is_some() && app.text_sel.is_none(),
        "the switch hands over the balloon (CSP behaviour)"
    );
}

/// TODO #7: a preset carrying `"mn-engine": "grid"` selects the Grid
/// engine — the mn-engine identity mechanism, end to end.
#[test]
fn mn_engine_grid_key_selects_the_grid_engine() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/brushes/krita/grid-dots.myb");
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::SelectBrush(path.clone()));
    assert!(
        matches!(app.engine().kind(), EngineKind::Grid(_)),
        "the mn-engine key selected the Grid engine"
    );

    // A stroke inks dotted lattice dots.
    let empty: [PenSample; 0] = [];
    let (x0, y0) = app.viewport.to_screen(120.0, 1024.0);
    let (x1, y1) = app.viewport.to_screen(400.0, 1024.0);
    app.canvas_down(x0, y0, PointerKind::Mouse, &empty);
    let batch: Vec<PenSample> = (0..30)
        .map(|i| {
            let (sx, sy) = app.viewport.to_screen(120.0 + i as f32 * 10.0, 1024.0);
            PenSample {
                x: sx,
                y: sy,
                pressure: 1.0,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: i as f64 * 8.0,
            }
        })
        .collect();
    app.push_batch(&batch);
    app.canvas_up(x1, y1, &empty);
    let alpha: u64 = app
        .doc
        .active_layer()
        .tiles()
        .map(|(_, t)| t.alpha_sum())
        .sum();
    assert!(alpha > 0, "the grid engine inked");
    let _ = (x1, y1);
}
