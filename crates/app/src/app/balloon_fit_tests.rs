//! ROADMAP good-first-issue #1 — "Fit a balloon to its text", driven the way
//! the Tool Property button drives it: `App::fit_balloon_to_text`, then the
//! command queue, then one `Undo`.

use crate::cmd::{AppCmd, dispatch};

fn bubble(cx: f32, cy: f32, rx: f32, ry: f32) -> mn_core::Balloon {
    let mut b = mn_core::Balloon {
        shape: mn_core::BalloonShape::Ellipse {
            center: [cx, cy],
            radii: [rx, ry],
        },
        ..Default::default()
    };
    b.tails.push(mn_core::Tail {
        base: [cx + rx, cy],
        tip: [cx + 240.0, cy + 200.0],
        width: 18.0,
        ..Default::default()
    });
    b
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

fn radii(app: &crate::App, layer: usize) -> [f32; 2] {
    match &app.doc.layers[layer]
        .balloons()
        .expect("balloon layer")
        .balloons[0]
        .shape
    {
        mn_core::BalloonShape::Ellipse { radii, .. } => *radii,
        s => panic!("expected an ellipse, got {s:?}"),
    }
}

#[test]
fn fit_balloon_to_text_sizes_around_the_topmost_visible_lettering() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    // A bubble far too small for what is written in it.
    let bl = app.doc.add_balloon_layer(
        "bubbles",
        mn_core::BalloonSet {
            balloons: vec![bubble(400.0, 400.0, 20.0, 20.0)],
            border_px: 4.0,
            pressure_width: false,
        },
    );
    // Lettering inside it, on a VISIBLE layer…
    app.doc.add_text_layer(
        "lettering",
        mn_core::TextSet {
            texts: vec![lettering([360.0, 320.0], [80.0, 160.0], "オイ")],
        },
    );
    // …a HIDDEN draft in the same spot, which must not be what we size to…
    let hidden = app.doc.add_text_layer(
        "draft",
        mn_core::TextSet {
            texts: vec![lettering([100.0, 100.0], [600.0, 600.0], "draft")],
        },
    );
    app.doc.layers[hidden].visible = false;
    // …and an SFX well outside the bubble.
    app.doc.add_text_layer(
        "sfx",
        mn_core::TextSet {
            texts: vec![lettering([1400.0, 1200.0], [80.0, 160.0], "ドン")],
        },
    );

    let before = app.doc.layers[bl].balloons().unwrap().clone();
    app.fit_balloon_to_text(bl, 0);
    while let Some(c) = app.cmds.pop_front() {
        dispatch(&mut app, c);
    }

    let after = radii(&app, bl);
    assert!(
        after[0] > 20.0 && after[1] > 20.0,
        "the bubble grew around its lettering: {after:?}"
    );
    assert!(
        after[1] > after[0],
        "tategaki lettering fits taller than wide: {after:?}"
    );
    // The hidden layer's enormous box would have blown the bubble up to the
    // whole page; it did not, so the hidden layer was skipped.
    assert!(
        after[0] < 300.0,
        "the hidden draft was not sized to: {after:?}"
    );
    assert!(app.status.contains("fitted"), "announced: {}", app.status);

    let bs = app.doc.layers[bl].balloons().unwrap();
    assert_eq!(bs.balloons[0].tails.len(), 1, "the tail survived");
    assert_eq!(
        bs.balloons[0].tails[0].tip,
        [640.0, 600.0],
        "the speaker did not move"
    );
    assert_eq!(bs.border_px, before.border_px, "the style is untouched");

    // ONE press, ONE undo — the whole reshape rides `set_balloons`.
    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(
        app.doc.layers[bl].balloons().unwrap(),
        &before,
        "one undo restores the old shape exactly"
    );
}

