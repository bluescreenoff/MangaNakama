//! Workflow audit #4 (2026-08-29): CSP EX's *File ▸ Import ▸ Batch import*
//! — the "I named the whole chapter on paper" step. N photographed roughs
//! become the draft underlays of N consecutive pages in one gesture.
//!
//! The three things that can go silently wrong, and so are pinned here:
//! the underlay landing UNDER a page's White base (invisible exactly where
//! you draw), a direct byte write leaving a parked live document to be
//! reinstalled over it (workflow audit #1's invariant), and the open page
//! costing more than one undo press to take back.

use super::new_document_tests::{headless, scribble, small_draft};
use crate::cmd::{AppCmd, dispatch};

/// A flat OPAQUE png on disk — a transparent one would pass "a layer
/// exists" while proving nothing about where the pixels went.
fn png(tag: &str, w: u32, h: u32) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mn-batchimport-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let p = dir.join(format!("{tag}.png"));
    image::RgbaImage::from_pixel(w, h, image::Rgba([20, 30, 40, 255]))
        .save(&p)
        .expect("write the test png");
    p
}

/// The paper a work of `pages` pages was created with (72 dpi drafts, the
/// same frugality rule as `new_document_tests`).
fn new_comic(app: &mut crate::App, pages: u32) -> (u32, u32) {
    small_draft(app, pages, "Batch import");
    dispatch(app, AppCmd::NewComicCreate);
    app.page
        .as_ref()
        .expect("a new comic carries its page setup")
        .paper_px()
}

fn has_ink(l: &mn_core::Layer) -> bool {
    l.tiles().any(|(_, t)| t.alpha_sum() > 0)
}

/// A non-open page's stashed bytes, decoded — the only truth about what a
/// direct byte write actually wrote.
fn page_doc(app: &crate::App, i: usize) -> mn_core::Document {
    let b = app.pages[i]
        .bytes
        .as_ref()
        .unwrap_or_else(|| panic!("page {} is stashed", i + 1));
    mn_core::project::bytes_to_doc(b).expect("decode the page")
}

fn draft_at(doc: &mn_core::Document) -> usize {
    doc.layers
        .iter()
        .position(|l| l.draft)
        .expect("a draft underlay")
}

/// The core of the feature: two roughs, two consecutive pages that already
/// exist, each getting the underlay and a fresh content revision — and the
/// PLACEMENT RULE, which is the part that is invisible when it is wrong.
///
/// A page that was blank or drawn carries the frame folder's White base,
/// and White paints the whole panel interior opaque. An underlay at the
/// bottom of THAT stack is hidden everywhere the drawing happens, so it
/// goes directly above the White instead, inside the folder, still under
/// every ink layer.
#[test]
fn batch_import_lands_draft_underlays_on_existing_pages() {
    let Some(mut app) = headless() else { return };
    new_comic(&mut app, 4);
    let before: Vec<u64> = app.pages.iter().map(|e| e.rev).collect();

    dispatch(
        &mut app,
        AppCmd::BatchImportPagesPicked(vec![png("r02", 40, 56), png("r03", 40, 56)]),
    );
    app.batch_import.start = 2; // slots 1 and 2; the open page is slot 0
    dispatch(&mut app, AppCmd::BatchImportApply);

    assert!(
        app.status.contains("2 page(s) written, 0 added"),
        "two existing pages, nothing added: {}",
        app.status
    );
    for i in [1usize, 2] {
        assert!(
            app.pages[i].rev > before[i],
            "page {} took a fresh content revision",
            i + 1
        );
        assert!(
            app.pages[i].thumb.is_none(),
            "page {}'s stale thumbnail was dropped",
            i + 1
        );
        let d = page_doc(&app, i);
        let u = draft_at(&d);
        assert!(has_ink(&d.layers[u]), "the rough actually landed on page {i}");
        let w = d
            .layers
            .iter()
            .position(|l| l.name == "White")
            .expect("a page seeded blank still has its White base");
        assert_eq!(
            u,
            w + 1,
            "the underlay sits directly ABOVE the White base, not under it"
        );
        assert_eq!(
            d.layers[u].depth, d.layers[w].depth,
            "and INSIDE the frame folder, so the panel mask still applies"
        );
        assert!(
            d.layers[u + 1..].iter().all(|l| !l.draft),
            "nothing above it is a draft — the ink layers still print"
        );
    }
    assert_eq!(
        app.pages[3].rev, before[3],
        "a page that was not a target is untouched"
    );
}

