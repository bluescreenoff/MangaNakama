//! Row 156 — Smart Shape (`FG-020`–`024`): hold at the end of a wobbly
//! stroke and it becomes the circle you meant.
//!
//! The recognizer itself is tested in `mn_core::shape_fit` (synthetic wobbly
//! inputs, and the refusals). What is tested HERE is the gesture and the
//! swap: that the hold is what arms it, that releasing without holding
//! leaves the stroke exactly as drawn, that the wobble really comes off the
//! page when the figure goes down, and that the pair costs ONE undo press.
//!
//! These drive the real pointer entry points, so the state machine is what
//! is under test. The hold is wound forward by moving `still_since` back
//! (the `SpringLoad` idiom) — no test in here sleeps.

use super::{App, PointerKind, headless_renderer};
use crate::cmd::{FigureMode, Tool};
use mn_core::{PenSample, TileIdx};

/// An empty pointer batch — a report that moved the cursor but carried no
/// new pen samples. Used by the two tests that are about the HOLD state
/// machine rather than about ink, so they say nothing about the page.
const NONE: [PenSample; 0] = [];

fn app_with_smart() -> Option<App> {
    let renderer = headless_renderer()?;
    let mut app = App::new(renderer, (400, 400), 1.0);
    app.viewport = mn_gpu::Viewport::default();
    app.tool = Tool::Figure;
    app.figure_mode = FigureMode::Smart;
    // `FG-020` is preference-driven now, and `App::new` reads the MACHINE's
    // prefs.txt — so pin all three to the shipped defaults or this suite
    // passes or fails according to what the developer last set.
    app.prefs.smart_shape = true;
    app.prefs.smart_hold_ms = crate::app::SMART_HOLD_MS;
    app.prefs.smart_fit_tol = mn_core::shape_fit::FIT_TOL;
    // A thin nib: "inked here / clear there" must be about the PATH.
    app.props_current.size_px = 5.0;
    app.apply_props();
    Some(app)
}

fn px(app: &App, x: i32, y: i32) -> [u16; 4] {
    let idx = TileIdx::of_pixel(x, y);
    let (ox, oy) = idx.origin();
    app.doc
        .active_layer()
        .tile(idx)
        .map(|t| t.pixel((x - ox) as usize, (y - oy) as usize))
        .unwrap_or([0; 4])
}

/// Is there ink anywhere within `r` px of `(x, y)`? The composite assertions
/// are about a STROKE landing near a place, not about one exact pixel — a
/// 5 px nib on a tessellated circle does not promise pixel identity.
fn inked_near(app: &App, x: i32, y: i32, r: i32) -> bool {
    (-r..=r).any(|dy| (-r..=r).any(|dx| px(app, x + dx, y + dy)[3] > 0))
}

/// A deterministic hand wobble, index-derived — no RNG, so a failure here
/// replays as the same failure.
fn wobble(i: usize, salt: usize) -> f32 {
    let h = (i.wrapping_mul(2654435761).wrapping_add(salt.wrapping_mul(40503))) >> 8;
    ((h % 2000) as f32 / 1000.0) - 1.0
}

/// A hand-drawn circle about `(cx, cy)`, as screen points to drive.
fn circle_path(cx: f32, cy: f32, r: f32, n: usize, amp: f32) -> Vec<[f32; 2]> {
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32 * std::f32::consts::TAU;
            let rr = r + wobble(i, 3) * amp;
            [cx + rr * t.cos(), cy + rr * t.sin()]
        })
        .collect()
}

/// One pointer report. This sub tool inks LIVE, so unlike the drag-driven
/// figure tests the batch cannot be empty — with no samples the engine lays
/// no dabs and "the wobble is on the page" would be vacuously false.
fn one(p: [f32; 2], i: usize) -> [PenSample; 1] {
    [PenSample {
        x: p[0],
        y: p[1],
        pressure: 1.0,
        tilt_x: 0.0,
        tilt_y: 0.0,
        t_ms: i as f64 * 8.0,
    }]
}

/// Press, then travel. Leaves the button DOWN — the caller decides whether
/// to hold before releasing, which is the whole gesture.
fn draw(app: &mut App, pts: &[[f32; 2]]) {
    app.canvas_down(pts[0][0], pts[0][1], PointerKind::Pen, &one(pts[0], 0));
    for (i, p) in pts.iter().enumerate().skip(1) {
        app.canvas_move(p[0], p[1], &one(*p, i));
    }
}

