//! ROADMAP "further out": one-gesture screentone. The recipe it replaces
//! was select → new tone layer → deselect, so what these pin is that ONE
//! click produces the same STRUCTURE that recipe did (a live tone layer
//! with a window mask — parameters, never baked pixels), costs ONE undo
//! press, carries the Tool Property's parameters, and finds the region
//! through the fill machinery's gap closing.

use crate::cmd::{AppCmd, ToneToolOpts, dispatch};
use mn_core::tile::{FIX15_ONE, TileIdx};
use mn_core::{Document, FillKind, FillOpts, LayerKind, Selection, TonePattern};

const INK: [u16; 4] = [0, 0, 0, FIX15_ONE as u16];

/// A 256×256 page with nothing on it — small on purpose (the flood is
/// canvas-sized and the default new document is 2048²).
fn page(app: &mut crate::app::App) {
    app.doc = Document::new(256, 256);
}

fn paint(doc: &mut Document, x: i32, y: i32) {
    let idx = TileIdx::of_pixel(x, y);
    let (ox, oy) = idx.origin();
    doc.active_layer_mut()
        .tile_mut(idx)
        .set_pixel((x - ox) as usize, (y - oy) as usize, INK);
}

/// A hollow ink rectangle with a `gap`-px hole in the middle of its top edge.
fn draw_box(doc: &mut Document, x0: i32, y0: i32, x1: i32, y1: i32, gap: i32) {
    let from = (x0 + x1) / 2 - gap / 2;
    for x in x0..=x1 {
        if !(from..from + gap).contains(&x) {
            paint(doc, x, y0);
        }
        paint(doc, x, y1);
    }
    for y in y0..=y1 {
        paint(doc, x0, y);
        paint(doc, x1, y);
    }
}

/// The window mask's coverage at one canvas pixel (0 = outside the window).
fn mask_at(app: &crate::app::App, li: usize, x: i32, y: i32) -> u16 {
    let idx = TileIdx::of_pixel(x, y);
    let (ox, oy) = idx.origin();
    app.doc.layers[li]
        .mask
        .as_ref()
        .and_then(|m| m.tiles.get(&idx))
        .map(|t| t.pixel((x - ox) as usize, (y - oy) as usize)[3])
        .unwrap_or(0)
}

/// Tool Property with the flood's own softening turned off, so the asserts
/// measure the BARRIER rather than the 1 px tuck-under.
fn opts(gap_close_px: u32) -> ToneToolOpts {
    ToneToolOpts {
        region: FillOpts {
            gap_close_px,
            expand_px: 0,
            ..FillOpts::default()
        },
        ..ToneToolOpts::default()
    }
}

fn tone_layers(app: &crate::app::App) -> Vec<usize> {
    app.doc
        .layers
        .iter()
        .enumerate()
        .filter(|(_, l)| matches!(l.kind, LayerKind::Fill(FillKind::Tone { .. })))
        .map(|(i, _)| i)
        .collect()
}

/// The whole gesture in one press, and back out in one: the click makes a
/// LIVE tone layer (params + a window mask, no painted pixels), the Tool
/// Property's screen rides on it, the status line narrates, and a single
/// undo unmakes all of it.
#[test]
fn one_click_tones_a_region_as_one_undoable_live_layer() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    page(&mut app);
    draw_box(&mut app.doc, 40, 40, 200, 200, 0);
    app.doc.clear_history();
    app.tone_opts = ToneToolOpts {
        tone: mn_core::ToneParams {
            pattern: TonePattern::Lines,
            lpi: 45.0,
            angle_deg: 30.0,
            ..Default::default()
        },
        density: 0.4,
        ..opts(0)
    };
    let layers = app.doc.layers.len();
    let steps = app.doc.undo_labels().len();

    dispatch(&mut app, AppCmd::ToneRegion(120.0, 120.0));

    let tones = tone_layers(&app);
    assert_eq!(tones.len(), 1, "one click, one tone layer");
    let li = tones[0];
    assert_eq!(app.doc.layers.len(), layers + 1);
    // The parameters rode across, and they are PARAMETERS: the layer has a
    // window and no source pixels of its own.
    match app.doc.layers[li].kind {
        LayerKind::Fill(FillKind::Tone { tone, density }) => {
            assert_eq!(tone.pattern, TonePattern::Lines);
            assert!((tone.lpi - 45.0).abs() < 1e-6, "{}", tone.lpi);
            assert!((tone.angle_deg - 30.0).abs() < 1e-6);
            assert!((density - 0.4).abs() < 1e-6);
        }
        ref k => panic!("not a live tone layer: {k:?}"),
    }
    assert!(app.doc.layers[li].mask.is_some(), "the region is the window");
    assert!(
        app.doc.layers[li].tiles().next().is_none(),
        "a tone gesture bakes no pixels"
    );
    assert!(mask_at(&app, li, 120, 120) > 0, "inside the box is windowed");
    assert_eq!(mask_at(&app, li, 10, 10), 0, "outside it is not");

    // The status narrates the area and the screen.
    assert!(
        app.status.starts_with("toned region ") && app.status.contains("45 LPI"),
        "{}",
        app.status
    );

    // ONE undo press for the whole gesture.
    assert_eq!(app.doc.undo_labels().len(), steps + 1, "one history step");
    dispatch(&mut app, AppCmd::Undo);
    assert!(tone_layers(&app).is_empty(), "undo took the layer");
    assert_eq!(app.doc.layers.len(), layers);
    dispatch(&mut app, AppCmd::Redo);
    assert_eq!(tone_layers(&app).len(), 1, "redo put it back");
}

