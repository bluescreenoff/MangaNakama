//! Item P — "I drew a balloon with the balloon pen over text but it just
//! added the balloon as a new layer above the text layer instead of
//! combining them: visually in the layer list, and functionally with the K
//! move tool. They should still be separately selectable via the Text tool
//! or the Object tool."
//!
//! One vector kind, `LayerKind::Speech { texts, balloons }`, is the answer.
//! These tests drive the four things the owner named: where a drawn bubble
//! lands, what the palette row says, that moving the layer moves both, and
//! that the Object tool still picks each one on its own.

use crate::app::PointerKind;
use mn_core::PenSample;
use crate::cmd::{AppCmd, dispatch};

fn bubble(cx: f32, cy: f32, rx: f32, ry: f32) -> mn_core::Balloon {
    mn_core::Balloon {
        shape: mn_core::BalloonShape::Ellipse {
            center: [cx, cy],
            radii: [rx, ry],
        },
        ..Default::default()
    }
}

fn lettering(pos: [f32; 2], size: [f32; 2], s: &str) -> mn_core::TextItem {
    let mut t = mn_core::TextItem::new(pos, "Gothic".into(), 9.0, [0, 0, 0], true);
    t.text = s.into();
    let n = t.utf16_len();
    t.runs = vec![mn_core::text::StyleRun::plain(n)];
    t.size = size;
    t.auto_size = false;
    t
}

/// Lettering at (380,360) sized 80×100, plus the bubble that encloses it.
fn text_layer_with_lettering(app: &mut crate::App) -> usize {
    app.doc.add_text_layer(
        "lettering",
        mn_core::TextSet {
            texts: vec![lettering([380.0, 360.0], [80.0, 100.0], "オイ")],
        },
    )
}

/// The owner's sentence, as a test: a bubble drawn AROUND lettering joins
/// that lettering's own layer instead of stacking a new layer on top of it.
#[test]
fn a_balloon_drawn_around_a_text_lands_on_the_texts_layer() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let tl = text_layer_with_lettering(&mut app);
    let n_before = app.doc.layers.len();

    dispatch(
        &mut app,
        AppCmd::BalloonAdd {
            balloon: bubble(420.0, 410.0, 140.0, 100.0),
        },
    );

    assert_eq!(
        app.doc.layers.len(),
        n_before,
        "no new layer: the bubble joined the words"
    );
    assert_eq!(
        app.doc.layers[tl].balloons().unwrap().balloons.len(),
        1,
        "the bubble is on the lettering's layer"
    );
    assert_eq!(app.doc.active, tl, "and that layer is the active one");
    let l = &app.doc.layers[tl];
    assert!(l.is_text() && l.is_balloon(), "one layer, both halves");
    // One row in the palette, and it wears the combined glyph.
    assert_eq!(
        crate::ui::layers::rows::row_glyph(l),
        Some(crate::ui::icons::Icon::Speech),
        "the row says 'words + bubble'"
    );
    // The first bubble on a words-only layer takes the Tool Property width,
    // because an empty set has none of its own to inherit.
    let want = app.mm_to_px(app.balloon_border_mm).max(2.0);
    assert!(
        (app.doc.layers[tl].balloons().unwrap().border_px - want).abs() < 1e-3,
        "border from Tool Property, not the placeholder"
    );
}

/// The fallback is untouched: a bubble drawn over bare paper still goes
/// where it always went — its own layer when the page has no balloon layer.
#[test]
fn a_balloon_drawn_away_from_any_text_keeps_the_old_rule() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    text_layer_with_lettering(&mut app);
    let n_before = app.doc.layers.len();
    dispatch(
        &mut app,
        AppCmd::BalloonAdd {
            balloon: bubble(1000.0, 700.0, 90.0, 60.0),
        },
    );
    assert_eq!(app.doc.layers.len(), n_before + 1, "a fresh balloon layer");
    let li = app.doc.active;
    assert!(app.doc.layers[li].is_balloon() && !app.doc.layers[li].is_text());
}

/// The K half of the complaint: with both on one layer, moving the layer
/// moves the bubble AND the words, by the same amount.
#[test]
fn moving_the_speech_layer_moves_the_words_and_the_bubble() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let tl = text_layer_with_lettering(&mut app);
    dispatch(
        &mut app,
        AppCmd::BalloonAdd {
            balloon: bubble(420.0, 410.0, 140.0, 100.0),
        },
    );
    let pos0 = app.doc.layers[tl].texts().unwrap().texts[0].pos;
    let mn_core::BalloonShape::Ellipse { center: c0, .. } =
        app.doc.layers[tl].balloons().unwrap().balloons[0].shape
    else {
        panic!("ellipse")
    };

    app.doc.layers[tl].translate_content(37, -21);

    let pos1 = app.doc.layers[tl].texts().unwrap().texts[0].pos;
    let mn_core::BalloonShape::Ellipse { center: c1, .. } =
        app.doc.layers[tl].balloons().unwrap().balloons[0].shape
    else {
        panic!("ellipse")
    };
    assert_eq!(pos1, [pos0[0] + 37.0, pos0[1] - 21.0], "the words moved");
    assert_eq!(
        c1,
        [c0[0] + 37.0, c0[1] - 21.0],
        "the bubble moved with them"
    );
}

/// "They should still be separately selectable." One Object-tool click
/// picks the words; the cycle key steps to the bubble on the SAME layer,
/// and neither selection drags the other along.
#[test]
fn the_object_tool_still_picks_the_text_and_the_bubble_apart() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let tl = text_layer_with_lettering(&mut app);
    dispatch(
        &mut app,
        AppCmd::BalloonAdd {
            balloon: bubble(420.0, 410.0, 140.0, 100.0),
        },
    );

    // A point inside the lettering box AND inside the bubble.
    app.tool = crate::cmd::Tool::Object;
    let empty: [PenSample; 0] = [];
    let (x, y) = app.viewport.to_screen(410.0, 400.0);
    app.canvas_down(x, y, PointerKind::Mouse, &empty);
    app.canvas_up(x, y, &empty);
    assert_eq!(app.text_sel, Some((tl, 0)), "the words claim the click");
    assert_eq!(app.balloon_sel, None, "and only the words");

    app.object_cycle(true);
    assert_eq!(
        app.balloon_sel,
        Some((tl, 0)),
        "the cycle reaches the bubble on the same layer"
    );
    assert_eq!(app.text_sel, None, "and lets the words go");
}

/// The mirror rule: words typed INSIDE an existing bubble join that
/// bubble's layer, so the pair moves as one from the first keystroke.
#[test]
fn words_typed_inside_a_bubble_join_the_bubbles_layer() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    if app.text_engine.is_none() {
        return; // no DirectWrite on this box; the text half cannot run
    }
    let mut bs = mn_core::BalloonSet::new(4.0);
    bs.balloons.push(bubble(400.0, 400.0, 160.0, 120.0));
    let bl = app.doc.add_balloon_layer("bubble", bs);
    let n_before = app.doc.layers.len();

    app.tool = crate::cmd::Tool::Text;
    app.start_new_text([400.0, 400.0], None);

    assert_eq!(
        app.doc.layers.len(),
        n_before,
        "no second layer for the words"
    );
    assert_eq!(
        app.text_edit.as_ref().map(|e| e.layer),
        Some(bl),
        "the caret opened on the bubble's own layer"
    );
    assert_eq!(app.doc.active, bl, "and that layer is active");
}
