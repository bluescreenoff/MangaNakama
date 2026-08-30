//! Leak-repair refill, end to end through the real gesture path: the
//! command arms, the canvas arms capture (or run) the closing stroke,
//! the fill re-runs from the remembered seed — and the whole recovery is
//! what the owner asked for: one command, one stroke, one undo press.

use super::new_document_tests::{headless, scribble};
use crate::app::PointerKind;
use crate::cmd::{AppCmd, dispatch};
use mn_core::tile::{FIX15_ONE, TileIdx};

const INK: [u16; 4] = [0, 0, 0, FIX15_ONE as u16];
const GAP_FROM: i32 = 56;
const GAP_TO: i32 = 72;
/// The U: walls at these bands, the gap in the bottom one.
const X0: i32 = 20;
const X1: i32 = 108;
const Y0: i32 = 20;
const Y1: i32 = 108;

fn paint(doc: &mut mn_core::Document, x: i32, y: i32) {
    let idx = TileIdx::of_pixel(x, y);
    let (ox, oy) = idx.origin();
    doc.active_layer_mut()
        .tile_mut(idx)
        .set_pixel((x - ox) as usize, (y - oy) as usize, INK);
}

/// Lineart below, a flats layer above it active for the fill. The U's
/// bottom band has a 16-px gap — big enough that the default 2-px gap
/// close cannot seal it.
fn u_shaped_gap(app: &mut crate::App) {
    let lineart = app.doc.active;
    app.doc.layers[lineart].name = "lineart".into();
    for x in X0..=X1 {
        paint(&mut app.doc, x, Y0);
        if !(GAP_FROM..GAP_TO).contains(&x) {
            paint(&mut app.doc, x, Y1);
        }
    }
    for y in Y0..=Y1 {
        paint(&mut app.doc, X0, y);
        paint(&mut app.doc, X1, y);
    }
    let flats = app.doc.add_layer("flats");
    app.doc.set_active(flats);
}

/// A pixel of the ACTIVE layer, as straight RGBA halves (fix15).
fn px(app: &crate::App, li: usize, x: i32, y: i32) -> [u16; 4] {
    let idx = TileIdx::of_pixel(x, y);
    let (ox, oy) = idx.origin();
    app.doc.layers[li]
        .display_tile(idx)
        .map(|t| t.pixel((x - ox) as usize, (y - oy) as usize))
        .unwrap_or([0, 0, 0, 0])
}

/// Screen coords for a canvas point, so the drives go through the REAL
/// pointer arms whatever the viewport is.
fn s(app: &crate::App, cx: f32, cy: f32) -> (f32, f32) {
    app.viewport.to_screen(cx, cy)
}

fn fill_layer(app: &crate::App) -> usize {
    app.doc
        .layers
        .iter()
        .position(|l| l.name == "flats")
        .expect("flats layer")
}

fn lineart_layer(app: &crate::App) -> usize {
    app.doc
        .layers
        .iter()
        .position(|l| l.name == "lineart")
        .expect("lineart layer")
}

