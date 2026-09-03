//! Friction 6: the two tone objects and the palettes that know about them.
//!
//! A LIVE tone layer (`FillKind::Tone`) carries its screen as parameters;
//! the raster tone EFFECT (`Layer::tone`) screens painted pixels. CSP has
//! one palette for both — the Layer Properties page says outright that a
//! tone layer's detailed settings live there — and CSP never lets a
//! gradient overwrite a tone layer. These pin both.

use crate::cmd::{AppCmd, dispatch};
use mn_core::tile::TileIdx;
use mn_core::{FillKind, LayerKind};

fn drain(app: &mut crate::app::App) {
    while let Some(c) = app.cmds.pop_front() {
        dispatch(app, c);
    }
}

fn page(app: &mut crate::app::App) {
    app.doc = mn_core::Document::new(256, 256);
    app.viewport = mn_gpu::Viewport::default();
}

fn tone_layer(app: &mut crate::app::App) -> usize {
    let li = app.doc.add_fill_layer(
        FillKind::Tone {
            tone: mn_core::tone::ToneParams::default(),
            density: 0.4,
        },
        false,
    );
    app.refresh_tones();
    li
}

/// Does the layer show anything at all?
fn shows_ink(app: &crate::app::App, li: usize) -> bool {
    app.doc.layers[li]
        .display_tiles()
        .values()
        .any(|t| t.alpha_sum() > 0)
}

/// Every string the panel painted this frame, headless: egui renders into
/// shapes, and a text shape carries its galley.
fn panel_text(app: &mut crate::app::App) -> String {
    let ctx = egui::Context::default();
    let out = ctx.run_ui(egui::RawInput::default(), |ui| {
        crate::ui::layers::layer_property(ui, app);
    });
    fn walk(s: &egui::epaint::Shape, into: &mut String) {
        match s {
            egui::epaint::Shape::Text(t) => {
                into.push_str(t.galley.text());
                into.push('\n');
            }
            egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| walk(s, into)),
            _ => {}
        }
    }
    let mut text = String::new();
    for c in &out.shapes {
        walk(&c.shape, &mut text);
    }
    // egui hands the caller a font-atlas delta and panics on drop if it is
    // ignored; nothing here paints, so let it go.
    out.drop_without_applying_deltas();
    text
}

/// The friction as reported: Layer Property read `Layer::tone` only, so a
/// live tone layer's own knobs were nowhere in it and the Effect combo
/// said "None" about a layer made of dots.
#[test]
fn layer_property_shows_a_live_tones_own_rows() {
    let Some(mut app) = super::new_document_tests::headless() else {
        println!("[test] SKIP: no usable adapter");
        return;
    };
    page(&mut app);
    tone_layer(&mut app);

    let text = panel_text(&mut app);
    for want in ["Tone", "Frequency", "Density", "Angle", "Dot position X"] {
        assert!(
            text.contains(want),
            "Layer Property must show the live tone's {want:?} row — it painted:\n{text}"
        );
    }
    assert!(
        !text.contains("Effect"),
        "…and NOT the raster Effect combo, which screens painted pixels a \
         live layer does not have:\n{text}"
    );
}

/// The other half of the same blindness: the Effect combo pushed `SetTone`
/// at a live layer, `Document::set_tone` refuses every non-raster kind, and
/// the status told a tone layer it was a folder. Refusing is right — the
/// effect screens PAINTED ink and a live layer has none — so this pins the
/// refusal and makes it say something true.
#[test]
fn the_raster_tone_effect_refuses_a_live_layer_and_says_why() {
    let Some(mut app) = super::new_document_tests::headless() else {
        println!("[test] SKIP: no usable adapter");
        return;
    };
    page(&mut app);
    let li = tone_layer(&mut app);
    assert!(shows_ink(&app, li), "the live tone screens the page");

    dispatch(&mut app, AppCmd::SetTone(Some(mn_core::ToneParams::default())));
    app.refresh_tones();

    assert!(
        app.doc.layers[li].tone.is_none(),
        "a live layer never takes the raster tone effect"
    );
    assert!(shows_ink(&app, li), "and it is still on the canvas");
    assert!(
        app.status.contains("live"),
        "the refusal says why, it does not just do nothing: {:?}",
        app.status
    );
}

