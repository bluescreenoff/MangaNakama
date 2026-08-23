//! Tone materials place a LIVE TONE LAYER, not pixels (owner report
//! 2026-08-23).
//!
//! Two bugs, one cause. Dragging a tone sheet in pasted it as a raster
//! float, so resizing that float resized the DOTS — and a screen is
//! canvas-absolute, it does not scale with the area it covers. And a
//! dropped tone covered one rectangle instead of the page. CSP's model
//! (verified) is a fill layer plus a mask, which `Document::add_fill_layer`
//! already implements; what these pin is that the DROP PATH uses it.

use super::materials::{
    MaterialKind, ToneSpec, infer_tone_spec, materials_scan_folder, read_tone_spec, write_tone_spec,
};
use super::{App, headless_renderer};
use crate::cmd::{AppCmd, dispatch};
use mn_core::{FillKind, LayerKind, Selection, ToneParams, TonePattern};
use std::path::{Path, PathBuf};

/// A small page — the fill layer derives one tile per 64², and the default
/// new document is 2048².
fn page(app: &mut App) {
    app.doc = mn_core::Document::new(256, 256);
}

/// The repo's shipped tone bank, scanned into folder 0.
fn bank(app: &mut App) -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/materials");
    assert!(
        dir.join("tones/tone-dot-60lpi-30.png").is_file(),
        "the starter tones must ship in assets/materials"
    );
    app.material_folders[0] = dir.clone();
    app.materials_scan();
    dir
}

/// The bank's path for one material stem (the `.tone.json` for a tone —
/// the sidecar is the material's identity, the PNG only its picture).
fn material(app: &App, name: &str) -> PathBuf {
    app.materials
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("the bank must hold {name}"))
        .path
        .clone()
}

fn fill_layers(app: &App) -> Vec<(usize, FillKind)> {
    app.doc
        .layers
        .iter()
        .enumerate()
        .filter_map(|(i, l)| match l.kind {
            LayerKind::Fill(k) => Some((i, k)),
            _ => None,
        })
        .collect()
}

/// (a) NO SELECTION → the whole canvas. The layer is a live `Fill(Tone)`
/// with NO mask (the maskless fill windows everything), no float is opened
/// at all, and the ink reaches the far corner of the page.
///
/// BEFORE THE FIX this failed on the first assert: the drop fell through to
/// `image::open` and opened a move/scale float of the sheet's own pixels.
#[test]
fn a_dropped_tone_fills_the_whole_page_as_a_live_layer() {
    let Some(r) = headless_renderer() else {
        return;
    };
    let mut app = App::new(r, (600, 400), 1.0);
    page(&mut app);
    bank(&mut app);
    let tone = material(&app, "tone-dot-60lpi-30");

    dispatch(
        &mut app,
        AppCmd::PasteMaterial {
            path: tone,
            tile: false,
        },
    );

    assert!(
        app.transform_drag.is_none(),
        "a tone must not arrive as a resizable float — that is the dots-resize bug"
    );
    let fills = fill_layers(&app);
    assert_eq!(fills.len(), 1, "one live layer: {fills:?}");
    let (i, kind) = fills[0];
    let FillKind::Tone { tone, density } = kind else {
        panic!("the dropped material must be a live TONE: {kind:?}");
    };
    assert_eq!(tone.pattern, TonePattern::Dots);
    assert!((tone.lpi - 60.0).abs() < 1e-3, "{} lpi", tone.lpi);
    assert!((density - 0.30).abs() < 1e-3, "density {density}");
    assert!(
        app.doc.layers[i].mask.is_none(),
        "no selection = no window = the whole canvas"
    );

    // …and it really covers the PAGE, not a rectangle somewhere on it:
    // both far corners carry the screen. Counted over a block rather than
    // sampled at a point — a 30 % screen is 70 % paper, so any single
    // pixel is more likely than not to be white.
    app.doc.refresh_derived(600);
    let block = |x0: i32, y0: i32| {
        (y0..y0 + 32)
            .flat_map(|y| (x0..x0 + 32).map(move |x| (x, y)))
            .filter(|&(x, y)| mn_core::export::composite_pixel(&app.doc, x, y).unwrap()[0] < 250)
            .count()
    };
    let (tl, br) = (block(0, 0), block(224, 224));
    assert!(
        tl > 50 && br > 50,
        "the screen must reach both corners: {tl} / {br} inked px"
    );
    assert_eq!(
        app.material_uses.get(&material(&app, "tone-dot-60lpi-30").display().to_string()),
        Some(&1),
        "the drop must still count a use"
    );
}