fn release(app: &mut App, p: [f32; 2]) {
    app.canvas_up(p[0], p[1], &one(p, 9999));
}

/// Wind the live gesture's hold past its threshold, as if the pen had rested
/// at the end of the stroke.
///
/// It also clears the `FG-024` refusal, which is sampled from the PHYSICAL
/// keyboard's Shift state (`Shell::sync_modifiers` calls `GetKeyState`). Any
/// test that wants a recognition must not depend on whether a developer
/// happened to be holding Shift while the suite ran; the refusal itself gets
/// its own test below, which sets the flag deliberately.
fn hold(app: &mut App) {
    let ms = app.prefs.smart_hold_ms + 50;
    let g = app
        .smart_shape
        .as_mut()
        .expect("a smart shape gesture is live");
    g.refused = false;
    g.still_since = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_millis(ms))
        .expect("the process is older than the hold threshold");
}

/// The headline. A wobbly loop, a hold, and the release leaves a circle —
/// composite-asserted from the page: the clean rim is inked and the wobble
/// that used to stick out past it is GONE.
#[test]
fn holding_at_the_end_of_a_wobbly_loop_leaves_a_circle() {
    let Some(mut app) = app_with_smart() else {
        return;
    };
    let steps = app.doc.undo_labels().len();
    // r = 100 with a 9 px wobble: at the four compass points the drawn path
    // wanders well outside r = 104, which is what makes "the wobble is gone"
    // an assertion and not a tautology.
    let path = circle_path(200.0, 200.0, 100.0, 96, 9.0);
    draw(&mut app, &path);
    hold(&mut app);
    app.smart_shape_tick();

    // The hold arms a preview BEFORE the release — that is the feedback the
    // artist judges by, and it is the same path the swap will ink.
    let armed = app
        .smart_shape
        .as_ref()
        .and_then(|g| g.preview())
        .expect("the hold recognized a shape");
    assert_eq!(armed.kind, mn_core::shape_fit::ShapeKind::Circle);
    assert!(app.status.contains("circle"), "status: {}", app.status);

    release(&mut app, path[path.len() - 1]);

    assert!(app.smart_shape.is_none(), "the gesture is over");
    assert!(app.status.contains("circle"), "status: {}", app.status);
    // The clean rim is on the page, all the way round.
    for (dx, dy) in [(100, 0), (0, 100), (-100, 0), (0, -100)] {
        assert!(
            inked_near(&app, 200 + dx, 200 + dy, 4),
            "the recognized rim is missing at ({dx}, {dy})"
        );
    }
    // And the wobble is not: the drawn path reached past r = 108 in places,
    // the circle never does.
    for (dx, dy) in [(112, 0), (0, 112), (-112, 0), (0, -112)] {
        assert!(
            !inked_near(&app, 200 + dx, 200 + dy, 2),
            "freehand ink survived the swap at ({dx}, {dy})"
        );
    }
    assert_eq!(
        app.doc.undo_labels().len(),
        steps + 1,
        "one gesture, one undo press: {:?}",
        app.doc.undo_labels()
    );
}

/// The other half of the promise, and the one that protects every other
/// stroke in the app: release WITHOUT holding and nothing is recognized,
/// nothing is swapped, and the wobble stays exactly where the hand put it.
#[test]
fn releasing_without_holding_keeps_the_stroke_as_drawn() {
    let Some(mut app) = app_with_smart() else {
        return;
    };
    let steps = app.doc.undo_labels().len();
    let path = circle_path(200.0, 200.0, 100.0, 96, 9.0);
    draw(&mut app, &path);
    // No `hold()`: the pointer arrived and left.
    assert!(
        app.smart_shape.as_ref().is_some_and(|g| !g.ripe),
        "the hold has not matured"
    );
    release(&mut app, path[path.len() - 1]);

    assert!(app.smart_shape.is_none(), "the gesture is over either way");
    assert!(
        app.transform_drag.is_none(),
        "and no edit mode was entered"
    );
    // The freehand path is still on the page — including the parts a circle
    // would have cut off. Find a sample that wanders outside r = 105 and
    // assert the ink is there.
    let outer = path
        .iter()
        .find(|p| (p[0] - 200.0).hypot(p[1] - 200.0) > 105.0)
        .copied()
        .expect("the wobble reaches past the clean radius");
    assert!(
        inked_near(&app, outer[0] as i32, outer[1] as i32, 4),
        "the drawn wobble at {outer:?} must survive an un-held release"
    );
    assert_eq!(
        app.doc.undo_labels().len(),
        steps + 1,
        "still exactly one press — it was one ordinary stroke"
    );
}

