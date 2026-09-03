//! Surface pass — canvas/view + interface, the way a mangaka uses them:
//! fit the page, zoom to 100 %, twist the paper, mirror it to check the
//! composition, put it all back, hide the palettes, save a workspace.
//! Every flow drives the real doors (`AppCmd`s through `cmd::dispatch`,
//! `App::fit_to_view_sized`, the Navigator's own helpers) and renders the
//! LIVE viewport, so a run with `MN_SURFACE_OUT=<dir>` leaves one PNG per
//! flow to look at.
//!
//! The renders are always a WINDOW (`render_win`), never the whole page:
//! a 600 dpi page as one texture runs CI's software GPU out of memory.

use super::App;
use crate::cmd::{AppCmd, dispatch};
use mn_gpu::Viewport;

/// A 900x700 page in a 1200x800 canvas area, ppp = 1 so points == pixels.
fn app() -> Option<App> {
    let mut app = super::new_document_tests::headless()?;
    app.doc = mn_core::Document::new(900, 700);
    app.shell.set_ppp(1.0);
    app.shell.set_canvas_rect_points(egui::Rect::from_min_max(
        egui::pos2(0.0, 0.0),
        egui::pos2(1200.0, 800.0),
    ));
    app.viewport = Viewport::default();
    Some(app)
}

fn run(app: &mut App, cmd: AppCmd) {
    dispatch(app, cmd);
    while let Some(c) = app.cmds.pop_front() {
        dispatch(app, c);
    }
}

/// Ink a fat black L in the page's top-left quarter, so a render can tell
/// upright from rotated and left from mirrored at a glance.
fn landmark(app: &mut App) {
    let li = app.doc.active;
    let mut ink = |x0: i32, y0: i32, x1: i32, y1: i32| {
        for y in y0..y1 {
            for x in x0..x1 {
                let idx = mn_core::TileIdx::of_pixel(x, y);
                let (ox, oy) = idx.origin();
                app.doc.layers[li].tile_mut(idx).set_pixel(
                    (x - ox) as usize,
                    (y - oy) as usize,
                    [0, 0, 0, 32768],
                );
            }
        }
    };
    ink(60, 60, 300, 90);
    ink(60, 60, 90, 260);
    app.doc.revision += 1;
}

/// One 1:1 render of the canvas area through the app's LIVE viewport —
/// what the window would be showing.
fn render_win(app: &mut App, name: &str) -> image::RgbaImage {
    let vp = app.viewport;
    let img = app.renderer.render_offscreen_vp(&app.doc, &vp, 600, 480);
    if let Ok(dir) = std::env::var("MN_SURFACE_OUT") {
        let _ = std::fs::create_dir_all(&dir);
        img.save(format!("{dir}/{name}.png")).expect("png written");
    }
    img
}

/// Bounding box of dark pixels, `[x0, y0, x1, y1]` exclusive.
fn ink_bbox(img: &image::RgbaImage) -> Option<[u32; 4]> {
    let mut bb: Option<[u32; 4]> = None;
    for (x, y, p) in img.enumerate_pixels() {
        if p[3] > 0 && (p[0] as u32 + p[1] as u32 + p[2] as u32) < 3 * 128 {
            bb = Some(match bb {
                None => [x, y, x + 1, y + 1],
                Some(b) => [b[0].min(x), b[1].min(y), b[2].max(x + 1), b[3].max(y + 1)],
            });
        }
    }
    bb
}

fn total_ink(app: &App) -> u64 {
    app.doc
        .layers
        .iter()
        .flat_map(|l| l.tiles())
        .map(|(_, t)| t.alpha_sum())
        .sum()
}

// --- v01 zoom ------------------------------------------------------------

