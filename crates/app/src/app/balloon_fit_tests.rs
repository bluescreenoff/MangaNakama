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
    match &app.doc.layers[layer].balloons().expect("balloon layer").balloons[0].shape {
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
    assert!(after[0] < 300.0, "the hidden draft was not sized to: {after:?}");
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
