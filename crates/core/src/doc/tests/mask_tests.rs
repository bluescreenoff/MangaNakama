use super::super::*;
use crate::export::{Background, composite};

/// LM-002/007/003: the mask scales the layer's contribution in the
/// CPU composite — outside-selection hides, the toggle restores,
/// delete removes, clear empties.
#[test]
fn mask_hides_in_composite_and_toggles() {
    let mut doc = Document::new(128, 128);
    doc.begin_op();
    // BOTH tiles the probes land in — (5,5) and (100,5) straddle the
    // x=64 selection edge, so paint tiles (0,0) and (1,0).
    for idx in [TileIdx::new(0, 0), TileIdx::new(1, 0)] {
        let t = doc.layers[0].tile_mut(idx);
        for p in 0..crate::tile::TILE_PIXELS {
            t.set_pixel(p % 64, p / 64, [32768, 0, 0, 32768]);
        }
    }
    doc.end_op();
    let full = composite(&doc, Background::Transparent);
    assert_eq!(full.get_pixel(5, 5).0[3], 255, "unmasked: opaque");

    // Mask outside the LEFT half-selection → the right half hides.
    doc.selection = Some(crate::selection::Selection::from_rect(
        &doc, 0.0, 0.0, 64.0, 128.0,
    ));
    assert!(doc.mask_outside_selection(0));
    let masked = composite(&doc, Background::Transparent);
    assert_eq!(
        masked.get_pixel(5, 5).0[3],
        255,
        "inside the selection: kept"
    );
    assert_eq!(masked.get_pixel(100, 5).0[3], 0, "outside: hidden");

    // LM-007: disable → everything shows again.
    assert!(doc.mask_set_enabled(0, false));
    let off = composite(&doc, Background::Transparent);
    assert_eq!(off.get_pixel(100, 5).0[3], 255, "mask off: full layer");

    // LM-003: clear (all hidden) vs delete (mask gone).
    assert!(doc.mask_set_enabled(0, true));
    assert!(doc.mask_clear(0));
    let cleared = composite(&doc, Background::Transparent);
    assert_eq!(cleared.get_pixel(5, 5).0[3], 0, "cleared: all hidden");
    assert!(doc.layers[0].mask.is_some(), "the mask itself kept");
    assert!(doc.mask_delete(0));
    let deleted = composite(&doc, Background::Transparent);
    assert_eq!(deleted.get_pixel(5, 5).0[3], 255, "deleted: unmasked again");
}

/// LM-006: baking multiplies the layer by coverage and removes the
/// mask — one undo op that restores BOTH the pixels and the mask
/// (undo restores pixels; the mask returns via the op's... check:
/// mask_delete runs OUTSIDE the op, so undo restores pixels only.
/// The test pins the pixels + the mask's absence.)
#[test]
fn bake_multiplies_and_removes_mask() {
    let mut doc = Document::new(128, 128);
    doc.begin_op();
    for idx in [TileIdx::new(0, 0), TileIdx::new(1, 0)] {
        let t = doc.layers[0].tile_mut(idx);
        for p in 0..crate::tile::TILE_PIXELS {
            t.set_pixel(p % 64, p / 64, [32768, 0, 0, 32768]);
        }
    }
    doc.end_op();
    doc.selection = Some(crate::selection::Selection::from_rect(
        &doc, 0.0, 0.0, 64.0, 128.0,
    ));
    assert!(doc.mask_outside_selection(0));

    assert!(doc.mask_apply_bake(0));
    assert!(doc.layers[0].mask.is_none(), "the mask is gone");
    // The layer pixels are now half-opacity on the right (masked) side.
    let alpha = |doc: &Document, x: i32| {
        let idx = TileIdx::of_pixel(x, 5);
        doc.layers[0]
            .tile(idx)
            .map(|t| t.pixel((x - idx.origin().0) as usize, (5 - idx.origin().1) as usize)[3])
            .unwrap_or(0)
    };
    assert_eq!(alpha(&doc, 5), 32768, "inside: untouched");
    assert_eq!(alpha(&doc, 100), 0, "outside: baked away");
    // TWO undo steps now (the round-68 MaskField group fixed the old
    // deviation): the first restores the MASK, the second the pixels.
    assert!(doc.undo());
    assert!(doc.layers[0].mask.is_some(), "the mask returns first");
    assert!(doc.undo());
    assert_eq!(alpha(&doc, 100), 32768, "then the pixels");
}

