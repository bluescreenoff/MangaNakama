use super::*;
use crate::cmd::AppCmd;

/// CV-035: the three view resets are distinct, and each leaves exactly
/// what its name does not mention alone. Rotation-only keeps the
/// mirror; rotation+flip keeps the zoom; the full reset also fits.
///
/// The ORDER inside the composites is the part worth a test: unflipping
/// also mirrors the rotation (`Viewport::flip_around`), so a reset that
/// zeroed the angle first would come out unmirrored but crooked.
#[test]
fn the_three_view_resets_stay_distinct() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (800, 600), 1.0);
    // A real canvas rect: without one, `fit_to_view` has no area to fit
    // into (a headless renderer reports a 0x0 surface) and does nothing.
    app.shell.set_canvas_rect_points(egui::Rect::from_min_size(
        egui::pos2(0.0, 0.0),
        egui::vec2(800.0, 600.0),
    ));
    // The fit is taken from the app itself rather than recomputed here,
    // so this test does not care what shape the fit function has.
    crate::cmd::dispatch(&mut app, AppCmd::ZoomFit);
    let fitted = app.viewport;
    let twist = |app: &mut App| {
        app.viewport = fitted;
        let c = app.canvas_center();
        app.viewport.rotate_around(c, 0.6);
        app.viewport.flip_around(c);
        app.viewport.zoom_around(c, 3.0);
        assert!(app.viewport.flip_h && app.viewport.rotate_rad != 0.0);
    };

    // 1. Rotation only — the mirror is untouched, because the
    //    drawing-error check must survive straightening the page.
    twist(&mut app);
    crate::cmd::dispatch(&mut app, AppCmd::RotateReset);
    assert_eq!(app.viewport.rotate_rad, 0.0);
    assert!(app.viewport.flip_h, "RotateReset must not unmirror");

    // 2. Rotation + flip — the zoom survives, so it can be used
    //    mid-drawing without losing the magnification being inked at.
    twist(&mut app);
    let zoom = app.viewport.zoom;
    crate::cmd::dispatch(&mut app, AppCmd::RotateFlipReset);
    assert_eq!(app.viewport.rotate_rad, 0.0, "upright");
    assert!(!app.viewport.flip_h, "and unmirrored");
    assert_eq!(app.viewport.zoom, zoom, "the zoom is not its business");

    // 3. The lot — upright, unmirrored AND refitted.
    twist(&mut app);
    let zoomed_in = app.viewport.zoom;
    crate::cmd::dispatch(&mut app, AppCmd::ViewReset);
    assert_eq!(app.viewport.rotate_rad, 0.0);
    assert!(!app.viewport.flip_h);
    assert_ne!(app.viewport.zoom, zoomed_in, "the fit ran");
    assert_eq!(app.viewport.zoom, fitted.zoom, "and landed on the fit");
    assert_eq!(app.viewport.pan, fitted.pan);
}

/// TL-013, CSP's meaning: a locked sub tool ACCEPTS every change and
/// simply never writes it down, so leaving and returning restores the
/// snapshot. Asserted through the real store/load pair, because the
/// whole mechanism is that `store_current_props` declines to write.
#[test]
fn a_locked_sub_tool_comes_back_to_its_snapshot() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (400, 300), 1.0);
    let Some(i) = app.selected_preset else {
        println!("[test] SKIP: no brush presets on disk");
        return;
    };
    let pen = app.presets[i].1.clone();

    // Calibrate, then lock: THIS is the state a return must restore.
    crate::cmd::dispatch(&mut app, AppCmd::SetBrushSize(2.0));
    crate::cmd::dispatch(&mut app, AppCmd::SetOpacity(0.4));
    crate::cmd::dispatch(&mut app, AppCmd::SetToolLock(true));
    assert!(app.props_current.locked);

    // A locked tool is not a read-only tool — the nudge lands.
    crate::cmd::dispatch(&mut app, AppCmd::SetBrushSize(3.5));
    assert_eq!(
        app.props_current.size, 3.5,
        "a locked tool still takes the change; refusing it would make \
             the lock something you keep switching off"
    );

    // Leave and come back: the snapshot, not the nudge.
    app.store_current_props();
    app.load_props_for(&pen);
    assert_eq!(app.props_current.size, 2.0, "the calibrated size is back");
    assert_eq!(app.props_current.opacity, 0.4, "and the opacity with it");
    assert!(app.props_current.locked, "still locked");

    // Released, the live values become the sub tool's own — the drift
    // is adopted rather than thrown away by the next switch.
    crate::cmd::dispatch(&mut app, AppCmd::SetBrushSize(1.25));
    crate::cmd::dispatch(&mut app, AppCmd::SetToolLock(false));
    app.store_current_props();
    app.load_props_for(&pen);
    assert_eq!(app.props_current.size, 1.25, "released, the nudge sticks");
    assert!(!app.props_current.locked);

    // "Reset to preset" drops the lock with the values it froze —
    // a padlock over settings that no longer exist is a lie.
    crate::cmd::dispatch(&mut app, AppCmd::SetToolLock(true));
    app.forget_current_props();
    assert!(!app.props_current.locked, "reset releases the lock");
}
