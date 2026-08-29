//! Row 105 correction LAYERS, app side: the command makes a live layer
//! (params + optional window, never baked pixels), the shared correction
//! dialog opens ON it in params-only mode, Cancel restores the opening
//! parameters, and Apply keeps the new ones without baking anything.

use crate::cmd::{AppCmd, dispatch};
use mn_core::{Adjust, LayerKind};

fn page(app: &mut crate::app::App) {
    app.doc = mn_core::Document::new(256, 256);
}

fn correction_layers(app: &crate::app::App) -> Vec<usize> {
    app.doc
        .layers
        .iter()
        .enumerate()
        .filter(|(_, l)| matches!(l.kind, LayerKind::Correction(_)))
        .map(|(i, _)| i)
        .collect()
}

fn params(app: &crate::app::App, li: usize) -> Adjust {
    match app.doc.layers[li].kind {
        LayerKind::Correction(a) => a,
        ref k => panic!("not a correction layer: {k:?}"),
    }
}

/// The command makes the layer, cuts the window from the selection, and
/// queues the dialog; the layer's raster is DERIVED (no painted pixels).
#[test]
fn the_command_makes_a_windowed_live_layer_and_opens_its_dialog() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    page(&mut app);
    app.doc.selection = Some(mn_core::Selection::from_rect(
        &app.doc, 0.0, 0.0, 64.0, 64.0,
    ));

    dispatch(&mut app, AppCmd::NewCorrectionLayer(Adjust::LEVELS));

    let cs = correction_layers(&app);
    assert_eq!(cs.len(), 1, "one command, one correction layer");
    let li = cs[0];
    assert_eq!(params(&app, li), Adjust::LEVELS);
    assert!(
        app.doc.layers[li].mask.is_some(),
        "the selection cut the window"
    );
    assert_eq!(
        app.doc.layers[li].tiles().count(),
        0,
        "params, not painted pixels"
    );
    // The parameterised kinds queue their dialog (the queue drains next
    // frame in the real app; the test dispatches it by hand).
    dispatch(&mut app, AppCmd::CorrectionEdit);
    assert_eq!(app.adjust_draft, Some(Adjust::LEVELS), "dialog open, seeded");
    assert!(
        app.adjust_live.as_ref().is_some_and(|l| l.layer == li),
        "…in params-only mode, on the new layer"
    );
    assert!(
        app.adjust_preview.is_none(),
        "no pixel snapshots in live mode — nothing will be baked"
    );
}

/// Slider moves reach the LAYER through the sync; Cancel restores the
/// opening params exactly, and leaves no undo residue (the pre-image is
/// only recorded on Apply — see `param_undo_tests`).
#[test]
fn cancel_restores_the_opening_parameters() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    page(&mut app);
    dispatch(&mut app, AppCmd::NewCorrectionLayer(Adjust::BINARIZE));
    let li = correction_layers(&app)[0];
    dispatch(&mut app, AppCmd::CorrectionEdit);

    // The user drags the threshold: the dialog writes the draft, the sync
    // pushes it into the layer.
    app.adjust_draft = Some(Adjust::Binarize { threshold: 0.9 });
    app.adjust_preview_sync();
    assert_eq!(
        params(&app, li),
        Adjust::Binarize { threshold: 0.9 },
        "the layer follows the sliders live"
    );

    dispatch(&mut app, AppCmd::AdjustCancel);
    assert_eq!(
        params(&app, li),
        Adjust::BINARIZE,
        "Cancel put the opening params back"
    );
    assert!(app.adjust_draft.is_none() && app.adjust_live.is_none());
}

/// Apply keeps the edited params; the dialog closes; nothing is baked into
/// pixels. The param change itself IS one undo step (`param_undo_tests`
/// owns that assertion); here we only pin that it is exactly one.
#[test]
fn apply_keeps_the_new_parameters_without_baking() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    page(&mut app);
    dispatch(&mut app, AppCmd::NewCorrectionLayer(Adjust::BINARIZE));
    let li = correction_layers(&app)[0];
    dispatch(&mut app, AppCmd::CorrectionEdit);
    let steps = app.doc.undo_labels().len();

    app.adjust_draft = Some(Adjust::Binarize { threshold: 0.2 });
    app.adjust_preview_sync();
    dispatch(&mut app, AppCmd::AdjustApply);

    assert_eq!(
        params(&app, li),
        Adjust::Binarize { threshold: 0.2 },
        "Apply kept the edit"
    );
    assert!(app.adjust_draft.is_none() && app.adjust_live.is_none());
    assert_eq!(
        app.doc.undo_labels().len(),
        steps + 1,
        "the whole dialog session is ONE undo step"
    );
    assert_eq!(
        app.doc.layers[li].tiles().count(),
        0,
        "still parameters — nothing baked into pixels"
    );
}

/// The dispatch head guard: any unrelated command while the live dialog is
/// open closes it and restores the opening params — the same door that
/// protects the destructive preview.
#[test]
fn an_unrelated_command_closes_the_live_dialog_and_restores() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    page(&mut app);
    dispatch(&mut app, AppCmd::NewCorrectionLayer(Adjust::BINARIZE));
    let li = correction_layers(&app)[0];
    dispatch(&mut app, AppCmd::CorrectionEdit);
    app.adjust_draft = Some(Adjust::Binarize { threshold: 0.9 });
    app.adjust_preview_sync();

    dispatch(&mut app, AppCmd::Deselect);

    assert!(app.adjust_live.is_none(), "the guard closed the dialog");
    assert_eq!(
        params(&app, li),
        Adjust::BINARIZE,
        "…and restored the opening params"
    );
}