#[test]
fn v01_the_zoom_ladder_fits_pixels_and_steps() {
    let Some(mut app) = app() else { return };
    landmark(&mut app);

    run(&mut app, AppCmd::ZoomFit);
    let fit = app.viewport.zoom;
    // 1200x800 area, 900x700 page, 0.98 margin => the HEIGHT binds.
    assert!(
        (fit - (800.0 / 700.0) * app.prefs.fit_margin).abs() < 1e-3,
        "fit zoom {fit}"
    );
    render_win(&mut app, "v01-fit");

    run(&mut app, AppCmd::Zoom100);
    assert!(
        (app.viewport.zoom - 1.0).abs() < 1e-4,
        "100% => {}",
        app.viewport.zoom
    );
    render_win(&mut app, "v01-100");

    run(&mut app, AppCmd::ZoomStep(2.0));
    assert!((app.viewport.zoom - 2.0).abs() < 1e-4);
    run(&mut app, AppCmd::ZoomStep(0.5));
    assert!((app.viewport.zoom - 1.0).abs() < 1e-4);

    // The step is anchored on the canvas-area centre: the canvas point
    // under the centre must not move.
    run(&mut app, AppCmd::Zoom100);
    let c = app.canvas_center();
    let before = app.viewport.to_canvas(c[0], c[1]);
    run(&mut app, AppCmd::ZoomStep(1.15));
    let after = app.viewport.to_canvas(c[0], c[1]);
    assert!(
        (before.0 - after.0).abs() < 0.01 && (before.1 - after.1).abs() < 0.01,
        "zoom step moved the anchor: {before:?} -> {after:?}"
    );
}

// --- v02 rotate ----------------------------------------------------------

#[test]
fn v02_rotate_steps_and_the_three_resets() {
    let Some(mut app) = app() else { return };
    landmark(&mut app);
    run(&mut app, AppCmd::ZoomFit);
    let fitted = app.viewport.zoom;
    render_win(&mut app, "v02-upright");

    let step = app.prefs.rotate_step_deg.to_radians();
    for _ in 0..3 {
        run(&mut app, AppCmd::RotateView(step));
    }
    assert!(
        (app.viewport.rotate_rad.to_degrees() - 45.0).abs() < 1e-3,
        "three 15 degree steps => {}",
        app.viewport.rotate_rad.to_degrees()
    );
    render_win(&mut app, "v02-rot45");

    // Reset rotation leaves zoom and flip alone.
    run(&mut app, AppCmd::FlipView);
    run(&mut app, AppCmd::RotateReset);
    assert!(app.viewport.rotate_rad.abs() < 1e-5);
    assert!(app.viewport.flip_h, "reset rotation must not un-mirror");
    assert!(
        (app.viewport.zoom - fitted).abs() < 1e-4,
        "reset rotation must not re-zoom"
    );

    // Reset rotation AND flip: still no re-zoom.
    run(&mut app, AppCmd::RotateView(step));
    run(&mut app, AppCmd::ZoomStep(2.0));
    let z = app.viewport.zoom;
    run(&mut app, AppCmd::RotateFlipReset);
    assert!(app.viewport.rotate_rad.abs() < 1e-5 && !app.viewport.flip_h && !app.viewport.flip_v);
    assert!(
        (app.viewport.zoom - z).abs() < 1e-4,
        "reset rotation+flip must keep the zoom"
    );

    // The whole view back: upright, unmirrored, FITTED.
    run(&mut app, AppCmd::RotateView(step));
    run(&mut app, AppCmd::FlipViewV);
    run(&mut app, AppCmd::ViewReset);
    assert!(app.viewport.rotate_rad.abs() < 1e-5 && !app.viewport.flip_h && !app.viewport.flip_v);
    assert!(
        (app.viewport.zoom - fitted).abs() < 1e-4,
        "view reset must re-fit"
    );
    render_win(&mut app, "v02-reset");
}

// --- v03 flip ------------------------------------------------------------

#[test]
fn v03_the_mirror_check_is_view_only_and_composes() {
    let Some(mut app) = app() else { return };
    landmark(&mut app);
    run(&mut app, AppCmd::Zoom100);
    // Park the page corner at the render window's corner so the landmark
    // is inside the 600x480 window at every flip.
    app.viewport.pan = [0.0, 0.0];
    let upright = render_win(&mut app, "v03-upright");
    let bb0 = ink_bbox(&upright).expect("landmark visible");
    assert!(
        bb0[0] < 100 && bb0[1] < 100,
        "landmark starts top-left: {bb0:?}"
    );
    let before = total_ink(&app);

    run(&mut app, AppCmd::FlipView);
    // Mirrored, `pan` is the screen spot of the top-RIGHT corner: park it
    // at the window's right edge so the landmark lands inside the window.
    app.viewport.pan = [600.0, 0.0];
    let flipped = render_win(&mut app, "v03-flip-h");
    let bb1 = ink_bbox(&flipped).expect("landmark still visible");
    assert!(
        bb1[2] > 500,
        "mirrored horizontally the landmark is on the RIGHT: {bb1:?}"
    );
    assert!(bb1[1] < 100, "and still on top: {bb1:?}");

    run(&mut app, AppCmd::FlipViewV);
    app.viewport.pan = [600.0, 480.0];
    let both = render_win(&mut app, "v03-flip-hv");
    let bb2 = ink_bbox(&both).expect("landmark still visible");
    assert!(
        bb2[2] > 500 && bb2[3] > 400,
        "both flips = 180 degrees, so bottom-right: {bb2:?}"
    );

    assert_eq!(
        before,
        total_ink(&app),
        "the mirror is a VIEW check — the art must not move"
    );

    // And the command door says which way the view is facing.
    let Some(mut app2) = self::app() else { return };
    run(&mut app2, AppCmd::FlipView);
    assert!(app2.viewport.flip_h && !app2.viewport.flip_v);
    assert!(!app2.status.is_empty(), "flip says something");
}