/// Workflow audit #1's invariant, from the other side. A page can hold a
/// PARKED live `Document` beside its bytes; a direct byte write that did
/// not bump the page's `rev` would be silently reverted the next time the
/// artist switched to that page, because `switch_page` prefers the park.
/// The batch bumps, so the park is stale and the decode wins.
#[test]
fn a_batch_write_makes_a_parked_page_stale() {
    let Some(mut app) = headless() else { return };
    new_comic(&mut app, 2);
    // Give page 2 real content (a still-blank template page is never
    // parked — it rebuilds instantly and has no history to keep), then
    // leave it, which parks it.
    app.switch_page(1);
    scribble(&mut app);
    app.switch_page(0);
    assert!(
        app.pages[1].parked.is_some(),
        "page 2 parked its live document on the way out"
    );

    dispatch(
        &mut app,
        AppCmd::BatchImportPagesPicked(vec![png("onto-parked", 40, 56)]),
    );
    app.batch_import.start = 2;
    dispatch(&mut app, AppCmd::BatchImportApply);

    assert!(
        app.pages[1].parked.is_some(),
        "the park is still sitting in the slot"
    );
    assert_ne!(
        app.pages[1].parked_rev, app.pages[1].rev,
        "but the rev bump marked it stale"
    );
    // The proof that matters: arriving shows what the batch wrote.
    app.switch_page(1);
    assert!(
        app.doc.layers.iter().any(|l| l.draft),
        "the underlay is on the page, so the stale park was NOT installed"
    );
    assert!(
        app.doc.layers.iter().any(has_ink),
        "and the drawing that was already there survived the round trip"
    );
}

/// More images than pages: the overflow becomes NEW pages of the work's
/// own paper, through the finding-2 door. A freshly imported page has no
/// White base at all, so there the underlay takes the bottom slot.
#[test]
fn batch_import_past_the_end_adds_pages_of_the_works_paper() {
    let Some(mut app) = headless() else { return };
    let paper = new_comic(&mut app, 2);

    dispatch(
        &mut app,
        AppCmd::BatchImportPagesPicked(vec![
            png("a", 40, 56),
            png("b", 40, 56),
            png("c", 40, 56),
        ]),
    );
    app.batch_import.start = 2;
    dispatch(&mut app, AppCmd::BatchImportApply);

    assert_eq!(app.pages.len(), 4, "one page written, two added");
    assert!(
        app.status.contains("1 page(s) written, 2 added"),
        "{}",
        app.status
    );
    for i in [2usize, 3] {
        let d = page_doc(&app, i);
        assert_eq!(d.size, paper, "an added page is the WORK's paper");
        let u = draft_at(&d);
        assert_eq!(
            u, 0,
            "an imported page has no White base, so the underlay is at the bottom"
        );
        assert!(has_ink(&d.layers[u]), "with the rough on it");
        assert!(
            !d.layers.iter().any(|l| l.name == "White"),
            "and nothing white over it"
        );
    }
}

/// The open page is the one page undo covers, and it must cost exactly one
/// press. Building the layer through `add_layer_from_image` + `move_layer`
/// would have recorded two structure groups for one import.
#[test]
fn the_open_page_target_is_one_undo_press() {
    let Some(mut app) = headless() else { return };
    new_comic(&mut app, 2);
    let layers = app.doc.layers.len();
    let steps = app.doc.undo_len();

    dispatch(
        &mut app,
        AppCmd::BatchImportPagesPicked(vec![png("open-page", 40, 56)]),
    );
    app.batch_import.start = 1; // the open page
    dispatch(&mut app, AppCmd::BatchImportApply);

    assert_eq!(
        app.doc.layers.len(),
        layers + 1,
        "the underlay landed on the LIVE document, not on stashed bytes"
    );
    assert!(app.doc.layers.iter().any(|l| l.draft));
    assert_eq!(
        app.doc.undo_len(),
        steps + 1,
        "ONE undo step for the import, not one per helper call"
    );
    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(
        app.doc.layers.len(),
        layers,
        "and one press takes the whole thing back"
    );
    assert!(!app.doc.layers.iter().any(|l| l.draft));
}

/// The picker hands files back in whatever order it feels like. A stack of
/// ネーム photos is named for the chapter, so name order is page order.
#[test]
fn picked_files_are_sorted_by_name() {
    let Some(mut app) = headless() else { return };
    new_comic(&mut app, 1);

    dispatch(
        &mut app,
        AppCmd::BatchImportPagesPicked(vec![png("p03", 8, 8), png("P01", 8, 8), png("p02", 8, 8)]),
    );
    let names: Vec<String> = app
        .batch_import
        .files
        .iter()
        .map(|p| p.file_stem().unwrap_or_default().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        vec!["P01", "p02", "p03"],
        "name order, case-insensitively"
    );
    assert!(app.batch_import_open, "and the dialog opened on that set");
    assert_eq!(
        app.batch_import.start,
        app.page_index + 1,
        "starting at the page the artist is on"
    );
}

/// Twenty photos off the same phone mismatch the paper the same way.
/// Twenty copies of that sentence is not twenty times the information.
#[test]
fn the_aspect_note_is_said_once_for_the_whole_batch() {
    let Some(mut app) = headless() else { return };
    new_comic(&mut app, 3);

    dispatch(
        &mut app,
        AppCmd::BatchImportPagesPicked(vec![png("sq1", 60, 60), png("sq2", 60, 60)]),
    );
    app.batch_import.start = 2;
    dispatch(&mut app, AppCmd::BatchImportApply);

    assert_eq!(
        app.status.matches("not the page's shape").count(),
        1,
        "one note for the batch: {}",
        app.status
    );
}