/// CSP's gradient tool makes a NEW gradient layer; it never converts the
/// layer you are standing on into one. Ours retargeted any live layer,
/// so a tone you had tuned was replaced by a ramp.
#[test]
fn a_live_gradient_lands_above_a_tone_layer_instead_of_eating_it() {
    let Some(mut app) = super::new_document_tests::headless() else {
        println!("[test] SKIP: no usable adapter");
        return;
    };
    page(&mut app);
    let li = tone_layer(&mut app);
    app.gradient_live = true;

    app.finish_gradient((20.0, 20.0), (200.0, 20.0));
    drain(&mut app);

    assert!(
        matches!(
            app.doc.layers[li].kind,
            LayerKind::Fill(FillKind::Tone { .. })
        ),
        "the tone survived: {:?}",
        app.doc.layers[li].kind
    );
    let gi = app.doc.active;
    assert_ne!(gi, li, "the ramp went on a layer of its own");
    assert!(
        matches!(
            app.doc.layers[gi].kind,
            LayerKind::Fill(FillKind::Gradient { .. })
        ),
        "…a live gradient layer: {:?}",
        app.doc.layers[gi].kind
    );
    assert!(gi > li, "stacked above the tone, CSP order");
    assert!(
        app.status.contains("tone"),
        "the new layer says why it is new: {:?}",
        app.status
    );

    // The behaviour that must NOT regress: standing on a live GRADIENT
    // layer, a second drag re-aims that ramp instead of stacking again.
    let before = app.doc.layers.len();
    app.finish_gradient((20.0, 200.0), (200.0, 200.0));
    drain(&mut app);
    assert_eq!(app.doc.layers.len(), before, "no second gradient layer");
    let LayerKind::Fill(FillKind::Gradient { a, .. }) = app.doc.layers[gi].kind else {
        panic!("still the gradient layer");
    };
    assert_eq!(a, [20.0, 200.0], "the ramp followed the new drag");
}

/// Sanity: the raster tone effect still works where it belongs — on a
/// raster layer with painted ink.
#[test]
fn the_raster_tone_effect_still_screens_painted_ink() {
    let Some(mut app) = super::new_document_tests::headless() else {
        println!("[test] SKIP: no usable adapter");
        return;
    };
    page(&mut app);
    let li = app.doc.active;
    for y in 0..40 {
        for x in 0..40 {
            let idx = TileIdx::of_pixel(x, y);
            let (ox, oy) = idx.origin();
            app.doc.layers[li].tile_mut(idx).set_pixel(
                (x - ox) as usize,
                (y - oy) as usize,
                [0, 0, 0, mn_core::tile::FIX15_ONE as u16],
            );
        }
    }
    dispatch(&mut app, AppCmd::SetTone(Some(mn_core::ToneParams::default())));
    app.refresh_tones();
    assert!(app.doc.layers[li].tone.is_some(), "raster layers take it");
    assert!(
        app.doc.layers[li]
            .display_tile(TileIdx::new(0, 0))
            .is_some(),
        "and the screen is derived from the ink"
    );
}

/// The Gradient tool's live-layer switch is its OWN. It used to be the
/// bucket's `fill_live`, so a mangaka who turned the bucket destructive
/// (the normal way to use a bucket) silently lost editable gradients too,
/// and there was no way to have one without the other. It also ships ON:
/// a gradient is an object you re-drag a day later, not paint.
#[test]
fn the_gradient_tool_has_its_own_live_switch_and_it_ships_on() {
    let Some(mut app) = super::new_document_tests::headless() else {
        println!("[test] SKIP: no usable adapter");
        return;
    };
    page(&mut app);
    assert!(app.gradient_live, "a gradient drag ships LIVE");
    assert!(!app.fill_live, "…while the bucket still ships destructive");

    // Untouched defaults: one drag, one live gradient layer.
    let raster = app.doc.active;
    let before = app.doc.layers.len();
    app.finish_gradient((20.0, 20.0), (200.0, 20.0));
    drain(&mut app);
    assert_eq!(app.doc.layers.len(), before + 1, "the drag made a layer");
    assert!(
        matches!(
            app.doc.layers[app.doc.active].kind,
            LayerKind::Fill(FillKind::Gradient { .. })
        ),
        "…a LIVE gradient layer: {:?}",
        app.doc.layers[app.doc.active].kind
    );

    // Now the entanglement itself: the bucket's switch ON, the gradient's
    // OFF. The old shared flag read this as "live" and made a second layer.
    app.doc.active = raster;
    app.fill_live = true;
    app.gradient_live = false;
    let before = app.doc.layers.len();
    app.finish_gradient((20.0, 60.0), (200.0, 60.0));
    drain(&mut app);
    assert_eq!(app.doc.layers.len(), before, "no layer: the gradient baked");
    assert!(
        shows_ink(&app, raster),
        "…the ramp landed as pixels on the raster layer"
    );
}
