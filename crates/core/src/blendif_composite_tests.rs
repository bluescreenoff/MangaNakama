//! Blend If through the real compositor — the pixels, not the arithmetic.
//!
//! `blendif.rs`'s own tests pin the weight curve. These pin what the curve
//! DOES to a page: which compositors apply it, what "underlying" resolves to
//! inside a folder, and that the three CPU entry points (full composite,
//! export composite, `composite_pixel`) cannot answer differently.
//!
//! Its own file rather than more of `export.rs`, which is already 2400 lines
//! and is the last module that wants another 250.

use crate::blendif::BlendIf;
use crate::doc::{Document, Layer};
use crate::export::{self, Background};
use crate::tile::{TILE_SIZE, TileIdx};

/// Flood one tile of `layer` with a straight colour at full alpha.
fn fill(doc: &mut Document, layer: usize, idx: TileIdx, rgb: [f32; 3]) {
    let px = [
        crate::blend::f32_to_fix15(rgb[0]),
        crate::blend::f32_to_fix15(rgb[1]),
        crate::blend::f32_to_fix15(rgb[2]),
        32768,
    ];
    let tile = doc.layers[layer].tile_mut(idx);
    for y in 0..TILE_SIZE {
        for x in 0..TILE_SIZE {
            tile.set_pixel(x, y, px);
        }
    }
}

/// A page whose four tiles are four grey levels, plus a solid red layer on
/// top of it. Tile `(0,0)` is black, `(1,0)` dark, `(0,1)` light, `(1,1)`
/// white — so ONE render exercises the whole luminance axis.
///
/// Returns the document; layer 0 is the wedge, layer 1 the red.
fn wedge_and_red() -> Document {
    let mut doc = Document::new(128, 128);
    for (i, v) in [0.0f32, 0.25, 0.75, 1.0].into_iter().enumerate() {
        fill(
            &mut doc,
            0,
            TileIdx::new((i % 2) as i32, (i / 2) as i32),
            [v, v, v],
        );
    }
    doc.add_layer("red");
    for ty in 0..2 {
        for tx in 0..2 {
            fill(&mut doc, 1, TileIdx::new(tx, ty), [1.0, 0.0, 0.0]);
        }
    }
    doc
}

/// The centre of tile `(tx, ty)` — a pixel well away from any tile seam.
fn mid(tx: i32, ty: i32) -> (i32, i32) {
    (tx * TILE_SIZE as i32 + 32, ty * TILE_SIZE as i32 + 32)
}

fn px(doc: &Document, tx: i32, ty: i32) -> [u8; 3] {
    let (x, y) = mid(tx, ty);
    let img = export::composite(doc, Background::White);
    let p = img.get_pixel(x as u32, y as u32).0;
    [p[0], p[1], p[2]]
}

/// THE FEATURE. A "shadows only" gate: the red shows over the two dark
/// tiles and is gone over the two light ones — and the page underneath is
/// what comes through, unchanged, where it is gone.
#[test]
fn a_shadows_gate_shows_on_the_dark_page_and_hides_on_the_light_one() {
    let mut doc = wedge_and_red();
    // Ungated first, so "hides" is measured against a layer that really was
    // covering all four tiles.
    for (tx, ty) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        assert_eq!(px(&doc, tx, ty), [255, 0, 0], "ungated: red everywhere");
    }

    doc.set_layer_blend_if(
        1,
        Some(BlendIf {
            lo: 0.0,
            hi: 0.5,
            feather: 0.0,
        }),
    );

    assert_eq!(px(&doc, 0, 0), [255, 0, 0], "black page: in range, shows");
    assert_eq!(px(&doc, 1, 0), [255, 0, 0], "0.25 page: in range, shows");
    // 0.75 and 1.0 are out of range: the page itself, with no red at all.
    assert_eq!(px(&doc, 0, 1), [191, 191, 191], "light page: gated out");
    assert_eq!(px(&doc, 1, 1), [255, 255, 255], "white page: gated out");
}

