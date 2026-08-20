use super::new_document_tests::{all_ink, headless, scribble, small_draft};
use crate::cmd::{AppCmd, dispatch};

/// The owner's report, in full: art in the default canvas must SURVIVE
/// making a new manga. The new project opens beside it, not over it.
#[test]
fn a_new_project_opens_in_its_own_tab_and_the_old_art_survives() {
    let Some(mut app) = headless() else { return };
    scribble(&mut app);
    let before = all_ink(&app);
    assert!(before > 0);

    small_draft(&mut app, 1, "Tab two");
    dispatch(&mut app, AppCmd::NewComicCreate);

    assert_eq!(app.doc_count(), 2, "the new project is a second tab");
    assert_eq!(app.active_doc, 1, "and it is the one you are looking at");

    // Back to the first tab: the drawing is exactly where it was left.
    assert!(app.switch_doc(0));
    assert_eq!(all_ink(&app), before, "the old art came back untouched");
    assert_eq!(app.doc_count(), 2, "switching does not close anything");
}

/// Each tab keeps its OWN identity — path, story, page count. A switch
/// that dropped any of them would eventually save one document over
/// another's file.
#[test]
fn each_tab_keeps_its_own_identity() {
    let Some(mut app) = headless() else { return };
    app.story = "First".into();
    small_draft(&mut app, 3, "Second");
    dispatch(&mut app, AppCmd::NewComicCreate);
    assert_eq!(app.story, "Second");
    assert_eq!(app.pages.len(), 3);

    assert!(app.switch_doc(0));
    assert_eq!(app.story, "First");
    assert_eq!(app.pages.len(), 1, "tab 1 is still a single page");

    assert!(app.switch_doc(1));
    assert_eq!(app.story, "Second");
    assert_eq!(app.pages.len(), 3);
}

#[test]
fn tab_labels_read_from_each_document_not_just_the_active_one() {
    let Some(mut app) = headless() else { return };
    app.story = "Alpha".into();
    small_draft(&mut app, 1, "Beta");
    dispatch(&mut app, AppCmd::NewComicCreate);

    let tabs = app.doc_tabs();
    assert_eq!(tabs.len(), 2);
    assert!(tabs[0].0.contains("Alpha"), "parked tab: {:?}", tabs[0].0);
    assert!(tabs[1].0.contains("Beta"), "active tab: {:?}", tabs[1].0);
}

/// Closing a tab closes THAT DOCUMENT. The old behaviour closed the
/// whole application, which the owner called dumb, correctly.
#[test]
fn closing_a_tab_leaves_the_app_running_and_moves_to_a_neighbour() {
    let Some(mut app) = headless() else { return };
    app.story = "Keep me".into();
    scribble(&mut app);
    let kept = all_ink(&app);

    small_draft(&mut app, 1, "Throwaway");
    dispatch(&mut app, AppCmd::NewComicCreate);
    assert_eq!(app.doc_count(), 2);

    assert!(app.close_doc(1), "the second tab closes");
    assert_eq!(app.doc_count(), 1);
    assert_eq!(app.story, "Keep me", "we landed on the neighbour");
    assert_eq!(all_ink(&app), kept, "with its art intact");
    assert!(!app.close_requested, "closing a TAB is not quitting");
}

/// The last document cannot be closed into nothing — `close_doc` says
/// so, and the UI then falls back to the app's own close flow (the one
/// that knows how to ask about unsaved work).
#[test]
fn the_last_document_refuses_to_close_itself() {
    let Some(mut app) = headless() else { return };
    assert!(!app.close_doc(0));
    assert_eq!(app.doc_count(), 1);
}

/// Closing a tab to the LEFT of the active one must not leave the active
/// index pointing at the wrong document — the off-by-one that shows up
/// as "my tab switched by itself".
#[test]
fn closing_an_earlier_tab_keeps_you_on_the_document_you_were_in() {
    let Some(mut app) = headless() else { return };
    app.story = "One".into();
    small_draft(&mut app, 1, "Two");
    dispatch(&mut app, AppCmd::NewComicCreate);
    small_draft(&mut app, 1, "Three");
    dispatch(&mut app, AppCmd::NewComicCreate);
    assert_eq!(app.doc_count(), 3);
    assert_eq!(app.story, "Three");

    assert!(app.close_doc(0));
    assert_eq!(app.doc_count(), 2);
    assert_eq!(app.story, "Three", "still in the document I was editing");
    assert_eq!(app.active_doc, 1);
    assert!(app.switch_doc(0));
    assert_eq!(app.story, "Two");
}