#[test]
fn fitting_a_balloon_with_no_lettering_in_it_changes_nothing() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let bl = app.doc.add_balloon_layer(
        "bubbles",
        mn_core::BalloonSet {
            balloons: vec![bubble(400.0, 400.0, 60.0, 40.0)],
            border_px: 4.0,
            pressure_width: false,
        },
    );
    app.doc.add_text_layer(
        "sfx",
        mn_core::TextSet {
            texts: vec![lettering([1400.0, 1200.0], [80.0, 160.0], "ドン")],
        },
    );
    let before = app.doc.layers[bl].balloons().unwrap().clone();

    app.fit_balloon_to_text(bl, 0);
    assert!(
        app.cmds.is_empty(),
        "nothing to fit around ⇒ no undo step is spent"
    );
    assert_eq!(app.doc.layers[bl].balloons().unwrap(), &before);
    assert!(
        app.status.contains("no lettering"),
        "said so out loud: {}",
        app.status
    );
}

// --- L-03: a text clicked inside a balloon wraps to it --------------------

/// Click in the middle of `bl`'s only bubble with the Text tool at 16 pt and
/// type `s`. Returns the bubble's interior box, canvas px.
fn click_and_type(app: &mut crate::App, bl: usize, at: [f32; 2], s: &str) -> [f32; 4] {
    app.tool = crate::cmd::Tool::Text;
    app.text_vertical = false;
    app.text_size_pt = 16.0;
    let dpi = app.doc_dpi();
    let em = mn_text::font_px(
        &mn_core::TextItem::new(at, String::new(), 16.0, [0, 0, 0], false),
        dpi,
    );
    let inner = app.doc.layers[bl].balloons().unwrap().balloons[0]
        .text_interior(em)
        .expect("the bubble has room for a box");
    app.text_tool_down(at[0], at[1], false, 1);
    app.text_tool_up(at[0], at[1]);
    for u in s.encode_utf16() {
        app.text_char(u);
    }
    inner
}

/// Ledger `L-03` / `S-14` — **a text box clicked inside a balloon wraps to
/// it.** Before this the click made a growing box that knew nothing about the
/// bubble it landed in, and the surface tester watched a line run 22 px past
/// a 156 px bubble until she pressed Enter by hand.
///
/// CSP ties the two (manual, Text tool ▸ How to add ▸ *Auto detect where to
/// insert*: "If you enter text within a balloon or near selected text, you
/// can add text to an existing Text or Balloon layer") but still hands you a
/// click-sized box — CSP wraps at the BOX ("Text boxes are set to Wrap text
/// at frame by default"), never at the bubble. Wrapping the default box to
/// the bubble's inside is ours; a dragged box is still the letterer's own.
#[test]
fn a_text_clicked_inside_a_balloon_wraps_to_the_bubble() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    // The tester's bubble, sized so its INSIDE is the ledger's 156 px: an
    // ellipse only holds a box radii/√2 across, and the margin takes the
    // rest, so the drawn bubble has to be wider than the box it gives you.
    let bl = app.doc.add_balloon_layer(
        "bubbles",
        mn_core::BalloonSet {
            balloons: vec![bubble(400.0, 400.0, 133.0, 96.0)],
            border_px: 4.0,
            pressure_width: false,
        },
    );
    let sentence = "I told you never to come";
    let inner = click_and_type(&mut app, bl, [400.0, 400.0], sentence);
    let iw = inner[2] - inner[0];
    let drawn = 2.0 * 133.0;
    println!("[note] a {drawn:.0} px bubble, interior box {inner:?} — {iw:.1} px wide");
    assert!(
        (iw - 156.0).abs() < 4.0,
        "this is the ledger's 156 px bubble interior: {iw}"
    );
    assert!(iw < drawn, "and the box sits inside the drawn bubble: {iw}");

    let item = app.edited_item().expect("a box is being typed in").clone();
    assert_eq!(item.text, sentence, "the typing went into the new box");
    assert!(
        !item.auto_size,
        "a box that grows with the text is exactly the bug — it must WRAP"
    );
    assert!(
        (item.size[0] - iw).abs() < 0.5,
        "the box is the bubble's inside: {:?} vs {iw}",
        item.size
    );

    let dpi = app.doc_dpi();
    let engine = app.text_engine.as_ref().expect("a text engine");
    // The sentence on one line really is wider than the bubble…
    let mut flat = item.clone();
    flat.size = [4000.0, 4000.0];
    let natural = engine.natural_size(&flat, dpi).expect("measured");
    println!("[note] the sentence unwrapped is {:.1} px wide", natural[0]);
    assert!(
        natural[0] > 156.0,
        "this test proves nothing unless the sentence overflows: {natural:?}"
    );

    // …and inside the box it comes out on two lines, neither of them wider
    // than the bubble's inside. Measured, not assumed: each visual line is
    // re-measured on its own.
    let len = item.utf16_len();
    let mut lines: Vec<(u32, u32)> = Vec::new();
    let mut p = 0;
    while p < len {
        let (a, b) = engine.line_bounds(&item, dpi, p).expect("a visual line");
        lines.push((a, b));
        p = b.max(p + 1);
    }
    println!("[note] visual lines: {lines:?} of {len} units");
    assert_eq!(lines.len(), 2, "wrapped onto two lines: {lines:?}");
    for (a, b) in lines {
        let mut probe = item.clone();
        let (ba, bb) = (
            mn_core::text::utf16_to_byte(&item.text, a),
            mn_core::text::utf16_to_byte(&item.text, b),
        );
        probe.text = item.text[ba..bb].trim_end().to_string();
        probe.runs = vec![mn_core::text::StyleRun::plain(probe.utf16_len())];
        probe.size = [4000.0, 4000.0];
        let w = engine.natural_size(&probe, dpi).expect("measured")[0];
        println!("[note] line {:?} is {w:.1} px wide", probe.text);
        assert!(w <= iw + 1.0, "line {:?} ran {w:.1} px past {iw:.1}", probe.text);
    }
}

