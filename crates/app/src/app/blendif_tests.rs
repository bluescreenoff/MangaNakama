//! Blend If at the command level: it is UNDOABLE, and a slider drag is one
//! press.
//!
//! The gate sits in the Layer Property panel next to the layer colour and
//! the expression preview, and those two record no history at all — they are
//! display-only, so there is nothing for Ctrl+Z to owe. Blend If is the odd
//! one out in that row: it decides what the EXPORTED page holds, so it takes
//! the undo shape the live-layer parameters use instead
//! (`param_undo_tests.rs`, `AppCmd::ParamEditSession`).

use crate::app::App;
use crate::cmd::{AppCmd, dispatch};
use mn_core::BlendIf;

const SHADOWS: BlendIf = BlendIf {
    lo: 0.0,
    hi: 0.4,
    feather: 0.1,
    ..BlendIf::FULL
};
const HIGHLIGHTS: BlendIf = BlendIf {
    lo: 0.6,
    hi: 1.0,
    feather: 0.1,
    ..BlendIf::FULL
};

/// A fresh page with one ordinary layer. Returns the layer index and the
/// undo depth at that moment.
fn page(app: &mut App) -> (usize, usize) {
    app.doc = mn_core::Document::new(256, 256);
    (0, app.doc.undo_labels().len())
}

fn gate(app: &App, li: usize) -> Option<BlendIf> {
    app.doc.layers[li].blend_if
}

/// One frame of a property-panel slider drag, in the order the panel emits
/// it: the value first, then the "a drag is live" report. The first frame
/// therefore records; the rest land inside the open session.
fn drag_tick(app: &mut App, li: usize, g: BlendIf) {
    dispatch(app, AppCmd::SetLayerBlendIf(li, Some(g)));
    dispatch(app, AppCmd::ParamEditSession(Some(li)));
}

/// Switching the gate on is one undo step, and one press puts the layer
/// back to showing everywhere.
#[test]
fn turning_the_gate_on_is_one_undo_step() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let (li, steps) = page(&mut app);

    dispatch(&mut app, AppCmd::SetLayerBlendIf(li, Some(SHADOWS)));

    assert_eq!(gate(&app, li), Some(SHADOWS), "the edit landed");
    assert_eq!(app.doc.undo_labels().len(), steps + 1);

    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(gate(&app, li), None, "one press took the gate off");
    assert_eq!(app.doc.undo_labels().len(), steps);
}

/// A slider drag emits one command per frame so the canvas follows the
/// pointer. All of them are ONE undo step, and the step's pre-image is the
/// state before the drag started — not the second-to-last tick.
#[test]
fn a_drags_many_ticks_coalesce_into_one_step() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let (li, steps) = page(&mut app);

    for i in 1..=8 {
        drag_tick(
            &mut app,
            li,
            BlendIf {
                lo: 0.0,
                hi: i as f32 / 10.0,
                feather: 0.1,
                ..BlendIf::FULL
            },
        );
    }

    assert_eq!(
        app.doc.undo_labels().len(),
        steps + 1,
        "eight ticks of one drag are one undo step"
    );
    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(
        gate(&app, li),
        None,
        "undo rewinds the WHOLE drag, not one tick of it"
    );
}

/// Letting go of the pointer ends the session: the next drag is its own undo
/// step, so two tweaks take two presses to unwind.
#[test]
fn letting_go_ends_the_session() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let (li, steps) = page(&mut app);

    drag_tick(&mut app, li, SHADOWS);
    dispatch(&mut app, AppCmd::ParamEditSession(None));
    drag_tick(&mut app, li, HIGHLIGHTS);

    assert_eq!(
        app.doc.undo_labels().len(),
        steps + 2,
        "release between them = two sessions = two steps"
    );
    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(gate(&app, li), Some(SHADOWS), "the second tweak came off");
    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(gate(&app, li), None, "then the first");
}

