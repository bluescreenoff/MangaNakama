//! Friction 8: the Selection Launcher's "New tone" button. CSP's launcher
//! offers 新規トーン right after Fill, and the reflex it serves is "screen
//! this area I just selected". The button is a door onto the existing live
//! tone command, so the test presses the door and checks the three things
//! the mangaka is owed: a tone layer, DOTS (not a black slab), and dots
//! that stop at the marching ants.

use crate::cmd::dispatch;
use crate::ui::launcher::new_tone_cmd;
use mn_core::tile::TileIdx;
use mn_core::{FillKind, LayerKind};

/// A small page with a selection over its top-left quarter.
fn page_with_selection(app: &mut crate::app::App) {
    app.doc = mn_core::Document::new(256, 256);
    app.doc.selection = Some(mn_core::Selection::from_rect(
        &app.doc, 0.0, 0.0, 128.0, 128.0,
    ));
}

/// Alpha of one derived pixel of the layer (the fill raster, not painted
/// pixels — a live layer has none).
fn alpha(app: &crate::app::App, li: usize, x: i32, y: i32) -> u16 {
    let ti = TileIdx::of_pixel(x, y);
    app.doc.layers[li]
        .display_tile(ti)
        .map(|t| t.pixel((x - ti.origin().0) as usize, (y - ti.origin().1) as usize)[3])
        .unwrap_or(0)
}

#[test]
fn the_launcher_tone_button_screens_the_selection_and_nothing_outside_it() {
    let Some(mut app) = super::new_document_tests::headless() else {
        println!("[test] SKIP: no usable adapter");
        return;
    };
    page_with_selection(&mut app);
    let before = app.doc.layers.len();

    dispatch(&mut app, new_tone_cmd());

    assert_eq!(app.doc.layers.len(), before + 1, "one press, one layer");
    let li = app.doc.active;
    let LayerKind::Fill(FillKind::Tone { tone, density }) = app.doc.layers[li].kind else {
        panic!("the button must make a LIVE tone layer: {:?}", app.doc.layers[li].kind);
    };
    assert_eq!(tone.lpi, 60.0, "the house default screen");
    assert!(
        app.doc.layers[li].mask.is_some(),
        "the selection cut the layer's window"
    );

    // Inside: a screen, which means SOME ink and SOME paper between the
    // dots. `density: 1.0` — the friction-11 mistake — makes every pixel
    // opaque, and this is the assertion that catches it.
    let mut inked = 0;
    let mut clear = 0;
    for y in 8..120 {
        for x in 8..120 {
            if alpha(&app, li, x, y) > 0 {
                inked += 1;
            } else {
                clear += 1;
            }
        }
    }
    assert!(inked > 0, "the tone laid dots inside the selection");
    assert!(
        clear > 0,
        "dots, not a solid black slab ({inked} inked, {clear} clear)"
    );
    assert_eq!(density, 0.4, "the Tone tool's own default density");

    // Outside: the window mask keeps whole tiles out of the raster.
    assert!(
        app.doc.layers[li].display_tile(TileIdx::of_pixel(200, 200)).is_none(),
        "nothing is screened outside the ants"
    );
}
