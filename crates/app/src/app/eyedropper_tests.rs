use super::new_document_tests::headless;
use crate::app::App;
use crate::cmd::{AppCmd, dispatch};
use mn_core::{FIX15_ONE, FillRefer, TileIdx};

/// Opaque ink at a canvas pixel inside the first tile.
fn ink(app: &mut App, layer: usize, x: usize, y: usize, rgb: [u16; 3]) {
    app.doc.layers[layer]
        .tile_mut(TileIdx::new(0, 0))
        .set_pixel(x, y, [rgb[0], rgb[1], rgb[2], FIX15_ONE as u16]);
}

const BLACK: [u16; 3] = [0, 0, 0];
const RED: [u16; 3] = [FIX15_ONE as u16, 0, 0];
const BLUE: [u16; 3] = [0, 0, FIX15_ONE as u16];

/// What the active slot ended up holding, as bytes.
fn picked(app: &App) -> [u8; 3] {
    app.active_color()
        .map(|c| (c * 255.0).round().clamp(0.0, 255.0) as u8)
}

/// THE PIN (house rule: defaults must not change behaviour). A fresh app
/// picks the single pixel of the visible composite, byte for byte the
/// same colour it picked before any of this existed.
#[test]
fn the_default_pick_is_one_pixel_of_what_you_see() {
    let Some(mut app) = headless() else {
        println!("[test] SKIP: no usable adapter");
        return;
    };
    assert_eq!(app.eyedrop_opts.refer, FillRefer::All, "composite default");
    assert_eq!(app.eyedrop_opts.size, 1, "single pixel default");

    ink(&mut app, 0, 10, 10, BLACK);
    dispatch(&mut app, AppCmd::PickColor(10.0, 10.0));
    assert_eq!(picked(&app), [0, 0, 0], "the ink");
    assert_eq!(
        Some(picked(&app)),
        mn_core::export::composite_pixel(&app.doc, 10, 10),
        "and it is exactly the one-pixel sampler's answer"
    );
    dispatch(&mut app, AppCmd::PickColor(11.0, 10.0));
    assert_eq!(picked(&app), [255, 255, 255], "bare paper next door");
    dispatch(&mut app, AppCmd::PickColor(-3.0, 10.0));
    assert!(app.status.contains("outside"), "{:?}", app.status);
}

/// E-016. Two inked pixels and two of paper: the answer is the light
/// grey the patch READS as (~188), not the mean of the bytes (128).
/// Same curve as the mip downsample, so the pick agrees with what the
/// zoomed-out canvas shows for that patch.
#[test]
fn the_average_reads_the_area_not_the_bytes() {
    let Some(mut app) = headless() else {
        println!("[test] SKIP: no usable adapter");
        return;
    };
    ink(&mut app, 0, 10, 10, BLACK);
    ink(&mut app, 0, 11, 10, BLACK);

    app.eyedrop_opts.size = 2;
    dispatch(&mut app, AppCmd::PickColor(10.0, 10.0));
    let got = picked(&app);
    assert!(
        (185..=191).contains(&got[0]),
        "half ink half paper must read ~188, got {got:?} (128 = averaged the bytes)"
    );
    assert!(
        app.status.contains("2×2"),
        "the status names it: {:?}",
        app.status
    );

    // The radius is the only thing that changed: back to 1×1 and the
    // same click is the old single-pixel answer again.
    app.eyedrop_opts.size = 1;
    dispatch(&mut app, AppCmd::PickColor(10.0, 10.0));
    assert_eq!(picked(&app), [0, 0, 0]);
}

/// E-014. The reference SET is sampled even where its own eye is off —
/// the point of marking roughs as reference is to keep them hidden.
#[test]
fn the_reference_set_is_picked_with_its_eye_off() {
    let Some(mut app) = headless() else {
        println!("[test] SKIP: no usable adapter");
        return;
    };
    // Bottom layer = the (hidden) reference; a red layer covers it.
    ink(&mut app, 0, 10, 10, BLUE);
    app.doc.set_layer_reference(0, true);
    app.doc.set_layer_visible(0, false);
    let top = app.doc.add_layer("over");
    ink(&mut app, top, 10, 10, RED);

    dispatch(&mut app, AppCmd::PickColor(10.0, 10.0));
    assert_eq!(picked(&app), [255, 0, 0], "the composite shows red");

    app.eyedrop_opts.refer = FillRefer::Reference;
    dispatch(&mut app, AppCmd::PickColor(10.0, 10.0));
    assert_eq!(picked(&app), [0, 0, 255], "the hidden reference layer");

    // The editing layer's own ink, whichever layer that is.
    app.eyedrop_opts.refer = FillRefer::Active;
    app.doc.set_active(0);
    dispatch(&mut app, AppCmd::PickColor(10.0, 10.0));
    assert_eq!(picked(&app), [0, 0, 255], "layer 0's own ink");
    app.doc.set_active(top);
    dispatch(&mut app, AppCmd::PickColor(10.0, 10.0));
    assert_eq!(picked(&app), [255, 0, 0], "the top layer's own ink");
}

/// Nothing marked as reference is the everyday state — the pick falls
/// back to what you see and SAYS SO, rather than returning bare paper.
#[test]
fn reference_mode_with_no_reference_layer_falls_back_and_says_so() {
    let Some(mut app) = headless() else {
        println!("[test] SKIP: no usable adapter");
        return;
    };
    ink(&mut app, 0, 10, 10, RED);
    app.eyedrop_opts.refer = FillRefer::Reference;
    assert!(app.doc.reference_layers().is_empty());

    dispatch(&mut app, AppCmd::PickColor(10.0, 10.0));
    assert_eq!(picked(&app), [255, 0, 0], "what you see");
    assert!(
        app.status.contains("no reference layer"),
        "the fallback must be visible, got {:?}",
        app.status
    );
}

/// The `,`/`.` sub-tool stepper walks all three referents now, and both
/// directions land somewhere different.
#[test]
fn the_subtool_stepper_walks_the_three_referents() {
    let Some(mut app) = headless() else {
        println!("[test] SKIP: no usable adapter");
        return;
    };
    app.tool = crate::cmd::Tool::Eyedrop;
    app.step_subtool(true);
    assert_eq!(app.eyedrop_opts.refer, FillRefer::Active);
    app.step_subtool(true);
    assert_eq!(app.eyedrop_opts.refer, FillRefer::Reference);
    app.step_subtool(true);
    assert_eq!(app.eyedrop_opts.refer, FillRefer::All, "wraps");
    app.step_subtool(false);
    assert_eq!(app.eyedrop_opts.refer, FillRefer::Reference, "and back");
}
