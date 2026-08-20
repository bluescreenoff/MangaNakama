use super::new_document_tests::{headless, scribble, small_draft};
use crate::cmd::{AppCmd, dispatch};

/// The hole tabs opened: `dirty()` speaks only for the document you are
/// looking at, so a close flow that asks it would discard the work in
/// every other tab without a word. `first_dirty_doc` is what the close
/// flow asks instead — this pins that it sees a BACKGROUND tab.
#[test]
fn unsaved_work_in_a_background_tab_is_still_found() {
    let Some(mut app) = headless() else { return };
    scribble(&mut app); // tab 0 now has unsaved work
    assert!(app.dirty());

    small_draft(&mut app, 1, "Clean");
    dispatch(&mut app, AppCmd::NewComicCreate);
    assert_eq!(app.active_doc, 1);
    assert!(!app.dirty(), "the new tab is clean...");
    assert_eq!(
        app.first_dirty_doc(),
        Some(0),
        "...but tab 0's drawing is still unsaved, and closing must ask"
    );
}

/// With nothing unsaved anywhere, the close flow has nothing to ask
/// about — the case that must NOT produce a prompt.
#[test]
fn a_clean_workspace_reports_no_dirty_document() {
    let Some(mut app) = headless() else { return };
    small_draft(&mut app, 1, "One");
    dispatch(&mut app, AppCmd::NewComicCreate);
    assert_eq!(app.first_dirty_doc(), None);
}

/// "No" to the prompt discards that document and moves on; the next
/// dirty one is then the answer, and eventually there is none.
#[test]
fn discarding_walks_through_every_dirty_tab() {
    let Some(mut app) = headless() else { return };
    scribble(&mut app);
    small_draft(&mut app, 1, "Second");
    dispatch(&mut app, AppCmd::NewComicCreate);
    scribble(&mut app);
    assert!(app.dirty(), "tab 1 is dirty too");

    // The close flow's loop, in miniature.
    let mut guard = 0;
    while let Some(i) = app.first_dirty_doc() {
        if i != app.active_doc {
            app.switch_doc(i);
        }
        app.discard_changes();
        guard += 1;
        assert!(guard < 5, "the walk must terminate");
    }
    assert_eq!(app.first_dirty_doc(), None, "every tab was accounted for");
    assert_eq!(guard, 2, "and both dirty tabs were asked about");
}
