//! CV-021, "New Window" as a pane: the SECOND live view's viewport.
//!
//! The whole point of the feature is that the two views do not share a
//! viewport — ink at 400% on the Canvas pane, watch the whole page in the
//! second one — while both read the same live document. That is state
//! logic, so it is tested here rather than through egui layout (which does
//! not run headless): the pane body is thin glue over these four calls.

use super::*;
use crate::cmd::AppCmd;

/// Untouched, the second view FITS the page into whatever size the pane
/// happens to be — including after a resize, with no bookkeeping: the
/// viewport is computed from the pane size until the user steers it.
#[test]
fn an_untouched_second_view_fits_the_page_at_any_pane_size() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (800, 600), 1.0);
    assert!(
        app.view_pane_vp.is_none(),
        "a fresh second view follows the pane"
    );

    let inside = |app: &App, size: (u32, u32)| {
        let vp = app.view_pane_viewport(size);
        let (w, h) = (app.doc.size.0 as f32, app.doc.size.1 as f32);
        [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)]
            .iter()
            .all(|&(x, y)| {
                let (sx, sy) = vp.to_screen(x, y);
                (-0.5..=size.0 as f32 + 0.5).contains(&sx)
                    && (-0.5..=size.1 as f32 + 0.5).contains(&sy)
            })
    };
    for size in [(320, 480), (900, 300), (64, 64)] {
        assert!(
            inside(&app, size),
            "the whole page must be inside a {size:?} pane"
        );
    }
    // A wider pane is a bigger fit — the same page, more of the pane used.
    let small = app.view_pane_viewport((320, 480)).zoom;
    let big = app.view_pane_viewport((640, 960)).zoom;
    assert!(big > small, "the fit follows the pane: {small} -> {big}");

    // Fit is idempotent, and re-fitting after steering forgets the steer.
    app.view_pane_pan((320, 480), 40.0, -25.0);
    assert!(app.view_pane_vp.is_some(), "panning materializes the view");
    app.view_pane_fit();
    assert!(app.view_pane_vp.is_none());
    assert!(inside(&app, (320, 480)), "Fit puts the whole page back");
}

/// The second view's zoom keeps the page point under the anchor pinned
/// (the wheel anchors on the pointer), and its pan is a plain pixel shift.
#[test]
fn the_second_view_zooms_about_its_anchor_and_pans() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (800, 600), 1.0);
    let size = (400u32, 600u32);
    let anchor = [130.0f32, 210.0f32];

    let before = app.view_pane_viewport(size);
    let under = before.to_canvas(anchor[0], anchor[1]);
    app.view_pane_zoom(size, anchor, 2.0);
    let after = app.view_pane_viewport(size);
    assert!(
        (after.zoom - before.zoom * 2.0).abs() < 1e-3,
        "the factor landed: {} -> {}",
        before.zoom,
        after.zoom
    );
    let s = after.to_screen(under.0, under.1);
    assert!(
        (s.0 - anchor[0]).abs() < 0.5 && (s.1 - anchor[1]).abs() < 0.5,
        "the page point under the pointer must stay under it, got {s:?}"
    );

    // Pan is target pixels, straight through.
    let pan = app.view_pane_viewport(size).pan;
    app.view_pane_pan(size, 17.0, -9.0);
    let moved = app.view_pane_viewport(size).pan;
    assert_eq!(moved, [pan[0] + 17.0, pan[1] - 9.0]);

    // Zoom is clamped at both ends: below half the fit there is nothing
    // left to see, and a second view past 8x has stopped being an
    // overview. Neither clamp may leave the viewport degenerate.
    for factor in [1e-6, 1e6] {
        app.view_pane_fit();
        app.view_pane_zoom(size, anchor, factor);
        let z = app.view_pane_viewport(size).zoom;
        assert!(z.is_finite() && z > 0.0 && z <= 8.0, "clamped zoom {z}");
    }
}

/// The load-bearing claim: the two viewports are INDEPENDENT. Steering the
/// canvas (zoom, rotate, flip, and every CV-035 reset) must not move the
/// second view, and steering the second view must not move the canvas.
#[test]
fn the_two_views_never_touch_each_others_viewport() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (800, 600), 1.0);
    app.shell.set_canvas_rect_points(egui::Rect::from_min_size(
        egui::pos2(0.0, 0.0),
        egui::vec2(800.0, 600.0),
    ));
    crate::cmd::dispatch(&mut app, AppCmd::ZoomFit);
    let size = (400u32, 600u32);

    // The second view zoomed in on a detail; the canvas left fitted.
    app.view_pane_zoom(size, [200.0, 300.0], 4.0);
    let view_vp = app.view_pane_viewport(size);
    let canvas_before = app.viewport;

    // Now drive the CANVAS through every view command that exists.
    for cmd in [
        AppCmd::ZoomStep(2.0),
        AppCmd::RotateView(0.6),
        AppCmd::FlipView,
        AppCmd::FlipViewV,
        AppCmd::RotateReset,
        AppCmd::RotateFlipReset,
        AppCmd::ViewReset,
        AppCmd::Zoom100,
        AppCmd::ZoomFit,
    ] {
        crate::cmd::dispatch(&mut app, cmd);
        let now = app.view_pane_viewport(size);
        assert_eq!(
            (now.pan, now.zoom, now.rotate_rad, now.flip_h, now.flip_v),
            (
                view_vp.pan,
                view_vp.zoom,
                view_vp.rotate_rad,
                view_vp.flip_h,
                view_vp.flip_v
            ),
            "a canvas view command moved the SECOND view"
        );
    }

    // …and the other direction: the canvas is where ZoomFit left it, and
    // steering the second view does not disturb it.
    let canvas_now = app.viewport;
    assert_eq!(canvas_now.zoom, canvas_before.zoom);
    app.view_pane_zoom(size, [10.0, 10.0], 0.5);
    app.view_pane_pan(size, 60.0, 60.0);
    app.view_pane_fit();
    assert_eq!(
        (app.viewport.pan, app.viewport.zoom, app.viewport.rotate_rad),
        (canvas_now.pan, canvas_now.zoom, canvas_now.rotate_rad),
        "steering the second view moved the CANVAS"
    );
}

/// The texture cache is keyed on everything the render depends on — the
/// document revision, the target size, and the viewport — so the second
/// view is live (a document change re-mints) without re-rendering a page
/// composite on every idle frame.
#[test]
fn the_second_views_texture_re_mints_exactly_when_it_must() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (800, 600), 1.0);
    let size = (200u32, 300u32);

    let key_after = |app: &mut App, size: (u32, u32)| {
        let _ = app.view_pane_texture(size);
        app.view_pane_key.expect("a render happened")
    };
    let first = key_after(&mut app, size);
    assert_eq!(
        key_after(&mut app, size),
        first,
        "an idle frame must not re-composite the page"
    );

    // A document change (this is what "live" means).
    app.doc.touch();
    let after_edit = key_after(&mut app, size);
    assert_ne!(after_edit, first, "the view follows the document");

    // A resize, and a steer.
    let resized = key_after(&mut app, (260, 300));
    assert_ne!(resized, after_edit, "the view follows the pane size");
    app.view_pane_pan((260, 300), 12.0, 0.0);
    assert_ne!(
        key_after(&mut app, (260, 300)),
        resized,
        "the view follows its own viewport"
    );
}
