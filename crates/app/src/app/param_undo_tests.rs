//! One undo press per parameter-edit SESSION, for all three live kinds
//! (fill, tone lattice, correction layer).
//!
//! The hole these close: a live layer's parameters used to change the
//! document with no history record at all, so Ctrl+Z after nudging a tone
//! curve skipped the nudge and undid whatever came before it. The fix is
//! one shape shared by all three — the params live in `Layer.kind` and the
//! derived raster beside them, so a `record_structure` stack snapshot
//! restores BOTH; the only question was when to take it.
//!
//! - Tool Property sliders (fill, gradient, live tone): the first tick of a
//!   drag records, and the panel then reports the drag
//!   (`ParamEditSession`) so the rest of it only re-derives. Coalescing is
//!   opt-in, so every other param source — preset click, canvas lattice
//!   nudge — keeps its own undo step.
//! - The correction dialog: the pre-image is taken at open, recorded at
//!   Apply. Cancel restores the opening params in place and records
//!   nothing.

use crate::app::App;
use crate::cmd::{AppCmd, dispatch};
use mn_core::{Adjust, FillKind, LayerKind, TileIdx, ToneParams};

const RED: FillKind = FillKind::Flat {
    color: [1.0, 0.0, 0.0, 1.0],
};
const BLUE: FillKind = FillKind::Flat {
    color: [0.0, 0.0, 1.0, 1.0],
};

/// A fresh page with one flat-red live fill layer, its raster derived.
/// Returns the layer index and the undo depth at that moment.
fn red_fill(app: &mut App) -> (usize, usize) {
    app.doc = mn_core::Document::new(256, 256);
    let li = app.doc.add_fill_layer(RED, false);
    app.refresh_tones();
    (li, app.doc.undo_labels().len())
}

fn fill_kind(app: &App, li: usize) -> FillKind {
    match app.doc.layers[li].kind {
        LayerKind::Fill(k) => k,
        ref k => panic!("not a fill layer: {k:?}"),
    }
}

/// The derived (not painted) tile a compositor would show. The WHOLE tile,
/// not one pixel: a lattice offset moves the dots without changing the
/// coverage, so any single sample (and `alpha_sum`) can miss the move.
fn derived(app: &App, li: usize) -> Vec<u16> {
    app.doc.layers[li]
        .display_tile(TileIdx::new(0, 0))
        .expect("the live layer derived its raster")
        .data()
        .to_vec()
}

/// THE BUG, fill half: one param edit, one undo press, and the press puts
/// back the old numbers AND the old pixels.
#[test]
fn a_fill_param_edit_is_one_undo_step_that_restores_pixels_too() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let (li, steps) = red_fill(&mut app);
    let was = derived(&app, li);

    dispatch(&mut app, AppCmd::SetFillParams(li, BLUE));

    assert_eq!(fill_kind(&app, li), BLUE, "the edit landed");
    assert_ne!(derived(&app, li), was, "the raster re-derived");
    assert_eq!(
        app.doc.undo_labels().len(),
        steps + 1,
        "the edit recorded exactly one undo step"
    );

    dispatch(&mut app, AppCmd::Undo);

    assert_eq!(fill_kind(&app, li), RED, "one press put the parameters back");
    assert_eq!(
        derived(&app, li),
        was,
        "…and the derived pixels came back with them"
    );
    assert_eq!(app.doc.undo_labels().len(), steps, "one press, one step");
}

/// One frame of a Tool Property slider drag, in the order the panel emits
/// it: the value first, then the "a drag is live" report. The first frame
/// therefore records; the rest land inside the open session.
fn drag_tick(app: &mut App, li: usize, kind: FillKind) {
    dispatch(app, AppCmd::SetFillParams(li, kind));
    dispatch(app, AppCmd::ParamEditSession(Some(li)));
}

/// A slider drag emits a `SetFillParams` every frame so the canvas follows
/// the pointer. All of them are ONE undo step, and the step's pre-image is
/// the state before the drag started — not the second-to-last tick.
#[test]
fn a_drags_many_ticks_coalesce_into_one_step() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let (li, steps) = red_fill(&mut app);
    let was = derived(&app, li);

    for i in 1..=8 {
        let g = i as f32 / 8.0;
        drag_tick(
            &mut app,
            li,
            FillKind::Flat {
                color: [1.0, g, 0.0, 1.0],
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
        fill_kind(&app, li),
        RED,
        "undo rewinds the WHOLE drag, not one tick of it"
    );
    assert_eq!(derived(&app, li), was);
}

/// Letting go of the pointer ends the session: the next drag is its own
/// undo step, so two tweaks take two presses to unwind.
#[test]
fn letting_go_ends_the_session() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let (li, steps) = red_fill(&mut app);

    drag_tick(&mut app, li, BLUE);
    dispatch(&mut app, AppCmd::ParamEditSession(None));
    drag_tick(&mut app, li, RED);

    assert_eq!(
        app.doc.undo_labels().len(),
        steps + 2,
        "release between them = two sessions = two steps"
    );
    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(fill_kind(&app, li), BLUE, "the second tweak came off");
    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(fill_kind(&app, li), RED, "then the first");
}

/// Any unrelated command is the other session door — the same belt-and-
/// braces guard that protects the correction preview.
#[test]
fn an_unrelated_command_ends_the_session() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let (li, steps) = red_fill(&mut app);

    drag_tick(&mut app, li, BLUE);
    dispatch(&mut app, AppCmd::ToneShowArea);
    drag_tick(&mut app, li, RED);

    assert_eq!(app.doc.undo_labels().len(), steps + 2);
}