/// A hold on something that is NOT a shape is a no-op with an honest status
/// line, not a forced snap. This is the wobbly-hatch-mark rule at the app
/// level: the recognizer said no and the app takes no for an answer.
#[test]
fn holding_on_a_scribble_changes_nothing() {
    let Some(mut app) = app_with_smart() else {
        return;
    };
    let steps = app.doc.undo_labels().len();
    let path: Vec<[f32; 2]> = (0..=240)
        .map(|i| {
            let t = i as f32 / 240.0;
            let a = t * std::f32::consts::TAU * 7.0;
            [110.0 + t * 170.0 + a.cos() * 35.0, 200.0 + a.sin() * 35.0]
        })
        .collect();
    draw(&mut app, &path);
    hold(&mut app);
    app.smart_shape_tick();

    assert!(
        app.smart_shape.as_ref().is_some_and(|g| g.ripe),
        "the hold matured"
    );
    assert!(
        app.smart_shape.as_ref().and_then(|g| g.preview()).is_none(),
        "but nothing was recognized"
    );
    assert!(
        app.status.contains("not a shape"),
        "it says so: {}",
        app.status
    );

    release(&mut app, path[path.len() - 1]);
    assert!(app.transform_drag.is_none(), "no edit mode for a scribble");
    assert_eq!(
        app.doc.undo_labels().len(),
        steps + 1,
        "one ordinary stroke, one press"
    );
    // The loops are still there.
    assert!(
        inked_near(&app, path[120][0] as i32, path[120][1] as i32, 4),
        "the scribble survives"
    );
}

/// The swap arms the Transform float over the new figure — `FG-022`/`FG-023`
/// as far as this round goes. The lift must be the SHAPE's box, not the
/// whole layer: art elsewhere on the page is not part of what you just drew.
#[test]
fn the_swap_arms_the_transform_float_over_the_new_shape() {
    let Some(mut app) = app_with_smart() else {
        return;
    };
    // Something else on the page, far from where the shape will go.
    app.tool = Tool::Pen;
    draw(&mut app, &[[20.0, 380.0], [30.0, 380.0], [40.0, 380.0]]);
    release(&mut app, [40.0, 380.0]);
    app.tool = Tool::Figure;

    let path = circle_path(200.0, 180.0, 80.0, 96, 6.0);
    draw(&mut app, &path);
    hold(&mut app);
    release(&mut app, path[path.len() - 1]);

    let d = app
        .transform_drag
        .as_ref()
        .expect("the swap armed the transform float");
    let xs: Vec<f32> = d.bbox.iter().map(|p| p[0]).collect();
    let ys: Vec<f32> = d.bbox.iter().map(|p| p[1]).collect();
    let (x0, x1) = (
        xs.iter().cloned().fold(f32::MAX, f32::min),
        xs.iter().cloned().fold(f32::MIN, f32::max),
    );
    let (y0, y1) = (
        ys.iter().cloned().fold(f32::MAX, f32::min),
        ys.iter().cloned().fold(f32::MIN, f32::max),
    );
    assert!(
        (x1 - x0 - 160.0).abs() < 30.0 && (y1 - y0 - 160.0).abs() < 30.0,
        "the lift is the circle's box, got {x0}..{x1} x {y0}..{y1}"
    );
    assert!(
        y1 < 300.0 && x0 > 100.0,
        "and it does not reach the earlier stroke in the corner: \
         {x0}..{x1} x {y0}..{y1}"
    );
    assert!(
        app.status.contains("Esc cancels"),
        "the status says how to leave it: {}",
        app.status
    );
}