/// The headline story: a fill leaks through the gap, ONE command arms
/// repair, the closing stroke is a virtual barrier, the fill re-runs
/// itself contained — and one Ctrl+Z takes the whole recovery back.
#[test]
fn a_leaked_fill_repairs_itself_through_a_virtual_barrier() {
    let Some(mut app) = headless() else { return };
    u_shaped_gap(&mut app);
    let flats = fill_layer(&app);
    let lineart = lineart_layer(&app);
    dispatch(&mut app, AppCmd::SetSlotColor([1.0, 0.0, 0.0]));

    let pre_leak = mn_core::export::composite(&app.doc, mn_core::Background::Transparent);

    // The leak: the naive click fill floods out through the gap.
    dispatch(&mut app, AppCmd::Fill(60.0, 60.0));
    assert!(
        px(&app, flats, 60, 124)[3] > 0,
        "the naive fill leaked outside the U"
    );
    let lineart_before = px(&app, lineart, 64, 106);

    // Arm, stroke, done — the two actions the idea exists for.
    dispatch(
        &mut app,
        AppCmd::ArmFillRepair {
            virtual_barrier: true,
        },
    );
    assert!(
        px(&app, flats, 60, 60)[3] == 0,
        "arming undid the leaked fill first"
    );
    let (ax, ay) = s(&app, 52.0, 106.0);
    app.canvas_down(ax, ay, PointerKind::Mouse, &[]);
    for x in [56.0, 60.0, 64.0, 68.0, 72.0] {
        let (mx, my) = s(&app, x, 106.0);
        app.canvas_move(mx, my, &[]);
    }
    let (ux, uy) = s(&app, 76.0, 106.0);
    app.canvas_up(ux, uy, &[]);

    // Contained: inside filled, outside paper.
    let inside = px(&app, flats, 60, 60);
    assert!(inside[0] > 20000 && inside[3] > 0, "inside is the red fill");
    assert_eq!(
        px(&app, flats, 60, 124)[3],
        0,
        "the repaired fill stayed inside the U"
    );
    // Zero residue: the barrier added no ink anywhere — the lineart is
    // byte-identical at the gap and beyond.
    assert_eq!(
        px(&app, lineart, 64, 106),
        lineart_before,
        "the virtual barrier left no ink"
    );
    assert!(app.fill_repair.is_none(), "the gesture closed");

    // One press: the leak was undone at arm time, so ONE undo on the
    // repaired fill returns the pre-leak picture exactly.
    dispatch(&mut app, AppCmd::Undo);
    let after = mn_core::export::composite(&app.doc, mn_core::Background::Transparent);
    assert_eq!(
        after.as_raw(), pre_leak.as_raw(),
        "one Ctrl+Z returns the pre-leak state"
    );
}

/// The barrier's LIFETIME: once the repair is done, a later fill does
/// not see the wall — it leaks through the same gap again, because the
/// barrier composited nowhere and saved nowhere.
#[test]
fn a_second_fill_does_not_see_the_dead_barrier() {
    let Some(mut app) = headless() else { return };
    u_shaped_gap(&mut app);
    let flats = fill_layer(&app);

    dispatch(&mut app, AppCmd::SetSlotColor([1.0, 0.0, 0.0]));
    dispatch(&mut app, AppCmd::Fill(60.0, 60.0));
    dispatch(
        &mut app,
        AppCmd::ArmFillRepair {
            virtual_barrier: true,
        },
    );
    let (ax, ay) = s(&app, 52.0, 106.0);
    app.canvas_down(ax, ay, PointerKind::Mouse, &[]);
    for x in [56.0, 60.0, 64.0, 68.0, 72.0] {
        let (mx, my) = s(&app, x, 106.0);
        app.canvas_move(mx, my, &[]);
    }
    let (ux, uy) = s(&app, 76.0, 106.0);
    app.canvas_up(ux, uy, &[]);
    assert_eq!(px(&app, flats, 60, 124)[3], 0, "contained by the barrier");

    // A fresh fill seeded OUTSIDE, in blue: if any wall survived, it
    // would stop at the gap; it must flood straight through into the U.
    dispatch(&mut app, AppCmd::SetSlotColor([0.0, 0.0, 1.0]));
    dispatch(&mut app, AppCmd::Fill(64.0, 120.0));
    let entered = px(&app, flats, 40, 90);
    assert!(
        entered[2] > 20000 && entered[0] < 5000,
        "the second fill crossed the dead barrier into the U"
    );
}