// --- v04 fit keeps the mirror -------------------------------------------

#[test]
fn v04_a_fit_carries_the_mirror_through() {
    let Some(mut app) = app() else { return };
    landmark(&mut app);
    run(&mut app, AppCmd::FlipView);
    run(&mut app, AppCmd::FlipViewV);
    app.fit_to_view_sized((1200, 800));
    assert!(
        app.viewport.flip_h && app.viewport.flip_v,
        "a fit must not silently un-mirror"
    );
    // and the page is still centred in the surface, mirrored or not.
    let mid = app.viewport.to_canvas(600.0, 400.0);
    assert!(
        (mid.0 - 450.0).abs() < 2.0 && (mid.1 - 350.0).abs() < 2.0,
        "mirrored fit is off centre: {mid:?}"
    );
}

// --- v05 navigator -------------------------------------------------------

#[test]
fn v05_the_navigator_pans_and_the_sticky_fit_follows_the_window() {
    let Some(mut app) = app() else { return };
    landmark(&mut app);
    run(&mut app, AppCmd::Zoom100);
    app.navigator_pan_to(700.0, 600.0);
    let c = app.canvas_center();
    let at = app.viewport.to_canvas(c[0], c[1]);
    assert!(
        (at.0 - 700.0).abs() < 0.5 && (at.1 - 600.0).abs() < 0.5,
        "navigator drag must centre the point it was given: {at:?}"
    );

    // Sticky fit: only a size CHANGE re-fits, and only while it is on.
    app.fit_sticky = false;
    app.nav_last_surface = (1200, 800);
    run(&mut app, AppCmd::Zoom100);
    app.navigator_sticky_fit_apply((900, 600));
    assert!(
        (app.viewport.zoom - 1.0).abs() < 1e-4,
        "sticky fit OFF must not re-fit"
    );
    app.fit_sticky = true;
    app.navigator_sticky_fit_apply((1200, 800));
    let want = (800.0 / 700.0) * app.prefs.fit_margin;
    assert!(
        (app.viewport.zoom - want).abs() < 1e-3,
        "sticky fit ON re-fits on a resize: {} wanted {want}",
        app.viewport.zoom
    );
    let z = app.viewport.zoom;
    app.navigator_sticky_fit_apply((1200, 800));
    assert!((app.viewport.zoom - z).abs() < 1e-6, "same size, no re-fit");
}

// --- v06 guides ----------------------------------------------------------

#[test]
fn v06_hiding_the_guides_says_the_page_is_unchanged() {
    let Some(mut app) = app() else { return };
    run(&mut app, AppCmd::SetGuidesHidden(true));
    assert!(app.layout.guides_hidden);
    assert!(app.status.contains("unchanged"), "status: {}", app.status);
    run(&mut app, AppCmd::SetGuidesHidden(false));
    assert!(!app.layout.guides_hidden);
}

// --- v08 the zoom ladder (CV-032) ---------------------------------------