/// The undo contract, asserted from the PAGE rather than from a counter:
/// one press after the swap and the page is blank again — no ghost of the
/// freehand stroke left behind by a half-undone pair.
#[test]
fn one_undo_press_takes_back_the_stroke_and_the_swap_together() {
    let Some(mut app) = app_with_smart() else {
        return;
    };
    let steps = app.doc.undo_labels().len();
    let path = circle_path(200.0, 200.0, 100.0, 96, 9.0);
    let outer = path
        .iter()
        .find(|p| (p[0] - 200.0).hypot(p[1] - 200.0) > 105.0)
        .copied()
        .expect("the wobble reaches past the clean radius");
    draw(&mut app, &path);
    hold(&mut app);
    release(&mut app, path[path.len() - 1]);
    assert_eq!(app.doc.undo_labels().len(), steps + 1);

    // The armed float is a placement, not history — Ctrl+Z means "take the
    // placement back" while one is live (the import-as-layer rule), so drop
    // it first and then press undo ONCE for the art itself.
    app.transform_drag = None;
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Undo);

    assert_eq!(
        app.doc.undo_labels().len(),
        steps,
        "one press, back to where we started: {:?}",
        app.doc.undo_labels()
    );
    assert!(
        !inked_near(&app, 300, 200, 6),
        "the recognized circle is gone"
    );
    assert!(
        !inked_near(&app, outer[0] as i32, outer[1] as i32, 6),
        "and so is the freehand stroke underneath it — a second press \
         must not be needed to clear a ghost"
    );
}

/// The swap presses Undo on the artist's behalf, which normally leaves a
/// REDO entry — and a redo that brought the wobble back on top of the clean
/// figure would be a silent, delayed mess. It cannot: inking the figure is a
/// history PUSH, and a push clears the redo branch. Pinned, because the day
/// somebody closes that op with `push_undo_keep_redo` this stops being true
/// and nothing else would notice.
#[test]
fn the_swap_leaves_nothing_to_redo() {
    let Some(mut app) = app_with_smart() else {
        return;
    };
    let path = circle_path(200.0, 200.0, 100.0, 96, 9.0);
    draw(&mut app, &path);
    hold(&mut app);
    release(&mut app, path[path.len() - 1]);
    assert!(app.transform_drag.is_some(), "the swap happened");
    assert_eq!(
        app.doc.redo_len(),
        0,
        "the undone freehand stroke is gone, not one Ctrl+Y away: {:?}",
        app.doc.redo_labels()
    );
}

/// A wobbly rectangle becomes a true one, corners included. Proves the app
/// path is not circle-only, and that a CLOSED recognition inks its closing
/// edge (the path the recognizer returns does not repeat its first point).
#[test]
fn a_held_wobbly_box_becomes_a_true_rectangle() {
    let Some(mut app) = app_with_smart() else {
        return;
    };
    let corners = [
        [80.0f32, 70.0],
        [320.0, 70.0],
        [320.0, 250.0],
        [80.0, 250.0],
        [80.0, 70.0],
    ];
    // Walk the outline with a wobble on.
    let mut path: Vec<[f32; 2]> = Vec::new();
    for w in corners.windows(2) {
        for k in 0..40 {
            let t = k as f32 / 40.0;
            let i = path.len();
            path.push([
                w[0][0] + (w[1][0] - w[0][0]) * t + wobble(i, 5) * 3.0,
                w[0][1] + (w[1][1] - w[0][1]) * t + wobble(i, 11) * 3.0,
            ]);
        }
    }
    path.push(corners[0]);
    draw(&mut app, &path);
    hold(&mut app);
    app.smart_shape_tick();
    assert_eq!(
        app.smart_shape
            .as_ref()
            .and_then(|g| g.preview())
            .map(|r| r.kind),
        Some(mn_core::shape_fit::ShapeKind::Rect)
    );
    release(&mut app, path[path.len() - 1]);

    // All four straight edges landed, the closing one included.
    assert!(inked_near(&app, 200, 70, 5), "top edge");
    assert!(inked_near(&app, 320, 160, 5), "right edge");
    assert!(inked_near(&app, 200, 250, 5), "bottom edge");
    assert!(inked_near(&app, 80, 160, 5), "left (closing) edge");
    // The middle stays clear — it inked an outline, not a fill.
    assert!(!inked_near(&app, 200, 160, 8), "the box is not filled");
}