/// (b) A RECT SELECTION → the tone fills exactly that, and nothing outside
/// it is inked. The mask is the window; the screen behind it is unchanged.
#[test]
fn a_dropped_tone_fills_the_selection_when_there_is_one() {
    let Some(r) = headless_renderer() else {
        return;
    };
    let mut app = App::new(r, (600, 400), 1.0);
    page(&mut app);
    bank(&mut app);
    let tone = material(&app, "tone-dot-60lpi-50");

    app.doc.selection = Some(Selection::from_rect(&app.doc, 32.0, 32.0, 96.0, 96.0));
    dispatch(
        &mut app,
        AppCmd::PasteMaterial {
            path: tone,
            tile: false,
        },
    );

    let fills = fill_layers(&app);
    assert_eq!(fills.len(), 1, "one live layer: {fills:?}");
    let (i, _) = fills[0];
    assert!(
        app.doc.layers[i].mask.is_some(),
        "the selection cut the window"
    );
    app.doc.refresh_derived(600);

    // Inside the rect: ink. Outside: paper, everywhere.
    let px = |x, y| mn_core::export::composite_pixel(&app.doc, x, y).unwrap();
    let inked = (32..96)
        .flat_map(|y| (32..96).map(move |x| (x, y)))
        .filter(|&(x, y)| px(x, y)[0] < 250)
        .count();
    assert!(inked > 200, "a 50% screen inside the window: {inked} px");
    for (x, y) in [(4, 4), (200, 200), (10, 200), (200, 10), (100, 20)] {
        assert_eq!(px(x, y), [255, 255, 255], "inked outside the selection at {x},{y}");
    }
}

/// (c) Inference: a sheet that SAYS it is a tone and states a grade places
/// as one; a photo that merely happens to say "60lpi" does not. The gate is
/// the tag (or the bank's `tone-` prefix), never the number alone.
#[test]
fn tone_inference_needs_the_tag_and_a_density() {
    let got = infer_tone_spec("tone-dot-60lpi-30", "screentone, tone, dots").expect("a tone");
    assert!((got.tone.lpi - 60.0).abs() < 1e-3, "{} lpi", got.tone.lpi);
    assert!((got.density - 0.30).abs() < 1e-3, "density {}", got.density);
    assert_eq!(got.tone.pattern, TonePattern::Dots);

    assert!(
        infer_tone_spec("photo 60lpi", "").is_none(),
        "an untagged photo must never be hijacked into a tone"
    );
    assert!(
        infer_tone_spec("photo 60lpi 30%", "landscape, reference").is_none(),
        "…not even one that spells out a frequency AND a percentage"
    );
    // The `tone-` prefix is the second gate, and the tags carry the grade.
    let pre = infer_tone_spec("tone-line-30lpi-50", "").expect("the prefix gate");
    assert_eq!(pre.tone.pattern, TonePattern::Lines);
    assert!((pre.density - 0.50).abs() < 1e-3);
    // A frequency with no grade is not a flat tone — our own gradient
    // sheet is exactly that, and must keep arriving as pixels.
    assert!(
        infer_tone_spec(
            "tone-dot-60lpi-gradient",
            "screentone, tone, gradient, graded, fade, ramp, dots, 60 lpi"
        )
        .is_none(),
        "a graded sheet has no single density to fill with"
    );
    // The shipped tag lines are what the real bank is read through.
    let noise = infer_tone_spec(
        "tone-noise-30lpi-30",
        "screentone, tone, noise, grain, sand, random, FM, 30%",
    )
    .expect("the noise sheet");
    assert_eq!(noise.tone.pattern, TonePattern::Noise);
    assert!((noise.density - 0.30).abs() < 1e-3);
}