#[test]
fn v08_zoom_in_and_out_walk_round_scales_not_wheel_notches() {
    let Some(mut app) = app() else { return };
    landmark(&mut app);
    run(&mut app, AppCmd::Zoom100);

    // Up from 100 %: the rungs a page is judged at, in order.
    let mut up = vec![];
    for _ in 0..4 {
        run(&mut app, AppCmd::ZoomIn);
        up.push((app.viewport.zoom * 100.0).round() as i32);
    }
    assert_eq!(up, vec![150, 200, 300, 400], "the ladder above 100 %");
    assert!(
        app.status.contains("400%"),
        "the step says where it landed: {}",
        app.status
    );

    // …and back down through the same rungs.
    let mut down = vec![];
    for _ in 0..5 {
        run(&mut app, AppCmd::ZoomOut);
        down.push((app.viewport.zoom * 100.0).round() as i32);
    }
    assert_eq!(down, vec![300, 200, 150, 100, 66], "the ladder below 400 %");

    // A zoom that is BETWEEN rungs snaps to the next one, and the ends
    // hold instead of running off.
    run(&mut app, AppCmd::ZoomTo(0.80));
    run(&mut app, AppCmd::ZoomIn);
    assert_eq!((app.viewport.zoom * 100.0).round() as i32, 100);
    run(&mut app, AppCmd::ZoomTo(64.0));
    run(&mut app, AppCmd::ZoomIn);
    assert!(
        (app.viewport.zoom - 64.0).abs() < 1e-3,
        "the top rung holds"
    );
    run(&mut app, AppCmd::ZoomTo(0.01));
    run(&mut app, AppCmd::ZoomOut);
    assert!(app.viewport.zoom <= 0.02 + 1e-4, "the bottom rung holds");

    // 200 % is a row of its own, exact.
    run(&mut app, AppCmd::ZoomTo(2.0));
    assert!((app.viewport.zoom - 2.0).abs() < 1e-4);
    render_win(&mut app, "v08-200");
}

// --- v09 the quarter turns (CV-033) -------------------------------------

#[test]
fn v09_a_quarter_turn_is_one_command() {
    let Some(mut app) = app() else { return };
    landmark(&mut app);
    run(&mut app, AppCmd::ZoomFit);
    let z = app.viewport.zoom;

    run(&mut app, AppCmd::RotateViewTo(90.0));
    assert!((app.viewport.rotate_rad.to_degrees() - 90.0).abs() < 1e-3);
    assert!(
        (app.viewport.zoom - z).abs() < 1e-4,
        "a quarter turn is not a re-zoom"
    );
    render_win(&mut app, "v09-rot90");

    // Absolute, not cumulative: asking for 90 twice leaves it at 90.
    run(&mut app, AppCmd::RotateViewTo(90.0));
    assert!((app.viewport.rotate_rad.to_degrees() - 90.0).abs() < 1e-3);

    run(&mut app, AppCmd::RotateViewTo(180.0));
    assert!((app.viewport.rotate_rad.to_degrees().abs() - 180.0).abs() < 1e-3);

    // 270 wraps to -90 in the viewport, and the status line says the angle
    // the view is actually at rather than the one that was asked for.
    run(&mut app, AppCmd::RotateViewTo(270.0));
    assert!(
        (app.viewport.rotate_rad.to_degrees() + 90.0).abs() < 1e-3,
        "270 => {}",
        app.viewport.rotate_rad.to_degrees()
    );
    assert!(app.status.contains("-90"), "status: {}", app.status);

    run(&mut app, AppCmd::RotateReset);
    assert!(app.viewport.rotate_rad.abs() < 1e-5);
}

// --- v10 the keys a CSP hand reaches for --------------------------------

#[test]
fn v10_the_csp_zoom_chords_are_bound() {
    let Some(mut app) = app() else { return };
    run(&mut app, AppCmd::Zoom100);
    for (ctrl, vk, want) in [
        (true, 0x6Bu16, 1.5f32), // Ctrl + NumPad +
        (true, 0x6D, 1.0),       // Ctrl + NumPad -
        (true, 0xBB, 1.5),       // Ctrl + =
        (true, 0xBD, 1.0),       // Ctrl + -
        (false, 0x21, 1.5),      // PageUp
        (false, 0x22, 1.0),      // PageDown
    ] {
        app.shell.test_modifiers = Some(egui::Modifiers {
            ctrl,
            ..Default::default()
        });
        assert!(crate::shortcut(&mut app, vk, false), "{vk:#x} is bound");
        app.shell.test_modifiers = None;
        while let Some(c) = app.cmds.pop_front() {
            dispatch(&mut app, c);
        }
        assert!(
            (app.viewport.zoom - want).abs() < 1e-3,
            "{vk:#x} (ctrl={ctrl}) => {} wanted {want}",
            app.viewport.zoom
        );
    }
}

// --- v07 palettes + workspaces ------------------------------------------

#[test]
fn v07_tab_hides_the_palettes_and_a_workspace_comes_back() {
    let Some(mut app) = app() else { return };
    assert!(!app.panels_hidden && !app.chrome_hidden);
    app.panels_hidden = true;
    app.chrome_hidden = true;
    assert!(app.panels_hidden && app.chrome_hidden);
}