/// …and once it exists it is a box like any other. Dragged out of the bubble
/// with the Object tool it KEEPS the width it was given.
///
/// Owner call to overrule if he wants it otherwise: which balloon a text
/// belongs to is decided by geometry, never by a stored link — that is the
/// rule `carry_texts_with_balloon` and `fit_balloon_to_text` already follow —
/// so a box carried out of its bubble simply keeps its own shape. Nothing
/// re-wraps it, and nothing snaps it back.
#[test]
fn a_bound_box_dragged_out_of_the_bubble_keeps_its_wrap_width() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let bl = app.doc.add_balloon_layer(
        "bubbles",
        mn_core::BalloonSet {
            balloons: vec![bubble(400.0, 400.0, 78.0, 60.0)],
            border_px: 4.0,
            pressure_width: false,
        },
    );
    click_and_type(&mut app, bl, [400.0, 400.0], "I told you never to come back");
    app.commit_text_edit();
    let (li, before) = app
        .doc
        .layers
        .iter()
        .enumerate()
        .find_map(|(i, l)| l.texts().map(|ts| (i, ts.texts[0].clone())))
        .expect("the lettering landed on a text layer");

    app.tool = crate::cmd::Tool::Object;
    let c = before.center();
    assert!(app.text_object_press(c[0], c[1], 1), "the box took the press");
    app.finish_text_obj_drag(c[0] + 500.0, c[1] + 400.0);
    while let Some(cmd) = app.cmds.pop_front() {
        dispatch(&mut app, cmd);
    }

    let after = &app.doc.layers[li].texts().unwrap().texts[0];
    assert!(
        (after.pos[0] - before.pos[0] - 500.0).abs() < 1.0
            && (after.pos[1] - before.pos[1] - 400.0).abs() < 1.0,
        "it really left the bubble: {:?} -> {:?}",
        before.pos,
        after.pos
    );
    assert_eq!(after.size, before.size, "and kept the width it was given");
    assert!(!after.auto_size, "still a wrap box, not a growing one");
    assert_eq!(after.text, before.text);
}
