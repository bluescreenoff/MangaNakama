//! Rows 125/`FI-050` and `FI-051` — the freeform gradient's multi-stroke
//! gesture.
//!
//! The core geometry and the paint ops are pinned in `mn_core::freeform` and
//! `mn_core::freeform_paint_tests`. What is under test HERE is the state
//! machine on top of them: strokes, drawn with the real pointer entry
//! points, that put no pixels down until the gesture is COMMITTED — and that
//! a tool switch, a sub tool switch or Esc in the gaps between them throws
//! away with nothing to undo.
//!
//! The `FigureStage2` suite next door is the same shape for the same
//! reason: a gesture that outlives its button is the thing that breaks.
//!
//! **`FI-051` changed the ending.** Until this round the second stroke's
//! release painted; now two lines are only ENOUGH, and Enter (or a click
//! away from them) is what paints, because a third line has to be able to
//! arrive first. Backspace walks the lines back, `FG-012`'s rule.

use super::{App, PointerKind, headless_renderer};
use crate::cmd::{AppCmd, GradMode, SubTool, Tool, dispatch};
use mn_core::TileIdx;

const NONE: [mn_core::PenSample; 0] = [];

fn app_with_freeform() -> Option<App> {
    let renderer = headless_renderer()?;
    let mut app = App::new(renderer, (400, 400), 1.0);
    app.viewport = mn_gpu::Viewport::default();
    dispatch(&mut app, AppCmd::SetTool(Tool::Gradient));
    app.grad_mode = GradMode::Freeform;
    // Known ends, so the colour assertions do not depend on whatever the
    // palette was carrying when the test exe started.
    app.main_color = [1.0, 0.0, 0.0];
    app.sub_color = [0.0, 0.0, 1.0];
    Some(app)
}

/// One guide stroke: press, a few moves, release.
fn stroke(app: &mut App, pts: &[(f32, f32)]) {
    let (first, rest) = pts.split_first().expect("a stroke needs a start");
    app.canvas_down(first.0, first.1, PointerKind::Pen, &NONE);
    for p in rest {
        app.canvas_move(p.0, p.1, &NONE);
    }
    let last = pts.last().unwrap();
    app.canvas_up(last.0, last.1, &NONE);
}