/// A hold that found NOTHING disarms when the hand moves on, and the stroke
/// carries on as an ordinary stroke. This is the pause-to-think protection:
/// stopping halfway through a long stroke must not cost you the rest of it.
///
/// (`FG-021` deliberately takes the other case — a hold that DID find a
/// shape — and turns further motion into an adjustment of that shape. The
/// hold-time preference is the way out of that one.)
#[test]
fn moving_on_after_a_hold_that_found_nothing_disarms_the_recognition() {
    let Some(mut app) = app_with_smart() else {
        return;
    };
    let steps = app.doc.undo_labels().len();
    // A scribble: held, and refused.
    let path: Vec<[f32; 2]> = (0..=240)
        .map(|i| {
            let t = i as f32 / 240.0;
            let a = t * std::f32::consts::TAU * 7.0;
            [110.0 + t * 170.0 + a.cos() * 35.0, 200.0 + a.sin() * 35.0]
        })
        .collect();
    draw(&mut app, &path);
    hold(&mut app);
    app.smart_shape_tick();
    assert!(
        app.smart_shape.as_ref().is_some_and(|g| g.ripe),
        "the hold matured"
    );
    assert!(
        app.smart_shape.as_ref().is_some_and(|g| !g.armed()),
        "…on nothing, so there is no figure to adjust"
    );

    // The hand carries on — well past the slop.
    app.canvas_move(60.0, 60.0, &NONE);
    assert!(
        app.smart_shape.as_ref().is_some_and(|g| !g.ripe),
        "the hold has to be earned again"
    );
    assert!(
        app.smart_shape.as_ref().is_some_and(|g| g.adjust.is_none()),
        "and no adjustment was started"
    );

    release(&mut app, [60.0, 60.0]);
    assert!(app.transform_drag.is_none(), "no swap happened");
    assert_eq!(
        app.doc.undo_labels().len(),
        steps + 1,
        "one ordinary stroke"
    );
}

/// `FG-021`, the headline: "after the shape corrects itself, you can adjust
/// the size and angle of the shape by dragging". Keep the pen down, pull the
/// handle halfway in, and the circle that lands is the SMALL one — asserted
/// from the page, so both halves are real: the new rim is inked and the rim
/// the hold previewed is not.
#[test]
fn dragging_after_the_hold_sizes_the_shape_before_it_commits() {
    let Some(mut app) = app_with_smart() else {
        return;
    };
    let steps = app.doc.undo_labels().len();
    let path = circle_path(200.0, 200.0, 100.0, 96, 9.0);
    draw(&mut app, &path);
    hold(&mut app);
    app.smart_shape_tick();
    assert!(
        app.smart_shape.as_ref().is_some_and(|g| g.armed()),
        "the hold found a circle"
    );

    // The pen rests at the rim to the right; pull it halfway to the centre.
    app.canvas_move(250.0, 200.0, &NONE);
    let adj = app
        .smart_shape
        .as_ref()
        .and_then(|g| g.adjust.as_ref())
        .expect("the drag adjusts rather than disarming");
    assert_eq!(
        adj.shape.kind,
        mn_core::shape_fit::ShapeKind::Circle,
        "a drag never changes what it is"
    );
    assert!(
        app.status.contains("drag to size"),
        "the status says what the pen is doing now: {}",
        app.status
    );

    release(&mut app, [250.0, 200.0]);

    // The adjusted rim is on the page…
    assert!(
        inked_near(&app, 250, 200, 8),
        "the rim of the shrunken circle is missing"
    );
    // …the rim it would have had without the drag is not…
    assert!(
        !inked_near(&app, 300, 200, 4),
        "the un-adjusted rim was inked — the drag did not reach the commit"
    );
    // …and neither is the freehand wobble that used to be there.
    assert!(
        !inked_near(&app, 200, 108, 4),
        "the drawn stroke survived the swap at the top of the loop"
    );
    assert_eq!(
        app.doc.undo_labels().len(),
        steps + 1,
        "still one gesture, one undo press: {:?}",
        app.doc.undo_labels()
    );
}

