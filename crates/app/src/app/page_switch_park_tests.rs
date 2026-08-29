//! Workflow-audit #1 (2026-08-29): a page switch used to be a two-way
//! undo wipe — `switch_page` encoded the leaving page to ORA bytes and
//! decoded a history-less copy on return, while TAB switching one level
//! up parked the live `Document` all along. Pages park now too (a small
//! LRU beside the bytes), and a same-size switch keeps the viewport.

use super::new_document_tests::{all_ink, headless, scribble, small_draft};
use crate::cmd::{AppCmd, dispatch};

/// A two-page comic: draw on page 1, hop to page 2 and back. The stroke
/// must still be undoable — and undoing it must actually remove the ink,
/// not just claim to.
#[test]
fn a_page_round_trip_keeps_the_undo_history() {
    let Some(mut app) = headless() else { return };
    small_draft(&mut app, 2, "Park");
    dispatch(&mut app, AppCmd::NewComicCreate);

    let before = all_ink(&app);
    scribble(&mut app);
    let inked = all_ink(&app);
    assert!(inked != before, "the scribble landed");
    assert!(app.doc.can_undo(), "the stroke pushed history");

    app.switch_page(1);
    assert!(
        app.pages[0].parked.is_some(),
        "the leaving page parked its live document"
    );
    app.switch_page(0);
    assert!(
        app.doc.can_undo(),
        "the history came back with the parked document"
    );
    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(all_ink(&app), before, "undo really removed the stroke");
}

/// The freshness contract: a direct byte writer bumps the page's `rev`,
/// so a parked document whose page moved on without it is DROPPED and
/// the bytes are decoded instead — never install a stale park over real
/// edits.
#[test]
fn a_byte_edit_behind_the_park_wins_over_the_parked_document() {
    let Some(mut app) = headless() else { return };
    small_draft(&mut app, 2, "Stale");
    dispatch(&mut app, AppCmd::NewComicCreate);

    scribble(&mut app);
    app.switch_page(1);
    assert!(app.pages[0].parked.is_some());

    // A direct byte edit on the parked page, through the same door every
    // real writer uses: fresh content revision.
    let bumped = app.page_rev_next();
    app.pages[0].rev = bumped;

    app.switch_page(0);
    assert!(
        !app.doc.can_undo(),
        "the stale park was dropped — the decoded page has no history"
    );
}

/// The LRU cap: with three pages parked in sequence, the oldest loses its
/// document (bytes remain the truth); the newest two keep theirs.
#[test]
fn the_park_keeps_the_newest_two_pages_only() {
    let Some(mut app) = headless() else { return };
    small_draft(&mut app, 4, "Lru");
    dispatch(&mut app, AppCmd::NewComicCreate);

    // Touch each page so none is a lazy blank (those never park).
    for i in [0usize, 1, 2] {
        if app.page_index != i {
            app.switch_page(i);
        }
        scribble(&mut app);
        app.switch_page(i + 1);
    }
    assert!(
        app.pages[0].parked.is_none(),
        "page 1 was evicted by the cap"
    );
    assert!(app.pages[1].parked.is_some(), "page 2 kept its park");
    assert!(app.pages[2].parked.is_some(), "page 3 kept its park");
}

/// The cheap half: same paper size = same viewport. A panel-by-panel
/// sweep across the chapter keeps its zoom and pan; only a page of a
/// different size re-fits.
#[test]
fn a_same_size_switch_keeps_the_viewport() {
    let Some(mut app) = headless() else { return };
    small_draft(&mut app, 2, "View");
    dispatch(&mut app, AppCmd::NewComicCreate);

    app.viewport.zoom = 3.0;
    app.viewport.pan = [-321.0, -123.0];
    app.switch_page(1);
    assert_eq!(app.viewport.zoom, 3.0, "zoom survived the page switch");
    assert_eq!(app.viewport.pan, [-321.0, -123.0], "pan survived too");
}
