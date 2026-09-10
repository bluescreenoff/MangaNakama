use super::super::*;

/// One undoable edit on the active layer.
fn edit(doc: &mut Document) {
    doc.begin_op();
    let li = doc.active;
    doc.layers[li]
        .tile_mut(TileIdx::new(0, 0))
        .set_pixel(1, 1, [32768, 0, 0, 32768]);
    doc.end_op();
}

#[test]
fn every_edit_undo_and_redo_moves_the_count() {
    let mut doc = Document::new(128, 128);
    let start = doc.op_count();

    edit(&mut doc);
    let after_edit = doc.op_count();
    assert!(after_edit > start, "an edit is an operation");

    assert!(doc.undo());
    let after_undo = doc.op_count();
    assert!(
        after_undo > after_edit,
        "undo is an operation too — its result is what needs recovering"
    );

    assert!(doc.redo());
    assert!(doc.op_count() > after_undo, "and so is redo");
}

/// The reason this is not `undo_len()`. Past the depth cap the stack
/// stops growing while the work does not, and a recovery save keyed to
/// the stack length would quietly stop firing — mid-session, with no
/// symptom, in the one feature whose entire job is to not do that.
#[test]
fn the_count_keeps_moving_after_the_depth_cap_is_reached() {
    let mut doc = Document::new(128, 128);
    doc.set_undo_limit(50); // the preference's own floor
    for _ in 0..50 {
        edit(&mut doc);
    }
    let capped = doc.undo_len();
    let ops = doc.op_count();

    for _ in 0..10 {
        edit(&mut doc);
    }
    assert_eq!(doc.undo_len(), capped, "the stack is full and stays full");
    assert_eq!(
        doc.op_count(),
        ops + 10,
        "the tally is not the stack; it counted all ten"
    );
}

/// Structural layer ops record a Structure group now, so they count
/// through the ordinary push path; the change undo genuinely cannot
/// express (`clear_history`, e.g. a resize) still counts via `note_op`
/// and never rewinds the tally.
#[test]
fn structural_ops_count_and_clearing_the_history_does_not_reset_the_tally() {
    let mut doc = Document::new(128, 128);
    edit(&mut doc);
    let before = doc.op_count();

    doc.add_layer("ref");
    assert!(
        doc.op_count() > before,
        "adding a layer is an operation ({before} -> {})",
        doc.op_count()
    );
    assert_eq!(doc.undo_len(), 2, "…and it is on the stack, not a wipe");
    let ops = doc.op_count();
    doc.clear_history();
    assert_eq!(doc.undo_len(), 0);
    assert!(
        doc.op_count() > ops,
        "the tally is monotonic: clear_history counts and never rewinds"
    );
}
// --- PA-001: the paper + the transparency checker ---------------------

/// The colour is CONTENT: an empty page composites to it, and that is
/// what a PNG export writes. A cream page prints cream.
#[test]
fn paper_colour_is_what_an_empty_page_exports() {
    let mut doc = Document::new(64, 64);
    assert_eq!(doc.paper, Paper::default(), "new documents are white paper");

    assert!(doc.set_paper_colour([250, 243, 224]), "cream");
    let img = crate::export::composite_for_export(&doc, doc.paper_export_background());
    assert_eq!(
        img.get_pixel(10, 10).0,
        [250, 243, 224, 255],
        "the empty page IS the paper"
    );

    // Setting the same colour again is a no-op, and pushes no undo entry.
    let depth = doc.history.undo_len();
    assert!(!doc.set_paper_colour([250, 243, 224]));
    assert_eq!(doc.history.undo_len(), depth, "a no-op pushes nothing");
}

/// THE EXPORT RULE. Hiding the paper is a look-at-it check — it must not
/// be able to ship a page with a transparent background, and the checker
/// must never reach a PNG. The exported bytes are identical either way.
#[test]
fn hiding_the_paper_never_changes_an_export() {
    let mut doc = Document::new(64, 64);
    doc.set_paper_colour([250, 243, 224]);
    doc.begin_op();
    doc.layers[0]
        .tile_mut(TileIdx::new(0, 0))
        .set_pixel(4, 4, [0, 0, 0, 32768]);
    doc.end_op();

    let shown = crate::export::composite_for_export(&doc, doc.paper_export_background());
    assert!(doc.set_paper_visible(false));
    let hidden = crate::export::composite_for_export(&doc, doc.paper_export_background());
    assert_eq!(
        shown.as_raw(),
        hidden.as_raw(),
        "the paper's eye is view state: an export cannot see it"
    );
    assert_eq!(
        hidden.get_pixel(10, 10).0,
        [250, 243, 224, 255],
        "still exports ON the paper colour with the eye off, and opaque"
    );
}

/// The other half: on SCREEN the eye does reach the composite, and it
/// reaches it as real transparency — which is what the viewer draws the
/// checker through. Painted pixels are untouched either way.
#[test]
fn hiding_the_paper_makes_the_screen_composite_transparent() {
    let mut doc = Document::new(64, 64);
    doc.begin_op();
    doc.layers[0]
        .tile_mut(TileIdx::new(0, 0))
        .set_pixel(4, 4, [0, 0, 0, 32768]);
    doc.end_op();

    assert_eq!(
        doc.paper_background(),
        crate::export::Background::Solid([255, 255, 255])
    );
    assert!(doc.set_paper_visible(false));
    assert_eq!(
        doc.paper_background(),
        crate::export::Background::Transparent,
        "no paper means nothing under the stack"
    );

    let img = crate::export::composite(&doc, doc.paper_background());
    assert_eq!(img.get_pixel(10, 10).0[3], 0, "empty pixels are a hole");
    assert_eq!(img.get_pixel(4, 4).0[3], 255, "the art is untouched");
}

/// The colour is undoable (it is content); the eye is not (it is view
/// state, like a layer's own eye). The paper's undo group belongs to no
/// layer, so a per-layer history purge must not drop it.
#[test]
fn paper_colour_undoes_and_the_eye_does_not() {
    let mut doc = Document::new(64, 64);
    doc.add_layer("second");

    assert!(doc.set_paper_colour([12, 34, 56]));
    assert_eq!(doc.paper.colour, [12, 34, 56]);
    assert!(doc.undo(), "the colour undoes");
    assert_eq!(doc.paper.colour, [255, 255, 255], "back to white");
    assert!(doc.redo(), "and redoes");
    assert_eq!(doc.paper.colour, [12, 34, 56]);

    // The eye pushes nothing.
    let depth = doc.history.undo_len();
    assert!(doc.set_paper_visible(false));
    assert_eq!(
        doc.history.undo_len(),
        depth,
        "the eye is view state — no undo entry"
    );
    assert!(!doc.set_paper_visible(false), "idempotent");
    assert!(doc.set_paper_visible(true));

    // A layer's history is purged: the paper's group has no layer to
    // belong to and must survive it.
    doc.history.drop_layer_history(1);
    assert!(doc.undo(), "the paper colour is still undoable");
    assert_eq!(doc.paper.colour, [255, 255, 255]);
}