/// Coalescing is opt-IN. A param source that is NOT a slider drag — the
/// Object tool's lattice nudge, a gradient preset click, the Fill tool's
/// live switch — never opens a session, so two of them in a row are two
/// undo steps even with nothing dispatched in between.
#[test]
fn two_discrete_param_edits_stay_two_steps() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let (li, steps) = red_fill(&mut app);

    dispatch(&mut app, AppCmd::SetFillParams(li, BLUE));
    dispatch(&mut app, AppCmd::SetFillParams(li, RED));

    assert_eq!(
        app.doc.undo_labels().len(),
        steps + 2,
        "no drag was reported, so neither edit was swallowed by the other"
    );
}

/// Setting the params to what they already are is not an edit and must not
/// leave a do-nothing step in the History palette.
#[test]
fn a_no_op_param_set_records_nothing() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let (li, steps) = red_fill(&mut app);

    dispatch(&mut app, AppCmd::SetFillParams(li, RED));

    assert_eq!(app.doc.undo_labels().len(), steps);
}

/// THE BUG, tone-lattice half: nudging a live tone layer's dot position
/// (the Object tool's lattice drag ends in exactly this command) is one
/// undo press, dots included.
#[test]
fn a_tone_lattice_nudge_is_one_undo_step() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    app.doc = mn_core::Document::new(256, 256);
    let base = ToneParams {
        lpi: 12.0,
        ..Default::default()
    };
    let li = app.doc.add_fill_layer(
        FillKind::Tone {
            tone: base,
            density: 0.6,
        },
        false,
    );
    app.refresh_tones();
    let steps = app.doc.undo_labels().len();
    let was = derived(&app, li);

    let moved = ToneParams {
        offset: [7.0, 3.0],
        ..base
    };
    dispatch(
        &mut app,
        AppCmd::SetFillParams(
            li,
            FillKind::Tone {
                tone: moved,
                density: 0.6,
            },
        ),
    );
    assert_ne!(derived(&app, li), was, "the dots slid");
    assert_eq!(app.doc.undo_labels().len(), steps + 1);

    dispatch(&mut app, AppCmd::Undo);
    let FillKind::Tone { tone, .. } = fill_kind(&app, li) else {
        panic!("still a tone layer");
    };
    assert_eq!(tone.offset, [0.0, 0.0], "the lattice went home");
    assert_eq!(derived(&app, li), was, "…and so did the dots");
}

// --- correction layers ---------------------------------------------------

fn correction_layer(app: &mut App) -> (usize, usize) {
    app.doc = mn_core::Document::new(256, 256);
    // Something below for the correction to derive from, or its raster is
    // an empty page either way and the pixel check proves nothing.
    app.doc.fill_selection([0.5, 0.5, 0.5]);
    dispatch(app, AppCmd::NewCorrectionLayer(Adjust::BINARIZE));
    let li = app
        .doc
        .layers
        .iter()
        .position(|l| matches!(l.kind, LayerKind::Correction(_)))
        .expect("the command made one");
    app.refresh_tones();
    (li, app.doc.undo_labels().len())
}

fn corr_params(app: &App, li: usize) -> Adjust {
    match app.doc.layers[li].kind {
        LayerKind::Correction(a) => a,
        ref k => panic!("not a correction layer: {k:?}"),
    }
}

/// THE BUG, correction half: the dialog's whole session — open, drag,
/// Apply — is ONE undo press, and it restores the params and the derived
/// page together.
#[test]
fn a_correction_dialog_session_is_one_undo_step() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let (li, steps) = correction_layer(&mut app);
    let was = derived(&app, li);

    dispatch(&mut app, AppCmd::CorrectionEdit);
    // Three frames of slider: each one writes the layer live (that IS the
    // preview) and none of them may record.
    for t in [0.3f32, 0.6, 0.8] {
        app.adjust_draft = Some(Adjust::Binarize { threshold: t });
        app.adjust_preview_sync();
        assert_eq!(
            app.doc.undo_labels().len(),
            steps,
            "a live preview frame records nothing"
        );
    }
    dispatch(&mut app, AppCmd::AdjustApply);

    assert_eq!(
        corr_params(&app, li),
        Adjust::Binarize { threshold: 0.8 },
        "Apply kept the edit"
    );
    assert_eq!(
        app.doc.undo_labels().len(),
        steps + 1,
        "the whole dialog is one undo step"
    );
    assert_ne!(derived(&app, li), was, "the corrected page changed");

    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(
        corr_params(&app, li),
        Adjust::BINARIZE,
        "one press put the opening params back"
    );
    assert_eq!(derived(&app, li), was, "…and the page with them");
}

/// Cancel restores the opening params by re-applying them — that is not an
/// edit, and must leave NO undo residue for Ctrl+Z to trip over.
#[test]
fn a_cancelled_dialog_leaves_no_undo_residue() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let (li, steps) = correction_layer(&mut app);
    let was = derived(&app, li);

    dispatch(&mut app, AppCmd::CorrectionEdit);
    app.adjust_draft = Some(Adjust::Binarize { threshold: 0.9 });
    app.adjust_preview_sync();
    dispatch(&mut app, AppCmd::AdjustCancel);

    assert_eq!(corr_params(&app, li), Adjust::BINARIZE);
    assert_eq!(derived(&app, li), was);
    assert_eq!(
        app.doc.undo_labels().len(),
        steps,
        "Cancel is not an edit — nothing recorded"
    );
}

/// Apply on a dialog nobody touched is not an edit either.
#[test]
fn an_untouched_dialog_apply_records_nothing() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let (li, steps) = correction_layer(&mut app);

    dispatch(&mut app, AppCmd::CorrectionEdit);
    dispatch(&mut app, AppCmd::AdjustApply);

    assert_eq!(corr_params(&app, li), Adjust::BINARIZE);
    assert_eq!(app.doc.undo_labels().len(), steps);
}
