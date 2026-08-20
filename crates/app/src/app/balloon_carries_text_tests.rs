use crate::cmd::{AppCmd, dispatch};

/// A drawn square bubble around (100..300, 100..300).
fn drawn_bubble() -> mn_core::Balloon {
    mn_core::Balloon {
        shape: mn_core::BalloonShape::Polygon {
            points: vec![
                [100.0, 100.0],
                [300.0, 100.0],
                [300.0, 300.0],
                [100.0, 300.0],
            ],
            widths: vec![0.5; 4],
            corners: vec![true; 4],
        },
        ..Default::default()
    }
}

fn lettering(pos: [f32; 2], s: &str) -> mn_core::TextItem {
    let mut t = mn_core::TextItem::new(pos, "Gothic".into(), 9.0, [0, 0, 0], true);
    t.text = s.into();
    let n = t.utf16_len();
    t.runs = vec![mn_core::text::StyleRun::plain(n)];
    t.size = [40.0, 60.0];
    t.auto_size = false;
    t
}

#[test]
fn the_lollipop_carries_the_lettering_and_skips_hidden_layers() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let b = drawn_bubble();
    app.doc.add_balloon_layer(
        "bubbles",
        mn_core::BalloonSet {
            balloons: vec![b.clone()],
            border_px: 4.0,
            pressure_width: false,
        },
    );
    // Inside the bubble, on a visible layer…
    let live = app.doc.add_text_layer(
        "lettering",
        mn_core::TextSet {
            texts: vec![lettering([180.0, 170.0], "オイ")],
        },
    );
    // …the same text on a HIDDEN layer, which must be left alone.
    let hidden = app.doc.add_text_layer(
        "draft",
        mn_core::TextSet {
            texts: vec![lettering([180.0, 170.0], "draft")],
        },
    );
    app.doc.layers[hidden].visible = false;
    // …and an SFX well outside it.
    let far = app.doc.add_text_layer(
        "sfx",
        mn_core::TextSet {
            texts: vec![lettering([900.0, 700.0], "ドン")],
        },
    );

    let quarter = std::f32::consts::FRAC_PI_2;
    app.carry_texts_with_balloon(&b, [200.0, 200.0], quarter);
    while let Some(c) = app.cmds.pop_front() {
        dispatch(&mut app, c);
    }

    let moved = &app.doc.layers[live].texts().unwrap().texts[0];
    assert!(
        (moved.rotation - quarter).abs() < 1e-4,
        "the item carries the angle, not a rasterised copy of it"
    );
    assert_eq!(moved.text, "オイ", "the string survived");
    assert_eq!(moved.runs.len(), 1, "and its styling");
    assert!(moved.vertical, "and its column direction");
    assert!(
        app.status.contains("still editable"),
        "the two-step undo is announced: {}",
        app.status
    );

    for (li, why) in [(hidden, "hidden layer"), (far, "text outside the bubble")] {
        let t = &app.doc.layers[li].texts().unwrap().texts[0];
        assert_eq!(t.rotation, 0.0, "{why} was left where it was");
    }

    // …and undo puts the lettering back, because it was a real edit on a
    // real layer and not a paint.
    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(
        app.doc.layers[live].texts().unwrap().texts[0].rotation,
        0.0,
        "undo un-turns the lettering"
    );
}

/// An ellipse cannot be tilted, so the lollipop must not spin its
/// lettering inside a bubble that never moved.
#[test]
fn an_analytic_bubble_never_reports_a_rotation() {
    use crate::app::canvas_input::{BalloonDragMode, BalloonObjDrag};
    let mk = |shape: mn_core::BalloonShape| BalloonObjDrag {
        layer: 0,
        balloon: 0,
        mode: BalloonDragMode::BoxRotate,
        start: (300.0, 200.0),
        cur: (200.0, 300.0),
        orig: mn_core::Balloon {
            shape,
            ..Default::default()
        },
    };
    assert!(
        mk(mn_core::BalloonShape::Ellipse {
            center: [200.0, 200.0],
            radii: [100.0, 60.0],
        })
        .rotation()
        .is_none(),
        "an ellipse does not tilt, so nothing may follow it"
    );
    assert!(
        mk(mn_core::BalloonShape::RoundRect {
            rect: [100.0, 100.0, 300.0, 300.0],
            corner: 8.0,
        })
        .rotation()
        .is_none()
    );
    let (pivot, rad) = mk(drawn_bubble().shape)
        .rotation()
        .expect("a drawn bubble turns");
    assert!((pivot[0] - 200.0).abs() < 1e-3 && (pivot[1] - 200.0).abs() < 1e-3);
    assert!(
        (rad - std::f32::consts::FRAC_PI_2).abs() < 1e-3,
        "a quarter turn: {rad}"
    );
}