/// A second click on another region with different settings is its own
/// layer with its own screen — the first one does not move.
#[test]
fn a_second_click_tones_another_region_independently() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    page(&mut app);
    draw_box(&mut app.doc, 20, 20, 100, 100, 0);
    draw_box(&mut app.doc, 140, 140, 230, 230, 0);
    app.tone_opts = ToneToolOpts {
        tone: mn_core::ToneParams {
            lpi: 60.0,
            ..Default::default()
        },
        density: 1.0,
        ..opts(0)
    };
    dispatch(&mut app, AppCmd::ToneRegion(60.0, 60.0));
    app.tone_opts = ToneToolOpts {
        tone: mn_core::ToneParams {
            pattern: TonePattern::Star,
            lpi: 25.0,
            ..Default::default()
        },
        density: 0.2,
        ..opts(0)
    };
    dispatch(&mut app, AppCmd::ToneRegion(185.0, 185.0));

    let tones = tone_layers(&app);
    assert_eq!(tones.len(), 2, "two clicks, two layers");
    let read = |app: &crate::app::App, li: usize| match app.doc.layers[li].kind {
        LayerKind::Fill(FillKind::Tone { tone, density }) => (tone.pattern, tone.lpi, density),
        ref k => panic!("{k:?}"),
    };
    let (a, b) = (read(&app, tones[0]), read(&app, tones[1]));
    assert_eq!(a.0, TonePattern::Dots);
    assert!((a.1 - 60.0).abs() < 1e-6 && (a.2 - 1.0).abs() < 1e-6, "{a:?}");
    assert_eq!(b.0, TonePattern::Star);
    assert!((b.1 - 25.0).abs() < 1e-6 && (b.2 - 0.2).abs() < 1e-6, "{b:?}");

    // And each window holds only its own pocket.
    assert!(mask_at(&app, tones[0], 60, 60) > 0);
    assert_eq!(mask_at(&app, tones[0], 185, 185), 0);
    assert!(mask_at(&app, tones[1], 185, 185) > 0);
    assert_eq!(mask_at(&app, tones[1], 60, 60), 0);
}

/// The region comes from the fill machinery, gap closing included: with the
/// option off, a 3 px break in the outline lets the tone out over the whole
/// page; with it on, the same click stays inside.
#[test]
fn enclosed_region_detection_respects_gap_closing() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    page(&mut app);
    draw_box(&mut app.doc, 40, 40, 200, 200, 3);

    app.tone_opts = opts(0);
    dispatch(&mut app, AppCmd::ToneRegion(120.0, 120.0));
    let leaky = *tone_layers(&app).first().expect("a layer was made");
    assert!(
        mask_at(&app, leaky, 10, 10) > 0,
        "without gap closing the tone escapes the gap"
    );
    dispatch(&mut app, AppCmd::Undo);
    assert!(tone_layers(&app).is_empty());

    app.tone_opts = opts(2);
    dispatch(&mut app, AppCmd::ToneRegion(120.0, 120.0));
    let sealed = *tone_layers(&app).first().expect("a layer was made");
    assert!(mask_at(&app, sealed, 120, 120) > 0, "the area is toned");
    assert_eq!(
        mask_at(&app, sealed, 10, 10),
        0,
        "gap sealed — the tone stayed in the area"
    );
}

/// The deselect step is what the gesture removes, so it must not leave a
/// selection behind — and a selection the artist made on purpose still
/// clips the window, exactly as it clips a bucket fill.
#[test]
fn the_gesture_leaves_no_selection_and_an_existing_one_still_clips() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    page(&mut app);
    draw_box(&mut app.doc, 40, 40, 200, 200, 0);
    app.tone_opts = opts(0);

    dispatch(&mut app, AppCmd::ToneRegion(120.0, 120.0));
    assert!(
        app.doc.selection.is_none(),
        "no marching ants left to dismiss"
    );
    dispatch(&mut app, AppCmd::Undo);

    let sel = Selection::from_rect(&app.doc, 0.0, 0.0, 128.0, 256.0);
    app.doc.selection = Some(sel);
    dispatch(&mut app, AppCmd::ToneRegion(120.0, 120.0));
    let li = *tone_layers(&app).first().expect("a layer was made");
    assert!(mask_at(&app, li, 100, 120) > 0, "inside the selection");
    assert_eq!(mask_at(&app, li, 180, 120), 0, "outside it, untoned");
    assert!(
        app.doc.selection.is_some(),
        "the artist's own selection survives the gesture"
    );
}