/// The belt-and-braces door: ANY other command means the drag cannot still
/// be running. Blend If had to be added to the dispatch head's allow-list to
/// keep a session open across its own ticks, and this is the test that the
/// addition did not swallow everything else with it.
#[test]
fn an_unrelated_command_ends_the_session() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let (li, steps) = page(&mut app);

    drag_tick(&mut app, li, SHADOWS);
    dispatch(&mut app, AppCmd::ToneShowArea);
    drag_tick(&mut app, li, HIGHLIGHTS);

    assert_eq!(app.doc.undo_labels().len(), steps + 2);
}

/// Coalescing is opt-IN. The checkbox and the reset button are finished
/// one-shot gestures that never report a session, so two of them in a row
/// are two undo steps even with nothing dispatched in between.
#[test]
fn two_discrete_edits_stay_two_steps() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let (li, steps) = page(&mut app);

    dispatch(&mut app, AppCmd::SetLayerBlendIf(li, Some(SHADOWS)));
    dispatch(&mut app, AppCmd::SetLayerBlendIf(li, Some(HIGHLIGHTS)));

    assert_eq!(
        app.doc.undo_labels().len(),
        steps + 2,
        "no drag was reported, so neither edit was swallowed by the other"
    );
}

/// Setting the gate to what it already is — a bar re-emitting its value, a
/// reset on an already reset gate — is not an edit and must not leave a
/// do-nothing step in the History palette.
#[test]
fn a_no_op_set_records_nothing() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let (li, steps) = page(&mut app);

    dispatch(&mut app, AppCmd::SetLayerBlendIf(li, None));
    assert_eq!(app.doc.undo_labels().len(), steps, "off → off");

    dispatch(&mut app, AppCmd::SetLayerBlendIf(li, Some(SHADOWS)));
    dispatch(&mut app, AppCmd::SetLayerBlendIf(li, Some(SHADOWS)));
    assert_eq!(app.doc.undo_labels().len(), steps + 1, "and the same value");
}

/// The reset affordance goes back to the OPEN range, not to off: the panel
/// section stays expanded so the next drag has something to grab. The gate
/// is then a visible no-op, which `BlendIf::is_open` and every compositor
/// agree about.
#[test]
fn reset_returns_to_the_open_range_and_stays_switched_on() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let (li, _) = page(&mut app);

    dispatch(&mut app, AppCmd::SetLayerBlendIf(li, Some(SHADOWS)));
    dispatch(&mut app, AppCmd::SetLayerBlendIf(li, Some(BlendIf::FULL)));

    assert_eq!(gate(&app, li), Some(BlendIf::FULL), "still switched on");
    assert_eq!(
        app.doc.layers[li].gate(),
        None,
        "…and inert: no compositor pays for it"
    );
}

/// A crossed range cannot reach the document. The panel clamps the two
/// handles against each other, but a script or a replayed action can send
/// anything, and `hi < lo` would hide the layer outright.
#[test]
fn a_crossed_range_is_normalised_by_the_command() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let (li, _) = page(&mut app);

    dispatch(
        &mut app,
        AppCmd::SetLayerBlendIf(
            li,
            Some(BlendIf {
                lo: 0.9,
                hi: 0.1,
                feather: 2.0,
                ..BlendIf::FULL
            }),
        ),
    );
    assert_eq!(
        gate(&app, li),
        Some(BlendIf {
            lo: 0.1,
            hi: 0.9,
            feather: 1.0,
            ..BlendIf::FULL
        })
    );
}

/// Folders are refused at the command, not just hidden in the panel.
#[test]
fn a_folder_refuses_the_gate() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    page(&mut app);
    let f = app.doc.add_layer("folder");
    app.doc.layers[f].folder = true;
    // AFTER the add: `add_layer` records its own step, and the point here is
    // that the refused command adds nothing on top of it.
    let steps = app.doc.undo_labels().len();

    dispatch(&mut app, AppCmd::SetLayerBlendIf(f, Some(SHADOWS)));

    assert_eq!(gate(&app, f), None, "refused");
    assert_eq!(
        app.doc.undo_labels().len(),
        steps,
        "and it recorded nothing to undo"
    );
}