/// The MaskField group: mask create/toggle/clear/delete and STROKES
/// all undo — and redo — through whole-field snapshots.
#[test]
fn maskfield_group_makes_mask_ops_undoable() {
    let mut doc = Document::new(128, 128);
    doc.begin_op();
    doc.layers[0]
        .tile_mut(TileIdx::new(0, 0))
        .set_pixel(5, 5, [32768, 0, 0, 32768]);
    doc.end_op();
    doc.selection = Some(crate::selection::Selection::from_rect(
        &doc, 0.0, 0.0, 64.0, 128.0,
    ));
    assert!(doc.mask_outside_selection(0));
    assert_eq!(doc.undo_len(), 2, "layer op + mask creation");
    assert!(doc.undo(), "undo creation");
    assert!(doc.layers[0].mask.is_none(), "creation undone");
    assert!(doc.redo());
    assert!(doc.layers[0].mask.is_some(), "creation redone");

    // Toggle: undo restores the previous enabled state.
    assert!(doc.mask_set_enabled(0, false));
    assert!(doc.undo());
    assert!(doc.layers[0].mask.as_ref().unwrap().enabled);
    // Delete: undo brings the mask back.
    assert!(doc.mask_delete(0));
    assert!(doc.layers[0].mask.is_none());
    assert!(doc.undo());
    assert!(doc.layers[0].mask.is_some(), "delete undone");
}

/// LM-001: the starter mask is all-visible.
#[test]
fn starter_mask_is_all_visible() {
    let mut doc = Document::new(128, 128);
    doc.begin_op();
    let t = doc.layers[0].tile_mut(TileIdx::new(0, 0));
    t.set_pixel(5, 5, [0, 0, 32768, 32768]);
    doc.end_op();
    assert!(doc.mask_selection_blank(0));
    let img = composite(&doc, Background::Transparent);
    assert_eq!(img.get_pixel(5, 5).0[3], 255, "blank mask hides nothing");
}

/// The bake matches the screen. A mask only has tiles where the layer
/// had ink at creation; ink painted AFTER lands in tiles the mask never
/// covers, which both compositors render VISIBLE. Apply-mask used to
/// treat those absent tiles as coverage 0 and silently DELETE that ink
/// — shown one moment, gone after the bake. Verified failing against
/// the old `unwrap_or(0)`.
#[test]
fn bake_keeps_ink_painted_after_the_mask_was_made() {
    let mut doc = Document::new(192, 128);
    doc.begin_op();
    let t = doc.layers[0].tile_mut(TileIdx::new(0, 0));
    t.set_pixel(5, 5, [32768, 0, 0, 32768]);
    doc.end_op();
    assert!(doc.mask_selection_blank(0), "mask over tile (0,0) only");

    // New ink in a tile the mask has no entry for.
    doc.begin_op();
    let t = doc.layers[0].tile_mut(TileIdx::new(2, 0));
    t.set_pixel(10, 5, [0, 32768, 0, 32768]);
    doc.end_op();
    let before = composite(&doc, Background::Transparent);
    assert_eq!(before.get_pixel(138, 5).0[3], 255, "on screen before");

    assert!(doc.mask_apply_bake(0));
    assert!(doc.layers[0].mask.is_none(), "bake consumed the mask");
    let after = composite(&doc, Background::Transparent);
    assert_eq!(after.get_pixel(5, 5).0[3], 255, "masked-visible ink kept");
    assert_eq!(
        after.get_pixel(138, 5).0[3],
        255,
        "ink painted after the mask must survive the bake"
    );
}