/// `FG-021`'s Shift, at app level: it forces the regular form, and it is
/// applied AFTER the drag rather than before — an ellipse dragged and
/// Shift-held comes back a circle, not an ellipse that was briefly round.
#[test]
fn shift_during_the_drag_forces_the_regular_form() {
    let Some(mut app) = app_with_smart() else {
        return;
    };
    // A wide, flat loop: 150 × 60.
    let path: Vec<[f32; 2]> = (0..=140)
        .map(|i| {
            let t = i as f32 / 140.0 * std::f32::consts::TAU;
            [200.0 + 150.0 * t.cos(), 200.0 + 60.0 * t.sin()]
        })
        .collect();
    draw(&mut app, &path);
    hold(&mut app);
    app.smart_shape_tick();
    assert_eq!(
        app.smart_shape
            .as_ref()
            .and_then(|g| g.preview())
            .map(|r| r.kind),
        Some(mn_core::shape_fit::ShapeKind::Ellipse),
        "the hold found the oval that was drawn"
    );

    // Shift is read from the physical keyboard at the pointer events, so
    // the gesture is driven here with the flag passed in — the same rule
    // the `FG-024` refusal test uses.
    app.smart_shape_adjust(350.0, 200.0, true);
    let shaped = app
        .smart_shape
        .as_ref()
        .and_then(|g| g.preview())
        .expect("still armed");
    assert_eq!(
        shaped.kind,
        mn_core::shape_fit::ShapeKind::Circle,
        "Shift made the oval a perfect circle, and it says so"
    );
    let b = shaped.bbox();
    assert!(
        ((b[2] - b[0]) - (b[3] - b[1])).abs() < 2.0,
        "as wide as it is tall: {b:?}"
    );

    // Letting Shift go puts the oval back — nothing was destroyed by it.
    app.smart_shape_adjust(350.0, 200.0, false);
    assert_eq!(
        app.smart_shape
            .as_ref()
            .and_then(|g| g.preview())
            .map(|r| r.kind),
        Some(mn_core::shape_fit::ShapeKind::Ellipse),
        "Shift is a live modifier, not a one-way door"
    );
}

/// A drift under the slop is still a hold, so it must not start an
/// adjustment either — otherwise a pen resting on glass would slowly
/// shrink the shape it just found.
#[test]
fn a_tremor_under_the_slop_does_not_start_an_adjustment() {
    let Some(mut app) = app_with_smart() else {
        return;
    };
    let path = circle_path(200.0, 200.0, 100.0, 96, 9.0);
    draw(&mut app, &path);
    hold(&mut app);
    app.smart_shape_tick();
    let last = path[path.len() - 1];
    let drift = crate::app::SMART_HOLD_SLOP_PX * 0.35;
    app.canvas_move(last[0] + drift, last[1] + drift, &NONE);
    assert!(
        app.smart_shape.as_ref().is_some_and(|g| g.adjust.is_none()),
        "a tremor is not a drag"
    );
    assert!(
        app.smart_shape.as_ref().and_then(|g| g.preview()).is_some(),
        "and the preview is still the one the hold armed"
    );
}

/// `FG-020`, the hold-duration preference: the gesture waits for the number
/// in Preferences, not for the constant it defaults to.
#[test]
fn the_hold_duration_preference_is_what_the_gesture_waits_for() {
    let Some(mut app) = app_with_smart() else {
        return;
    };
    app.prefs.smart_hold_ms = 1000;
    let path = circle_path(200.0, 200.0, 100.0, 96, 9.0);
    draw(&mut app, &path);

    // Long enough for the DEFAULT hold, nowhere near the one that is set.
    if let Some(g) = app.smart_shape.as_mut() {
        g.refused = false;
        g.still_since = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_millis(crate::app::SMART_HOLD_MS + 50))
            .expect("the process is older than the hold threshold");
    }
    app.smart_shape_tick();
    assert!(
        app.smart_shape.as_ref().is_some_and(|g| !g.ripe),
        "300 ms is not 1000 ms — the preference is what counts"
    );

    hold(&mut app); // now past the preference
    app.smart_shape_tick();
    assert!(
        app.smart_shape.as_ref().and_then(|g| g.preview()).is_some(),
        "and past it, the shape arrives"
    );
}

/// `FG-020`'s switch. Off, the sub tool is a plain freehand pen: no gesture
/// is armed at all, so no hold can mature and no stroke can be swapped.
#[test]
fn turning_hold_to_create_figures_off_leaves_a_plain_freehand_stroke() {
    let Some(mut app) = app_with_smart() else {
        return;
    };
    app.prefs.smart_shape = false;
    let steps = app.doc.undo_labels().len();
    let path = circle_path(200.0, 200.0, 100.0, 96, 9.0);
    let outer = path
        .iter()
        .find(|p| (p[0] - 200.0).hypot(p[1] - 200.0) > 105.0)
        .copied()
        .expect("the wobble reaches past the clean radius");
    draw(&mut app, &path);
    assert!(
        app.smart_shape.is_none(),
        "nothing to hold: the gesture was never armed"
    );

    release(&mut app, path[path.len() - 1]);
    assert!(app.transform_drag.is_none(), "and nothing was swapped");
    assert!(
        inked_near(&app, outer[0] as i32, outer[1] as i32, 4),
        "the wobble the hand drew is the wobble that stays"
    );
    assert_eq!(
        app.doc.undo_labels().len(),
        steps + 1,
        "one ordinary stroke"
    );
}