/// The mirror image, so the test cannot pass by hiding everything: a
/// "highlights only" gate keeps exactly the tiles the shadows gate dropped.
#[test]
fn a_highlights_gate_is_the_exact_complement() {
    let mut doc = wedge_and_red();
    doc.set_layer_blend_if(
        1,
        Some(BlendIf {
            lo: 0.5,
            hi: 1.0,
            feather: 0.0,
        }),
    );
    assert_eq!(px(&doc, 0, 0), [0, 0, 0], "black page: gated out");
    assert_eq!(px(&doc, 1, 0), [64, 64, 64], "0.25 page: gated out");
    assert_eq!(px(&doc, 0, 1), [255, 0, 0], "light page: shows");
    assert_eq!(px(&doc, 1, 1), [255, 0, 0], "white page: shows");
}

/// The knee, MEASURED rather than "looks soft". Range `0..0.5`, feather
/// `0.5`: the 0.75 tile sits exactly halfway up the upper ramp, so the red
/// arrives at half strength over a mid-grey page and the result is the
/// arithmetic mean of the two — not "some red", a specific number.
#[test]
fn the_feather_lands_the_layer_at_a_measured_half_strength() {
    let mut doc = wedge_and_red();
    doc.set_layer_blend_if(
        1,
        Some(BlendIf {
            lo: 0.0,
            hi: 0.5,
            feather: 0.5,
        }),
    );
    // Underlying luma 0.75, hi 0.5, feather 0.5 ⇒ w = 1 - 0.25/0.5 = 0.5.
    // Normal blend of premultiplied red at 0.5 over 0.75 grey:
    //   R = 0.5 + 0.75·0.5 = 0.875 → 223
    //   G = B =   0 + 0.75·0.5 = 0.375 → 96
    let p = px(&doc, 0, 1);
    assert_eq!(p, [223, 96, 96], "half weight, exactly");

    // The far side of the ramp is still fully open, and past it fully shut.
    assert_eq!(px(&doc, 0, 0), [255, 0, 0], "inside the band");
    assert_eq!(px(&doc, 1, 1), [255, 255, 255], "1.0 is a full feather out");
}

/// A gate whose range is the whole axis is a no-op, byte for byte — that is
/// what makes `BlendIf::FULL` safe as the value a freshly ticked-on gate
/// starts at, and as the GPU's neutral instance word.
#[test]
fn an_open_gate_is_byte_identical_to_no_gate() {
    let plain = wedge_and_red();
    let mut gated = wedge_and_red();
    gated.set_layer_blend_if(
        1,
        Some(BlendIf {
            // Feather included: it points outward, so it cannot bite here.
            feather: 0.4,
            ..BlendIf::FULL
        }),
    );
    assert_eq!(
        export::composite(&plain, Background::White).into_raw(),
        export::composite(&gated, Background::White).into_raw()
    );
}

/// Every CPU entry point walks the SAME `composite_size`, so a gate cannot
/// reach the screen without reaching the eyedropper and the exported page.
/// If these ever disagree, "what colour is this pixel" has two answers.
#[test]
fn the_full_composite_the_export_and_composite_pixel_agree() {
    let mut doc = wedge_and_red();
    doc.set_layer_blend_if(
        1,
        Some(BlendIf {
            lo: 0.1,
            hi: 0.6,
            feather: 0.3,
        }),
    );
    let screen = export::composite(&doc, Background::White);
    let printed = export::composite_for_export(&doc, Background::White);
    for (tx, ty) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        let (x, y) = mid(tx, ty);
        let s = screen.get_pixel(x as u32, y as u32).0;
        let e = printed.get_pixel(x as u32, y as u32).0;
        let one = export::composite_pixel(&doc, x, y).expect("on canvas");
        assert_eq!([s[0], s[1], s[2]], one, "composite_pixel at {tx},{ty}");
        assert_eq!(s, e, "export sees the gate too at {tx},{ty}");
    }
    // …and the gate really did something, or the agreement is vacuous.
    assert_ne!(
        screen.into_raw(),
        export::composite(&wedge_and_red(), Background::White).into_raw()
    );
}

// --- folders: what "underlying" means when you are inside one -------------

/// Build `page` / `inner` / `gated` where the last two live in a folder.
/// `through` decides whether the folder is sealed or transparent.
///
/// The page is WHITE and the folder's own content is BLACK, so the two
/// possible answers to "what is underneath?" are as far apart as they get:
/// a shadows-only gate shows on the group and hides on the page.
fn folder_fixture(through: bool) -> Document {
    let mut doc = Document::new(128, 128);
    fill(&mut doc, 0, TileIdx::new(0, 0), [1.0, 1.0, 1.0]);

    let inner = doc.add_layer("inner");
    fill(&mut doc, inner, TileIdx::new(0, 0), [0.0, 0.0, 0.0]);
    let gated = doc.add_layer("gated");
    fill(&mut doc, gated, TileIdx::new(0, 0), [1.0, 0.0, 0.0]);
    doc.set_layer_blend_if(
        gated,
        Some(BlendIf {
            lo: 0.0,
            hi: 0.5,
            feather: 0.0,
        }),
    );

    let mut folder = Layer::new("folder");
    folder.folder = true;
    folder.through = through;
    doc.layers[inner].depth = 1;
    doc.layers[gated].depth = 1;
    doc.layers.push(folder);
    doc
}

