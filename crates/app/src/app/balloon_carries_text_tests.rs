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
        shift_snap: false,
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

/// Walk #2 (CSP manual, moving/rotating balloons): Shift constrains a
/// balloon MOVE to horizontal/vertical/45° and its ROTATION to 45°
/// increments from the original orientation; without Shift both follow
/// the pointer freely.
#[test]
fn balloon_moves_and_rotates_constrain_under_shift() {
    use crate::app::canvas_input::{BalloonDragMode, BalloonObjDrag};
    let ellipse = mn_core::BalloonShape::Ellipse {
        center: [100.0, 100.0],
        radii: [40.0, 25.0],
    };
    let mut d = BalloonObjDrag {
        layer: 0,
        balloon: 0,
        mode: BalloonDragMode::MoveWhole,
        start: (0.0, 0.0),
        cur: (10.0, 6.0),
        orig: mn_core::Balloon {
            shape: ellipse,
            ..Default::default()
        },
        shift_snap: true,
    };
    // 31° of drag snaps to the 45° octant at the drag's own length.
    let len = 10.0_f32.hypot(6.0);
    let oct = std::f32::consts::FRAC_PI_4;
    let moved = d.preview();
    let (cx, cy) = match moved.shape {
        mn_core::BalloonShape::Ellipse {
            center: [cx, cy], ..
        } => (cx, cy),
        _ => panic!("shape preserved"),
    };
    assert!(
        (cx - (100.0 + oct.cos() * len)).abs() < 1e-3
            && (cy - (100.0 + oct.sin() * len)).abs() < 1e-3,
        "the move snapped to the 45° octant ({cx}, {cy})"
    );
    d.shift_snap = false;
    let (cx, cy) = match d.preview().shape {
        mn_core::BalloonShape::Ellipse {
            center: [cx, cy], ..
        } => (cx, cy),
        _ => panic!("shape preserved"),
    };
    assert_eq!([cx, cy], [110.0, 106.0], "no Shift → the raw drag");

    // Rotation: 40° of lollipop drag quantizes to 45°, free without Shift.
    let drawn = drawn_bubble().shape;
    let c = [200.0, 200.0];
    let at = |deg: f32| {
        (
            c[0] + 100.0 * deg.to_radians().cos(),
            c[1] + 100.0 * deg.to_radians().sin(),
        )
    };
    let (start, cur) = (at(10.0), at(50.0));
    let mut r = BalloonObjDrag {
        layer: 0,
        balloon: 0,
        mode: BalloonDragMode::BoxRotate,
        start,
        cur,
        orig: mn_core::Balloon {
            shape: drawn.clone(),
            ..Default::default()
        },
        shift_snap: true,
    };
    let mut want = r.orig.clone();
    want.transform_around(c, 1.0, 1.0, std::f32::consts::FRAC_PI_4);
    let got = r.preview();
    assert!(
        got.shape == want.shape && got.tails == want.tails,
        "40° of drag → 45° of turn"
    );
    r.shift_snap = false;
    let a0 = (start.1 - c[1]).atan2(start.0 - c[0]);
    let a1 = (cur.1 - c[1]).atan2(cur.0 - c[0]);
    let mut free = r.orig.clone();
    free.transform_around(c, 1.0, 1.0, a1 - a0);
    let got = r.preview();
    assert!(
        got.shape == free.shape && got.tails == free.tails,
        "no Shift → the raw angle"
    );
}

/// Walk #2's open item, closed (CSP manual, moving balloons): MOVING a
/// bubble takes its lettering along through the real release path — the
/// exact committed delta, the same geometric pairing as the turn, and a
/// hidden layer's texts stay untouched. Old code left the text behind.
#[test]
fn moving_a_balloon_carries_its_lettering() {
    use crate::app::canvas_input::{BalloonDragMode, BalloonObjDrag};
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let b = drawn_bubble();
    let ob = b.bbox();
    let bl = app.doc.add_balloon_layer(
        "bubbles",
        mn_core::BalloonSet {
            balloons: vec![b.clone()],
            border_px: 4.0,
            pressure_width: false,
        },
    );
    let live = app.doc.add_text_layer(
        "lettering",
        mn_core::TextSet {
            texts: vec![lettering([180.0, 170.0], "オイ")],
        },
    );
    let hidden = app.doc.add_text_layer(
        "draft",
        mn_core::TextSet {
            texts: vec![lettering([180.0, 170.0], "draft")],
        },
    );
    app.doc.layers[hidden].visible = false;
    // Raw canvas coordinates: make to_canvas the identity (the same
    // reset the edge-drag tests use) or the release maps the drag
    // through a fitted viewport and the bubble lands elsewhere.
    app.viewport = mn_gpu::Viewport::default();

    app.balloon_obj_drag = Some(BalloonObjDrag {
        layer: bl,
        balloon: 0,
        mode: BalloonDragMode::MoveWhole,
        start: (200.0, 200.0),
        cur: (300.0, 260.0),
        orig: b,
        shift_snap: false,
    });
    app.canvas_up(300.0, 260.0, &[]);
    while let Some(c) = app.cmds.pop_front() {
        dispatch(&mut app, c);
    }

    let bb = app.doc.layers[bl].balloons().unwrap().balloons[0].bbox();
    assert_eq!(
        [bb[0] - ob[0], bb[1] - ob[1]],
        [100.0, 60.0],
        "the bubble moved by the drag"
    );
    let t = &app.doc.layers[live].texts().unwrap().texts[0];
    assert_eq!(
        t.pos,
        [280.0, 230.0],
        "the visible lettering came along, by the same delta"
    );
    let h = &app.doc.layers[hidden].texts().unwrap().texts[0];
    assert_eq!(h.pos, [180.0, 170.0], "hidden layers are left alone");
}