/// A drift SMALLER than the slop is still a hold — a pen resting on glass is
/// never perfectly still, and a strict threshold would make the row feel
/// broken on real hardware.
#[test]
fn a_tremor_under_the_slop_still_counts_as_holding() {
    let Some(mut app) = app_with_smart() else {
        return;
    };
    let path = circle_path(200.0, 200.0, 100.0, 96, 9.0);
    draw(&mut app, &path);
    hold(&mut app);
    let last = path[path.len() - 1];
    // Half the slop, in both axes.
    let drift = crate::app::SMART_HOLD_SLOP_PX * 0.35;
    app.canvas_move(last[0] + drift, last[1] + drift, &NONE);
    app.smart_shape_tick();
    assert!(
        app.smart_shape.as_ref().and_then(|g| g.preview()).is_some(),
        "a tremor is not a move"
    );
}

/// `FG-024`: where the row refuses. A stroke whose shape was already decided
/// — Shift held, or a ruler snapping it — is never second-guessed, even
/// though it would fit a shape perfectly well.
#[test]
fn a_snapped_stroke_is_never_recognized() {
    let Some(mut app) = app_with_smart() else {
        return;
    };
    let steps = app.doc.undo_labels().len();
    let path = circle_path(200.0, 200.0, 100.0, 96, 9.0);
    draw(&mut app, &path);
    hold(&mut app);
    // `hold` clears the flag (it is keyboard-derived); set it deliberately.
    app.smart_shape.as_mut().expect("live").refused = true;
    app.smart_shape_tick();

    assert!(
        app.smart_shape.as_ref().and_then(|g| g.preview()).is_none(),
        "a snapped stroke is left alone"
    );
    assert!(
        app.status.contains("snapped"),
        "and says why: {}",
        app.status
    );
    release(&mut app, path[path.len() - 1]);
    assert!(app.transform_drag.is_none());
    assert_eq!(app.doc.undo_labels().len(), steps + 1, "one plain stroke");
}

/// A ruler on the page arms that refusal through the real predicate, so the
/// flag above is not the only thing keeping `FG-024`'s promise.
#[test]
fn a_live_ruler_arms_the_refusal_at_the_press() {
    let Some(mut app) = app_with_smart() else {
        return;
    };
    app.doc.rulers.items.push(mn_core::Ruler::Line {
        a: [40.0, 40.0],
        b: [360.0, 360.0],
    });
    app.doc.rulers.attach.push(None);
    app.doc.rulers.on = true;

    app.canvas_down(120.0, 120.0, PointerKind::Pen, &one([120.0, 120.0], 0));
    assert!(
        app.smart_shape.as_ref().is_some_and(|g| g.refused),
        "a live ruler already decided this stroke's shape"
    );
    release(&mut app, [120.0, 120.0]);
}

/// The registry four-place rule (docs/CODE-MAP.md): a row that is missing
/// from `SubTool::ALL` still draws fine but is invisible to Ctrl+K, to
/// `keys.json` and to the ui.txt memory — a silent half-landing.
#[test]
fn smart_shape_is_a_real_sub_tool_row() {
    use crate::cmd::SubTool;
    let sub = SubTool::Figure(FigureMode::Smart);
    assert!(
        SubTool::ALL.contains(&sub),
        "the row is in the enumeration Ctrl+K and keys.json read"
    );
    assert_eq!(
        crate::subtools::group_of(sub),
        crate::subtools::group::DIRECT_DRAW,
        "it inks the active layer, so it is a Direct draw row"
    );
    assert!(
        crate::subtools::rows(Tool::Figure, crate::subtools::group::DIRECT_DRAW).contains(&sub),
        "and the palette's tab lists it"
    );
    let Some(mut app) = app_with_smart() else {
        return;
    };
    app.figure_mode = FigureMode::Line;
    assert!(!crate::subtools::is_current(&app, sub));
    crate::subtools::apply_state(&mut app, sub);
    assert_eq!(app.figure_mode, FigureMode::Smart);
    assert!(
        crate::subtools::is_current(&app, sub),
        "is_current is the reverse of apply_state — miss this and the \
         shortcut cycle silently restarts from the top every press"
    );
}
