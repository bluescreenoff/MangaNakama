//! Row 157 — the Figure tool's mid-draw gesture grammar (`FG-002`,
//! `FG-011`, `FG-012`).
//!
//! Three gestures that all share one property: the mark is not finished
//! when the button comes up. `FG-002`'s Curve drags a baseline and then
//! bends it; `FG-011` drags a shape's size and then spins it; `FG-012`
//! lets a multi-point figure give back its last point instead of being
//! thrown away whole. Each still costs exactly ONE undo press, because the
//! extra stages are gesture state and not history.
//!
//! These drive the real pointer entry points (`canvas_down/move/up`) rather
//! than the finishers, so the state machine's transitions are what is under
//! test and not just the geometry underneath it.

use super::{App, PointerKind, headless_renderer};
use crate::cmd::{FigureMode, FigureStage2Kind, Tool};
use mn_core::{PenSample, TileIdx};

const NONE: [PenSample; 0] = [];

fn app_with_figure(mode: FigureMode) -> Option<App> {
    let renderer = headless_renderer()?;
    let mut app = App::new(renderer, (400, 400), 1.0);
    app.viewport = mn_gpu::Viewport::default();
    app.tool = Tool::Figure;
    app.figure_mode = mode;
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

/// Stage one: press, drag, release — the size gesture everyone already has.
fn drag(app: &mut App, a: (f32, f32), b: (f32, f32)) {
    app.canvas_down(a.0, a.1, PointerKind::Pen, &NONE);
    app.canvas_move(b.0, b.1, &NONE);
    app.canvas_up(b.0, b.1, &NONE);
}

/// Stage two's commit: the click that says "there".
fn click(app: &mut App, p: (f32, f32)) {
    app.canvas_down(p.0, p.1, PointerKind::Pen, &NONE);
    app.canvas_up(p.0, p.1, &NONE);
}

/// `FG-002` end to end. A drag lays a straight baseline and inks NOTHING;
/// the release hands over to a second stage; the pointer is then a point ON
/// the curve, and the click that commits inks an arc that runs through it —
/// in one undo press.
#[test]
fn curve_tool_bends_its_baseline_through_the_second_stage_pointer() {
    let Some(mut app) = app_with_figure(FigureMode::Arc) else {
        return;
    };
    let steps = app.doc.undo_labels().len();

    drag(&mut app, (60.0, 300.0), (340.0, 300.0));

    // The release did NOT ink: this is the transition the row is about.
    let s = app.figure_stage2.expect("the release opened stage two");
    assert_eq!(s.kind, FigureStage2Kind::Bend);
    assert_eq!(s.a, (60.0, 300.0));
    assert_eq!(s.b, (340.0, 300.0));
    assert_eq!(
        s.cur,
        (200.0, 300.0),
        "seeded at the baseline's midpoint, so committing without moving \
         reproduces stage one"
    );
    assert!(app.figure_drag.is_none(), "stage one is over");
    assert_eq!(app.doc.undo_labels().len(), steps, "and nothing inked yet");
    assert_eq!(px(&app, 200, 300)[3], 0, "not even the baseline");

    // Aim well above the baseline and commit.
    app.figure_hover(200, 140);
    assert_eq!(
        app.figure_stage2.expect("still open").cur,
        (200.0, 140.0),
        "hover steers it with no button held"
    );
    click(&mut app, (200.0, 140.0));

    assert!(app.figure_stage2.is_none(), "the gesture is over");
    assert!(px(&app, 60, 300)[3] > 0, "inked from the dragged start");
    assert!(px(&app, 340, 300)[3] > 0, "to the dragged end");
    assert!(
        px(&app, 200, 140)[3] > 0,
        "and THROUGH the point the pointer aimed at"
    );
    // The discriminator: the straight baseline is empty. A one-stage line
    // tool would have inked exactly there.
    assert_eq!(
        px(&app, 200, 300)[3],
        0,
        "the baseline is not what got inked"
    );
    assert_eq!(
        app.doc.undo_labels().len(),
        steps + 1,
        "one curve, one undo press: {:?}",
        app.doc.undo_labels()
    );
    assert!(app.status.contains("curve inked"), "status: {}", app.status);
}

/// The no-op path, and the reason `cur` is seeded rather than left at the
/// pointer: release and click straight away, and the Curve inks the same
/// straight line the Straight-line tool would have.
#[test]
fn committing_a_curve_without_bending_it_inks_the_baseline() {
    let Some(mut app) = app_with_figure(FigureMode::Arc) else {
        return;
    };
    drag(&mut app, (60.0, 200.0), (340.0, 200.0));
    let s = app.figure_stage2.expect("stage two");
    for p in app.figure_stage2_path(&s) {
        assert!(
            (p[1] - 200.0).abs() < 0.01,
            "an unbent curve is the baseline, got {p:?}"
        );
    }
}

/// `FG-011` "Adjust angle after fixed" — off by default (the drag inks on
/// release exactly as it always did), and when on it turns every dragged
/// shape into a rotatable one for the cost of one more click.
#[test]
fn adjust_angle_after_fixed_is_opt_in_and_spins_the_fixed_shape() {
    let Some(mut app) = app_with_figure(FigureMode::Rect) else {
        return;
    };
    let steps = app.doc.undo_labels().len();

    // Off: the release inks, no second stage. This is the regression guard —
    // the row must not change what the shape tools already do.
    assert!(!app.figure_adjust_angle, "opt-in, like CSP's toggle");
    drag(&mut app, (100.0, 150.0), (300.0, 250.0));
    assert!(app.figure_stage2.is_none(), "no second stage when off");
    assert_eq!(app.doc.undo_labels().len(), steps + 1, "it inked on release");

    // On: the same drag freezes the size and waits. On a FRESH layer, so
    // the "clear here" assertions below are about the turn and not about
    // what the opt-in half already inked.
    app.doc.active = app.doc.add_layer("spin");
    app.figure_adjust_angle = true;
    let steps = app.doc.undo_labels().len();
    drag(&mut app, (100.0, 150.0), (300.0, 250.0));
    let s = app.figure_stage2.expect("the release opened stage two");
    assert_eq!(s.kind, FigureStage2Kind::Angle);
    assert_eq!(s.cur, s.b, "seeded at the dragged corner — zero turn");
    assert_eq!(s.angle(), 0.0, "so an unmoved commit inks it unrotated");
    assert_eq!(app.doc.undo_labels().len(), steps, "nothing inked yet");

    // Aim a quarter turn round the centre (200, 200): the corner (300, 250)
    // is (100, 50) from it, and (-50, 100) is that vector turned 90°.
    app.figure_hover(150, 300);
    let s = app.figure_stage2.expect("still open");
    assert!(
        (s.angle() - std::f32::consts::FRAC_PI_2).abs() < 1e-4,
        "a quarter turn, got {}",
        s.angle()
    );
    // The path is the rectangle's four corners, each turned about the
    // centre — a 100x200 upright box where stage one drew a 200x100 one.
    let path = app.figure_stage2_path(&s);
    let corner = |want: [f32; 2]| {
        assert!(
            path.iter()
                .any(|p| (p[0] - want[0]).hypot(p[1] - want[1]) < 0.01),
            "turned corner {want:?} missing from {path:?}"
        );
    };
    corner([250.0, 100.0]);
    corner([250.0, 300.0]);
    corner([150.0, 300.0]);
    corner([150.0, 100.0]);

    click(&mut app, (150.0, 300.0));
    assert!(app.figure_stage2.is_none(), "the gesture is over");
    assert!(px(&app, 250, 200)[3] > 0, "the turned right edge is inked");
    assert!(px(&app, 200, 100)[3] > 0, "and the turned top edge");
    assert_eq!(
        px(&app, 100, 200)[3],
        0,
        "while the UNturned rectangle's left edge is clear — it really \
         rotated rather than inking what stage one showed"
    );
    assert_eq!(
        app.doc.undo_labels().len(),
        steps + 1,
        "one rectangle, one undo press: {:?}",
        app.doc.undo_labels()
    );
}

/// Esc during a second stage throws the whole figure away — the size drag
/// included — and spends no undo press doing it, because nothing was ever
/// committed. `cancel_figure_stage2` is the arm `main.rs` binds Esc to.
#[test]
fn esc_during_the_second_stage_leaves_no_ink_and_no_undo_step() {
    let Some(mut app) = app_with_figure(FigureMode::Arc) else {
        return;
    };
    let steps = app.doc.undo_labels().len();
    drag(&mut app, (60.0, 300.0), (340.0, 300.0));
    app.figure_hover(200, 140);
    assert!(app.figure_stage2.is_some());

    app.cancel_figure_stage2();

    assert!(app.figure_stage2.is_none(), "aborted clean");
    assert!(app.figure_drag.is_none(), "and stage one is not left behind");
    assert_eq!(app.doc.undo_labels().len(), steps, "no undo press spent");
    assert_eq!(px(&app, 200, 140)[3], 0, "nothing inked");
    assert_eq!(px(&app, 200, 300)[3], 0, "not the baseline either");
    // A second Esc is inert rather than an error.
    app.cancel_figure_stage2();
    assert!(app.figure_stage2.is_none());
}

/// A TAP is not a drag: it must not strand the user in a second stage with
/// nothing on screen to aim at. It falls through to the old advice.
#[test]
fn a_tap_never_opens_a_second_stage() {
    let Some(mut app) = app_with_figure(FigureMode::Arc) else {
        return;
    };
    let steps = app.doc.undo_labels().len();
    drag(&mut app, (200.0, 200.0), (200.5, 200.5));
    assert!(app.figure_stage2.is_none(), "no stage two on a tap");
    assert_eq!(app.doc.undo_labels().len(), steps, "and nothing inked");
    assert!(app.status.contains("drag the shape out"), "{}", app.status);
}

/// `FG-012`, the row people miss most: Backspace during a multi-point
/// figure gives back the LAST point and leaves the figure alive, so one bad
/// vertex does not cost a twelve-click polygon. At the first point there is
/// nothing left to keep, so it ends the gesture rather than being a dead
/// key — the magnetic lasso's rule.
#[test]
fn backspace_takes_back_one_point_without_ending_the_figure() {
    let Some(mut app) = app_with_figure(FigureMode::Polygon) else {
        return;
    };
    let steps = app.doc.undo_labels().len();
    for p in [(60.0, 60.0), (300.0, 60.0), (300.0, 300.0), (240.0, 200.0)] {
        click(&mut app, p);
    }
    assert_eq!(app.figure_poly.as_ref().map(Vec::len), Some(4));

    app.figure_undo_point();

    assert_eq!(
        app.figure_poly.as_ref().map(Vec::len),
        Some(3),
        "one point back, figure still alive"
    );
    assert_eq!(
        app.figure_poly.as_ref().expect("alive").last().copied(),
        Some((300.0, 300.0)),
        "and it is the LAST one that went"
    );
    assert!(app.status.contains("3 left"), "status: {}", app.status);
    assert_eq!(app.doc.undo_labels().len(), steps, "no history was touched");

    // Placing continues from where the walk-back left off, and the commit
    // inks the REDUCED list — the taken-back vertex is not in the shape.
    click(&mut app, (60.0, 300.0));
    app.finish_figure_poly();
    assert!(app.figure_poly.is_none(), "the gesture is over");
    assert!(px(&app, 60, 180)[3] > 0, "the closing edge is inked");
    assert_eq!(
        px(&app, 240, 200)[3],
        0,
        "the point Backspace took back is not on the shape"
    );
    assert_eq!(app.doc.undo_labels().len(), steps + 1, "one undo press");
}

/// The same key on the Continuous curve, walked all the way down: the last
/// press ends the gesture instead of leaving an empty point list behind
/// (which would keep the overlay alive with nothing in it).
#[test]
fn backspace_past_the_first_point_ends_the_figure_cleanly() {
    let Some(mut app) = app_with_figure(FigureMode::Curve) else {
        return;
    };
    let steps = app.doc.undo_labels().len();
    for p in [(60.0, 300.0), (200.0, 120.0), (340.0, 300.0)] {
        click(&mut app, p);
    }
    assert_eq!(app.figure_poly.as_ref().map(Vec::len), Some(3));

    app.figure_undo_point();
    app.figure_undo_point();
    assert_eq!(app.figure_poly.as_ref().map(Vec::len), Some(1));
    app.figure_undo_point();

    assert!(app.figure_poly.is_none(), "the gesture ended, not stalled");
    assert!(app.status.contains("cancelled"), "status: {}", app.status);
    assert_eq!(app.doc.undo_labels().len(), steps, "and inked nothing");
    // Inert once there is no figure — the key is never an error.
    app.figure_undo_point();
    assert!(app.figure_poly.is_none());
}