/// Choice B: the closing stroke is REAL ink — it stays (that is the
/// point of choosing it), the fill re-runs behind it, and stroke + fill
/// are ONE undo press.
#[test]
fn a_real_ink_barrier_persists_and_wraps_into_one_undo() {
    let Some(mut app) = headless() else { return };
    u_shaped_gap(&mut app);
    let flats = fill_layer(&app);
    let lineart = lineart_layer(&app);
    dispatch(&mut app, AppCmd::SetSlotColor([1.0, 0.0, 0.0]));
    let pre_leak = mn_core::export::composite(&app.doc, mn_core::Background::Transparent);

    dispatch(&mut app, AppCmd::Fill(60.0, 60.0));
    assert!(px(&app, flats, 60, 124)[3] > 0, "the leak");

    // Arm for real ink, pick up the pen, close the gap in the LINEART.
    dispatch(
        &mut app,
        AppCmd::ArmFillRepair {
            virtual_barrier: false,
        },
    );
    dispatch(&mut app, AppCmd::SetTool(crate::cmd::Tool::Pen));
    app.doc.set_active(lineart);
    dispatch(&mut app, AppCmd::SetBrushSizePx(9.0));
    let (ax, ay) = s(&app, 52.0, 106.0);
    app.canvas_down(ax, ay, PointerKind::Mouse, &[]);
    for x in [56.0, 60.0, 64.0, 68.0, 72.0] {
        let (mx, my) = s(&app, x, 106.0);
        app.canvas_move(
            mx,
            my,
            &[crate::app::PenSample {
                x: mx,
                y: my,
                pressure: 0.9,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: 0.0,
            }],
        );
    }
    let (ux, uy) = s(&app, 76.0, 106.0);
    app.canvas_up(ux, uy, &[]);

    assert!(app.fill_repair.is_none(), "the gesture closed");
    // The ink PERSISTED — real is real.
    assert!(
        px(&app, lineart, 64, 106)[3] > 0,
        "the closing stroke stayed on the lineart"
    );
    // The refill ran behind it and stayed inside.
    assert!(px(&app, flats, 60, 60)[3] > 0, "the fill re-ran");
    assert_eq!(px(&app, flats, 60, 124)[3], 0, "contained by the ink");

    // ONE undo takes stroke + refill both back — the pre-leak picture.
    dispatch(&mut app, AppCmd::Undo);
    let after = mn_core::export::composite(&app.doc, mn_core::Background::Transparent);
    assert_eq!(
        after.as_raw(), pre_leak.as_raw(),
        "one Ctrl+Z returns the pre-leak state (ink and fill together)"
    );
}

/// The refusals: nothing remembered, the page moved, the layer went, or
/// the fill is no longer the newest step — each answers with a status
/// line and arms nothing.
#[test]
fn arming_refuses_when_it_cannot_be_safe() {
    let Some(mut app) = headless() else { return };
    u_shaped_gap(&mut app);

    // Nothing remembered yet.
    dispatch(
        &mut app,
        AppCmd::ArmFillRepair {
            virtual_barrier: true,
        },
    );
    assert!(app.status.contains("no fill to repair"), "{:?}", app.status);
    assert!(app.fill_repair.is_none());

    // A fill, then more work on top: the fill is no longer the newest.
    dispatch(&mut app, AppCmd::Fill(60.0, 60.0));
    scribble(&mut app);
    dispatch(
        &mut app,
        AppCmd::ArmFillRepair {
            virtual_barrier: true,
        },
    );
    assert!(
        app.status.contains("no longer the newest"),
        "{:?}",
        app.status
    );
    assert!(app.fill_repair.is_none());

    // A wrong page in the memory: refused.
    let true_page = app.pages.get(app.page_index).map(|p| p.uid).unwrap_or(0);
    app.last_fill.as_mut().unwrap().page_uid = 999;
    dispatch(
        &mut app,
        AppCmd::ArmFillRepair {
            virtual_barrier: true,
        },
    );
    assert!(app.status.contains("another page"), "{:?}", app.status);
    app.last_fill.as_mut().unwrap().page_uid = true_page;

    // The filled layer deleted: refused (a structural removal clears the
    // history, so this checks the LAYER-resolution refusal, not the
    // newest-step one).
    let f = app.last_fill.clone().unwrap();
    let li = app
        .doc
        .layer_index_of(f.layer_id)
        .expect("the flats layer exists");
    app.doc.layers.remove(li);
    app.doc.set_active(0);
    dispatch(
        &mut app,
        AppCmd::ArmFillRepair {
            virtual_barrier: true,
        },
    );
    assert!(app.status.contains("layer is gone"), "{:?}", app.status);
    assert!(app.fill_repair.is_none());
}
