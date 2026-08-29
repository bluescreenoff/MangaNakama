//! `LP-001` Save as default, end to end through the real commands: what a
//! saved default does to the next layer of its type, what it does to the
//! other types (nothing), and the thing that would make the feature a bug —
//! creation growing a second undo step because the defaults were applied
//! after the add's own snapshot.

use crate::app::App;
use crate::app::layer_defaults::LayerDefaults;
use crate::cmd::{AppCmd, dispatch};
use mn_core::{Blend, FillKind, LayerKind, ToneParams};

/// A defaults file as it would sit beside the exe: raster layers at 40 %
/// Multiply with a 30 LPI tone, and nothing said about any other type.
const RASTER_BODY: &str = "raster.opacity=0.4000\n\
     raster.blend=svg:multiply\n\
     raster.tone={\"pattern\":\"dots\",\"lpi\":30.0,\"angle_deg\":15.0}\n";

fn with_defaults(app: &mut App, body: &str) {
    app.layer_defaults = LayerDefaults::parse(body);
}

fn active(app: &App) -> &mn_core::Layer {
    &app.doc.layers[app.doc.active]
}

/// The feature: make a layer of a type you have saved a default for, and it
/// arrives wearing it.
#[test]
fn a_new_raster_layer_starts_from_the_saved_default() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    with_defaults(&mut app, RASTER_BODY);

    dispatch(&mut app, AppCmd::AddLayer);

    let l = active(&app);
    assert!((l.opacity - 0.4).abs() < 1e-4, "opacity: {}", l.opacity);
    assert_eq!(l.blend, Blend::Multiply);
    assert_eq!(l.tone.expect("the tone default landed").lpi, 30.0);
    // Identity and safety are never defaulted.
    assert!(l.visible && !l.lock && !l.draft && !l.clip);
    assert!(l.name.starts_with("Layer"));
}

/// A default belongs to ONE type. A folder made under a raster default is
/// a stock folder.
#[test]
fn other_types_do_not_pick_up_a_raster_default() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    with_defaults(&mut app, RASTER_BODY);

    dispatch(&mut app, AppCmd::AddFolder);

    let f = active(&app);
    assert!(f.folder, "the folder was made");
    assert_eq!(f.opacity, 1.0, "a raster default is not a folder default");
    assert_eq!(f.blend, Blend::Normal);
    assert_eq!(f.tone, None);
}

/// THE regression this feature could introduce: the defaults are written
/// after `add_layer` has already snapshotted the stack, so they must ride
/// inside that one step. One press removes the layer wholesale; redo brings
/// it back still wearing them.
#[test]
fn creating_a_defaulted_layer_is_still_one_undo_press() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    with_defaults(&mut app, RASTER_BODY);
    let (layers, steps) = (app.doc.layers.len(), app.doc.undo_labels().len());

    dispatch(&mut app, AppCmd::AddLayer);

    assert_eq!(app.doc.layers.len(), layers + 1);
    assert_eq!(
        app.doc.undo_labels().len(),
        steps + 1,
        "applying defaults must not record a step of its own"
    );

    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(
        app.doc.layers.len(),
        layers,
        "ONE press took the whole creation away"
    );
    assert_eq!(app.doc.undo_labels().len(), steps);

    dispatch(&mut app, AppCmd::Redo);
    assert_eq!(app.doc.layers.len(), layers + 1);
    assert_eq!(
        active(&app).blend,
        Blend::Multiply,
        "redo restores the layer with its defaults on it"
    );
}

/// The tone caution: a saved TONE default is live-fill PARAMETERS, so it
/// has to reach the layer as creation input (the derived-raster stamp is
/// taken inside `add_fill_layer`) — and still inside one undo step.
#[test]
fn a_saved_tone_default_reaches_a_new_tone_layer_in_one_step() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    with_defaults(
        &mut app,
        "tone.opacity=0.8000\n\
         tone.fill={\"Tone\":{\"tone\":{\"pattern\":\"lines\",\"lpi\":25.0,\"angle_deg\":90.0},\"density\":0.9}}\n",
    );
    let (layers, steps) = (app.doc.layers.len(), app.doc.undo_labels().len());

    // What the Layer menu pushes: stock tone parameters.
    dispatch(
        &mut app,
        AppCmd::NewLiveFill(FillKind::Tone {
            tone: ToneParams::default(),
            density: 0.4,
        }),
    );

    let l = active(&app);
    assert!((l.opacity - 0.8).abs() < 1e-4);
    match l.kind {
        LayerKind::Fill(FillKind::Tone { tone, density }) => {
            assert_eq!(tone.pattern, mn_core::TonePattern::Lines);
            assert_eq!(tone.lpi, 25.0);
            assert_eq!(density, 0.9, "the saved density, not the menu's 0.4");
        }
        ref k => panic!("not a tone fill layer: {k:?}"),
    }
    assert_eq!(
        app.doc.undo_labels().len(),
        steps + 1,
        "one step for the creation, defaults included"
    );

    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(app.doc.layers.len(), layers, "one press, layer and all");
}

/// The command path: Save as default reads the ACTIVE layer, and Forget
/// puts the type back to stock. (The file write is a no-op in tests — the
/// exe directory here is `target/debug/deps` — so this exercises the
/// in-memory half, which is what creation reads.)
#[test]
fn save_and_forget_go_through_the_active_layer() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let i = app.doc.active;
    app.doc.layers[i].blend = Blend::Screen;
    app.doc.layers[i].opacity = 0.5;

    dispatch(&mut app, AppCmd::SaveLayerDefaults);
    assert!(app.layer_defaults.has("raster"));

    dispatch(&mut app, AppCmd::AddLayer);
    assert_eq!(active(&app).blend, Blend::Screen);
    assert_eq!(active(&app).opacity, 0.5);

    dispatch(&mut app, AppCmd::ForgetLayerDefaults);
    assert!(!app.layer_defaults.has("raster"));

    dispatch(&mut app, AppCmd::AddLayer);
    assert_eq!(active(&app).blend, Blend::Normal, "stock again");
    assert_eq!(active(&app).opacity, 1.0);
}