/// **The folder ruling, decided and pinned.** Inside a SEALED folder the
/// underlying composite is the group's own content — the page beneath the
/// folder is not visible to a child, because the group is isolated. That is
/// Photoshop's answer for a non-pass-through group, and here it falls out of
/// the accumulator model rather than being special-cased.
///
/// The fixture makes the two answers opposite: a shadows-only gate over
/// BLACK group content shows, and would have been hidden outright if the
/// gate had read the white page instead.
#[test]
fn inside_a_sealed_folder_the_underlying_is_the_group() {
    let doc = folder_fixture(false);
    assert_eq!(px(&doc, 0, 0), [255, 0, 0], "gate read the group's black ink");
}

/// A THROUGH folder removes the seal, so its children blend against the page
/// exactly as if loose — and the gate reads the page with them. Same
/// fixture, opposite answer, which is the proof that the sealed case above
/// is a real decision and not an accident of the fixture.
#[test]
fn inside_a_through_folder_the_underlying_is_the_page() {
    let doc = folder_fixture(true);
    // The page is white and the inner layer's black ink lands on it first,
    // so the gate DOES see black here — the point is that it is the same
    // accumulator the page is in, not a separate group. Prove that by
    // hiding the inner layer: sealed would then read transparent (luma 0,
    // in range, shows), through reads the white page (out of range, hides).
    let mut d = doc;
    let inner = d.layers.iter().position(|l| l.name == "inner").unwrap();
    d.set_layer_visible(inner, false);
    assert_eq!(px(&d, 0, 0), [255, 255, 255], "gate read the white page");

    let mut sealed = folder_fixture(false);
    let inner = sealed.layers.iter().position(|l| l.name == "inner").unwrap();
    sealed.set_layer_visible(inner, false);
    assert_eq!(
        px(&sealed, 0, 0),
        [255, 0, 0],
        "sealed reads its own empty group as luma 0, so a shadows gate shows"
    );
}

/// v1 offers the gate on painted layers only, and `Layer::gate` is the one
/// place that says so. A folder carrying one (from a hand-edited file, or a
/// future round that adds the UI) is ignored by every compositor rather than
/// half-honoured by one of them.
#[test]
fn a_folders_gate_is_ignored_by_the_compositor() {
    let mut doc = folder_fixture(false);
    let f = doc.layers.len() - 1;
    // The setter refuses outright…
    assert!(
        !doc.set_layer_blend_if(
            f,
            Some(BlendIf {
                lo: 0.9,
                hi: 1.0,
                feather: 0.0
            })
        ),
        "the document door refuses a folder"
    );
    // …and even a field written past it stays inert.
    doc.layers[f].blend_if = Some(BlendIf {
        lo: 0.9,
        hi: 1.0,
        feather: 0.0,
    });
    assert_eq!(doc.layers[f].gate(), None, "Layer::gate refuses folders");
    assert_eq!(px(&doc, 0, 0), [255, 0, 0], "the picture is unchanged");
}

/// Undo carries the gate: it lives on `Layer`, so the whole-stack snapshot
/// `record_structure` takes restores it with everything else. Pinned here
/// because a field added to `Layer` and forgotten by the snapshot is exactly
/// the kind of thing that only shows up months later.
#[test]
fn the_gate_rides_the_stack_snapshot() {
    let mut doc = wedge_and_red();
    let before = doc.stack_snapshot();
    let active = doc.active;
    doc.record_structure("gate", before, active);
    doc.set_layer_blend_if(
        1,
        Some(BlendIf {
            lo: 0.2,
            hi: 0.4,
            feather: 0.0,
        }),
    );
    assert!(doc.layers[1].gate().is_some());
    doc.undo();
    assert_eq!(doc.layers[1].blend_if, None, "one press took the gate off");
}