/// Straight RGBA at a canvas pixel of the active layer.
fn px(app: &App, x: i32, y: i32) -> [f32; 4] {
    let idx = TileIdx::of_pixel(x, y);
    let (ox, oy) = idx.origin();
    let raw = app
        .doc
        .active_layer()
        .tile(idx)
        .map(|t| t.pixel((x - ox) as usize, (y - oy) as usize))
        .unwrap_or([0; 4]);
    let a = raw[3] as f32 / mn_core::FIX15_ONE as f32;
    let un = |v: u16| {
        if a > 0.0 {
            (v as f32 / mn_core::FIX15_ONE as f32 / a).clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    [un(raw[0]), un(raw[1]), un(raw[2]), a]
}

/// End to end: each stroke banks a guide and inks NOTHING, Enter applies the
/// gradient, each guide wears its own palette colour, and the whole thing is
/// one undo press.
#[test]
fn two_strokes_lay_a_gradient_between_the_guides_in_one_undo_press() {
    let Some(mut app) = app_with_freeform() else {
        return;
    };
    let steps = app.doc.undo_labels().len();

    stroke(&mut app, &[(80.0, 20.0), (80.0, 200.0), (80.0, 380.0)]);

    // The transition this row is about: the release banked a guide and put
    // no pixels anywhere.
    let g = app.grad_free.as_ref().expect("the first guide is banked");
    assert_eq!(g.done.len(), 1, "guide 1 is recorded");
    assert!(!g.drawing, "and the stroke is over");
    assert!(g.cur.is_empty(), "with nothing left half-drawn");
    assert_eq!(app.doc.undo_labels().len(), steps, "nothing inked yet");
    assert_eq!(px(&app, 80, 200)[3], 0.0, "not even the guide itself");
    assert!(app.grad_drag.is_none(), "the one-drag path stayed out of it");

    stroke(&mut app, &[(320.0, 20.0), (320.0, 200.0), (320.0, 380.0)]);

    // `FI-051`: two lines is ENOUGH, not DONE — a third could still arrive,
    // so nothing is painted until the commit.
    assert!(
        app.grad_free.as_ref().is_some_and(|g| g.ready()),
        "two lines are banked and the gesture is still open"
    );
    assert_eq!(app.doc.undo_labels().len(), steps, "still nothing inked");
    app.commit_grad_free();

    assert!(app.grad_free.is_none(), "the gesture is spent");
    assert_eq!(
        app.doc.undo_labels().len(),
        steps + 1,
        "the whole apply is ONE undo press"
    );

    // Guide 1 got the main colour, guide 2 the sub colour, the middle mixes.
    let g1 = px(&app, 80, 200);
    assert!(g1[0] > 0.99 && g1[2] < 0.01, "guide 1 is main (red): {g1:?}");
    let g2 = px(&app, 320, 200);
    assert!(g2[2] > 0.99 && g2[0] < 0.01, "guide 2 is sub (blue): {g2:?}");
    let mid = px(&app, 200, 200);
    assert!(
        (mid[0] - 0.5).abs() < 0.05 && (mid[2] - 0.5).abs() < 0.05,
        "the midline mixes them: {mid:?}"
    );

    assert!(app.doc.undo(), "and one press takes the page back");
    assert_eq!(px(&app, 200, 200)[3], 0.0);
}

/// A curved guide bends the gradient on the CANVAS, through the real
/// gesture — not just in the geometry module.
#[test]
fn a_curved_guide_bends_the_painted_result() {
    let Some(mut app) = app_with_freeform() else {
        return;
    };
    // Guide 1 bulges out to x=240 level with y=200.
    stroke(
        &mut app,
        &[(80.0, 20.0), (80.0, 140.0), (240.0, 200.0), (80.0, 260.0), (80.0, 380.0)],
    );
    stroke(&mut app, &[(320.0, 20.0), (320.0, 380.0)]);
    app.commit_grad_free();

    // The tip of the bulge wears guide 1's colour, way out in open canvas
    // where a straight guide would have left almost pure sub colour.
    let tip = px(&app, 240, 200);
    assert!(
        tip[0] > 0.99 && tip[2] < 0.01,
        "the drawn shape carries the colour with it: {tip:?}"
    );
    // Level with the bulge is much redder than the same x well above it.
    let near = px(&app, 280, 200)[0];
    let away = px(&app, 280, 30)[0];
    assert!(
        near > away + 0.2,
        "the bend is real and local: {near} level with it, {away} above"
    );
}

/// The gap between the two strokes is the dangerous state, so every way out
/// of it is pinned: Esc, a tool switch, and a sub tool switch each drop the
/// gesture, leave the canvas clean, and bank no undo step.
#[test]
fn every_exit_between_the_strokes_cancels_cleanly() {
    // 1. Esc (the key table calls `cancel_grad_free`).
    let Some(mut app) = app_with_freeform() else {
        return;
    };
    let steps = app.doc.undo_labels().len();
    stroke(&mut app, &[(80.0, 20.0), (80.0, 380.0)]);
    assert!(app.grad_free.is_some());
    app.cancel_grad_free();
    assert!(app.grad_free.is_none(), "Esc drops the gesture");
    assert_eq!(app.doc.undo_labels().len(), steps, "and banks no history");
    assert_eq!(px(&app, 200, 200)[3], 0.0, "canvas untouched");
    // The next stroke is a FIRST guide again, not a stranded second one.
    stroke(&mut app, &[(120.0, 20.0), (120.0, 380.0)]);
    assert_eq!(app.doc.undo_labels().len(), steps, "still nothing painted");
    assert!(
        app.grad_free.as_ref().is_some_and(|g| !g.done.is_empty()),
        "it started a fresh gesture"
    );

    // 2. A tool switch.
    let Some(mut app) = app_with_freeform() else {
        return;
    };
    let steps = app.doc.undo_labels().len();
    stroke(&mut app, &[(80.0, 20.0), (80.0, 380.0)]);
    dispatch(&mut app, AppCmd::SetTool(Tool::Pen));
    assert!(app.grad_free.is_none(), "a tool switch drops it");
    dispatch(&mut app, AppCmd::SetTool(Tool::Gradient));
    assert_eq!(app.doc.undo_labels().len(), steps);
    assert_eq!(px(&app, 200, 200)[3], 0.0);

    // 3. A SUB tool switch — the same row's neighbour, which would otherwise
    //    have inherited a guide line it has no use for.
    let Some(mut app) = app_with_freeform() else {
        return;
    };
    stroke(&mut app, &[(80.0, 20.0), (80.0, 380.0)]);
    crate::subtools::apply_state(&mut app, SubTool::Gradient(GradMode::FgToBg));
    assert!(app.grad_free.is_none(), "a sub tool switch drops it too");
    assert_eq!(app.grad_mode, GradMode::FgToBg);

    // 4. The `,`/`.` mode cycle, the fourth way to leave the row.
    let Some(mut app) = app_with_freeform() else {
        return;
    };
    stroke(&mut app, &[(80.0, 20.0), (80.0, 380.0)]);
    app.step_subtool(true);
    assert!(app.grad_free.is_none(), "the mode cycle drops it as well");
}

/// A tap is not a guide — while there is not yet a gradient to paint. A
/// stray click must not commit a page-wide radial ramp, and must not close
/// the stage it landed in either. (Once two lines ARE down a tap means
/// something else entirely; `a_click_away_from_the_lines_commits` has that.)
#[test]
fn a_tap_is_refused_without_losing_the_gesture() {
    let Some(mut app) = app_with_freeform() else {
        return;
    };
    let steps = app.doc.undo_labels().len();

    // A tap with no gesture open starts nothing.
    stroke(&mut app, &[(200.0, 200.0)]);
    assert!(app.grad_free.is_none(), "a tap does not open a gesture");
    assert_eq!(app.doc.undo_labels().len(), steps);

    // A real first guide, then a tap: the gesture SURVIVES, still waiting.
    stroke(&mut app, &[(80.0, 20.0), (80.0, 380.0)]);
    stroke(&mut app, &[(200.0, 200.0)]);
    assert!(
        app.grad_free.as_ref().is_some_and(|g| !g.done.is_empty()),
        "the banked guide survives a stray click"
    );
    assert_eq!(app.doc.undo_labels().len(), steps, "and nothing was painted");

    // The real second guide still lands — and, being the second, it makes
    // the gesture ready rather than painting on its own.
    stroke(&mut app, &[(320.0, 20.0), (320.0, 380.0)]);
    assert_eq!(app.doc.undo_labels().len(), steps, "two lines, no paint yet");
    app.commit_grad_free();
    assert_eq!(app.doc.undo_labels().len(), steps + 1);
}

/// Selection-else-layer, driven through the gesture rather than the core op.
#[test]
fn the_gesture_honours_a_selection() {
    let Some(mut app) = app_with_freeform() else {
        return;
    };
    app.doc.selection = Some(mn_core::Selection::from_rect(
        &app.doc, 0.0, 0.0, 200.0, 400.0,
    ));
    stroke(&mut app, &[(80.0, 20.0), (80.0, 380.0)]);
    stroke(&mut app, &[(320.0, 20.0), (320.0, 380.0)]);
    app.commit_grad_free();
    assert!(px(&app, 100, 200)[3] > 0.99, "inside the selection: painted");
    assert_eq!(px(&app, 300, 200)[3], 0.0, "outside: untouched");
}

/// The CODE-MAP four-place rule for a new sub tool row: `SubTool::ALL`,
/// `group_of`, `apply_state` and `is_current`. Miss `ALL` and the row is
/// invisible to Ctrl+K and to `keys.json`; miss `is_current` and the
/// shortcut cycle silently restarts from the top every press.
#[test]
fn the_freeform_row_is_wired_in_all_four_places() {
    let sub = SubTool::Gradient(GradMode::Freeform);
    // 1. Enumerated.
    assert!(
        SubTool::ALL.contains(&sub),
        "the row must be in SubTool::ALL or Ctrl+K cannot see it"
    );
    // 2. Filed under a group, with the rest of the gradient rows.
    assert_eq!(
        crate::subtools::group_of(sub),
        crate::subtools::group::GRADIENT
    );
    assert_eq!(sub.tool(), Tool::Gradient);
    assert!(!sub.label().is_empty());

    let Some(mut app) = app_with_freeform() else {
        return;
    };
    // 3. `apply_state` puts it into the app, and 4. `is_current` reads it
    //    back — for EVERY gradient row, so the pair stays each other's
    //    inverse and the shortcut cycle can find where it is standing.
    for m in [
        GradMode::FgToBg,
        GradMode::FgToTransparent,
        GradMode::TransparentToFg,
        GradMode::Freeform,
    ] {
        let row = SubTool::Gradient(m);
        crate::subtools::apply_state(&mut app, row);
        assert!(
            crate::subtools::is_current(&app, row),
            "{m:?} does not report that you are standing on it"
        );
        for other in [GradMode::FgToBg, GradMode::Freeform] {
            if other != m {
                assert!(
                    !crate::subtools::is_current(&app, SubTool::Gradient(other)),
                    "{other:?} lit up while standing on {m:?}"
                );
            }
        }
    }
    // The mode cycle reaches Freeform rather than skipping it.
    app.grad_mode = GradMode::FgToBg;
    let mut seen = false;
    for _ in 0..4 {
        app.step_subtool(true);
        seen |= app.grad_mode == GradMode::Freeform;
    }
    assert!(seen, "`,`/`.` must be able to reach the new row");
}

// --- `FI-051`: a third line and up ----------------------------------------

/// THE ROW. A third drag adds a third guide instead of being refused, that
/// guide carries the MAIN colour as it stands when it is drawn, and the
/// commit lays all three down in one undo press.
#[test]
fn a_third_line_adds_a_third_colour_and_commits_in_one_press() {
    let Some(mut app) = app_with_freeform() else {
        return;
    };
    let steps = app.doc.undo_labels().len();

    stroke(&mut app, &[(40.0, 20.0), (40.0, 380.0)]); // main: red
    stroke(&mut app, &[(200.0, 20.0), (200.0, 380.0)]); // sub: blue
    // The artist picks a new colour BEFORE the third line, which is the
    // whole point of recording the colour at draw time.
    app.main_color = [0.0, 1.0, 0.0];
    stroke(&mut app, &[(360.0, 20.0), (360.0, 380.0)]);

    let g = app.grad_free.as_ref().expect("still open");
    assert_eq!(g.done.len(), 3, "three lines banked");
    assert_eq!(g.done[0].colour, [1.0, 0.0, 0.0, 1.0], "line 1 = main");
    assert_eq!(g.done[1].colour, [0.0, 0.0, 1.0, 1.0], "line 2 = sub");
    assert_eq!(
        g.done[2].colour,
        [0.0, 1.0, 0.0, 1.0],
        "line 3 = the main colour AS IT WAS when the line was drawn"
    );
    assert_eq!(app.doc.undo_labels().len(), steps, "nothing painted yet");

    app.commit_grad_free();
    assert!(app.grad_free.is_none(), "the gesture is spent");
    assert_eq!(app.doc.undo_labels().len(), steps + 1, "ONE undo press");

    // Each line wears its own colour on the canvas.
    let one = px(&app, 40, 200);
    assert!(one[0] > 0.98 && one[1] < 0.02, "line 1 is red: {one:?}");
    let two = px(&app, 200, 200);
    assert!(two[2] > 0.98 && two[1] < 0.02, "line 2 is blue: {two:?}");
    let three = px(&app, 360, 200);
    assert!(three[1] > 0.98 && three[0] < 0.02, "line 3 is green: {three:?}");

    assert!(app.doc.undo(), "and one press takes the whole field back");
    assert_eq!(px(&app, 200, 200)[3], 0.0);
}

/// The other commit affordance: once there are lines to paint, a CLICK away
/// from them means "done" rather than "that was not a line". Same gesture
/// grammar as the figure tool's stage two.
#[test]
fn a_click_away_from_the_lines_commits() {
    let Some(mut app) = app_with_freeform() else {
        return;
    };
    let steps = app.doc.undo_labels().len();
    stroke(&mut app, &[(40.0, 20.0), (40.0, 380.0)]);
    stroke(&mut app, &[(200.0, 20.0), (200.0, 380.0)]);
    // A tap, which with one line down would have been refused.
    stroke(&mut app, &[(300.0, 300.0)]);
    assert!(app.grad_free.is_none(), "the click committed the gesture");
    assert_eq!(app.doc.undo_labels().len(), steps + 1);
    assert!(px(&app, 120, 200)[3] > 0.99, "and it really painted");
}

/// `FG-012`'s rule, borrowed: Backspace takes the last line back rather
/// than throwing the gesture away — and at the last one it cancels, so the
/// key is never dead. Nothing it does is history.
#[test]
fn backspace_walks_the_lines_back_and_then_cancels() {
    let Some(mut app) = app_with_freeform() else {
        return;
    };
    let steps = app.doc.undo_labels().len();
    stroke(&mut app, &[(40.0, 20.0), (40.0, 380.0)]);
    stroke(&mut app, &[(200.0, 20.0), (200.0, 380.0)]);
    stroke(&mut app, &[(360.0, 20.0), (360.0, 380.0)]);

    app.grad_free_undo_guide();
    assert_eq!(
        app.grad_free.as_ref().map(|g| g.done.len()),
        Some(2),
        "one line came off, the rest stayed"
    );
    app.grad_free_undo_guide();
    assert_eq!(app.grad_free.as_ref().map(|g| g.done.len()), Some(1));
    app.grad_free_undo_guide();
    assert!(app.grad_free.is_none(), "the last one cancels the gesture");
    assert_eq!(app.doc.undo_labels().len(), steps, "none of it was history");
    assert_eq!(px(&app, 200, 200)[3], 0.0, "and the canvas is clean");

    // The line Backspace took back is not in the painted field: draw three,
    // drop the third, and the far side is the SECOND line's colour.
    let Some(mut app) = app_with_freeform() else {
        return;
    };
    stroke(&mut app, &[(40.0, 20.0), (40.0, 380.0)]);
    stroke(&mut app, &[(200.0, 20.0), (200.0, 380.0)]);
    app.main_color = [0.0, 1.0, 0.0];
    stroke(&mut app, &[(360.0, 20.0), (360.0, 380.0)]);
    app.grad_free_undo_guide();
    app.commit_grad_free();
    // Out past line 2, where the dropped line was: no green at all, and the
    // two-line ramp's own drift back toward the middle (documented in
    // `mn_core::freeform`) rather than the three-line field.
    let far = px(&app, 380, 200);
    assert!(far[1] < 0.02, "the dropped line left no trace: {far:?}");
    assert!(far[2] > far[0], "still leaning to line 2: {far:?}");
}

/// A commit with only one line is refused rather than painting a page-wide
/// radial ramp — Enter pressed too early must not cost an undo step.
#[test]
fn committing_one_line_paints_nothing() {
    let Some(mut app) = app_with_freeform() else {
        return;
    };
    let steps = app.doc.undo_labels().len();
    stroke(&mut app, &[(40.0, 20.0), (40.0, 380.0)]);
    app.commit_grad_free();
    assert!(
        app.grad_free.as_ref().is_some_and(|g| g.done.len() == 1),
        "the gesture survives a premature commit"
    );
    assert_eq!(app.doc.undo_labels().len(), steps);
    assert_eq!(px(&app, 200, 200)[3], 0.0);
}