/// (d) The shipped sidecars round-trip: `gen_materials` writes the exact
/// `ToneSpec` the app reads back, and the bank scans the sheet as ONE tone
/// material (the PNG is its thumbnail, never a second entry).
#[test]
fn gen_materials_sidecars_round_trip_through_read_tone_spec() {
    let Some(r) = headless_renderer() else {
        return;
    };
    let mut app = App::new(r, (600, 400), 1.0);
    let dir = bank(&mut app).join("tones");

    let spec = read_tone_spec(&dir.join("tone-dot-42.5lpi-20.tone.json"))
        .expect("the shipped sidecar parses");
    assert_eq!(spec.tone.pattern, TonePattern::Dots);
    assert!((spec.tone.lpi - 42.5).abs() < 1e-3, "{} lpi", spec.tone.lpi);
    assert!((spec.tone.angle_deg - 45.0).abs() < 1e-3);
    assert!((spec.density - 0.20).abs() < 1e-3, "{}", spec.density);

    let items = materials_scan_folder(&dir, 0);
    let sheet: Vec<_> = items
        .iter()
        .filter(|m| m.name == "tone-dot-42.5lpi-20")
        .collect();
    assert_eq!(sheet.len(), 1, "one material, not two: {sheet:?}");
    assert_eq!(sheet[0].kind, MaterialKind::Tone(spec));
    assert_eq!(
        sheet[0].path,
        dir.join("tone-dot-42.5lpi-20.tone.json"),
        "the sidecar is the material's identity"
    );
    assert_eq!(
        sheet[0].thumb_path(),
        dir.join("tone-dot-42.5lpi-20.png"),
        "the PNG is its picture"
    );
    assert!(
        !sheet[0].tags.is_empty(),
        "the sheet's own tags carry over to its sidecar"
    );
    // The one sheet that is NOT a flat tone keeps arriving as pixels.
    assert_eq!(
        items
            .iter()
            .find(|m| m.name == "tone-dot-60lpi-gradient")
            .map(|m| m.kind.clone()),
        Some(MaterialKind::Image),
        "a graded sheet is a bitmap material"
    );

    // A hand-written sidecar round-trips too — the write/read pair is what
    // `Register layer as material` will use next.
    let tmp = std::env::temp_dir().join(format!("mn-tonemat-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let mine = ToneSpec {
        tone: ToneParams {
            pattern: TonePattern::Lozenge,
            lpi: 33.0,
            angle_deg: 15.0,
            ..Default::default()
        },
        density: 0.42,
    };
    let p = write_tone_spec(&tmp, "mine", &mine).expect("written");
    assert_eq!(read_tone_spec(&p), Some(mine));
    let _ = std::fs::remove_dir_all(&tmp);
}

/// The tone drop is ONE undo press, and it leaves the page clean again —
/// the same contract the Tone tool's one-click gesture has.
#[test]
fn a_dropped_tone_costs_one_undo_press() {
    let Some(r) = headless_renderer() else {
        return;
    };
    let mut app = App::new(r, (600, 400), 1.0);
    page(&mut app);
    bank(&mut app);
    let tone = material(&app, "tone-line-60lpi-50");
    let before = app.doc.layers.len();

    dispatch(
        &mut app,
        AppCmd::PasteMaterial {
            path: tone,
            tile: false,
        },
    );
    assert_eq!(app.doc.layers.len(), before + 1);
    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(
        app.doc.layers.len(),
        before,
        "one press must take the whole tone layer back"
    );
    assert!(fill_layers(&app).is_empty());
    // Nothing inked anywhere: a live layer bakes no pixels to leave behind.
    let ink: u64 = app
        .doc
        .layers
        .iter()
        .flat_map(|l| l.tiles())
        .map(|(_, t)| t.alpha_sum())
        .sum();
    assert_eq!(ink, 0, "the tone left no baked pixels");
}
