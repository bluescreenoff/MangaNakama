use super::new_document_tests::{headless, small_draft};
use crate::cmd::{AppCmd, dispatch};

/// The Story Editor is a NON-MODAL window, so it survived a tab click —
/// and it holds DECODED PAGES of the document it was opened on, which
/// its write path re-encodes into `self.pages`. Typing one character in
/// it after a switch replaced the new document's page with the old
/// document's content, wholesale. It is cleared on every switch now,
/// along with the rest of the index-keyed family: each of those is a
/// LAYER INDEX into the document that produced it, and carried across it
/// aims an edit at whatever happens to sit at that index in the other
/// one.
#[test]
fn switching_tabs_clears_the_story_editor_and_the_index_keyed_state() {
    let Some(mut app) = headless() else { return };
    small_draft(&mut app, 1, "Chapter two");
    dispatch(&mut app, AppCmd::NewComicCreate);
    assert_eq!(app.active_doc, 1);

    // Chapter two's editor, open, with chapter two's page decoded in it.
    app.story_open = true;
    app.story_docs.push(Some(mn_core::Document::new(4, 4)));
    app.story_bufs.push("chapter two, page one".into());
    app.story_sel = Some(0);
    // The rest of the family, all keyed by index into THIS document.
    app.gen_sel = Some(0);
    app.renaming = Some((0, "Ink".into()));
    app.frame_delete_armed = Some((0, 0));
    app.eye_solo_backup = Some(vec![true]);
    app.last_selection = Some(mn_core::Selection::default());
    app.frame_order = Some(mn_core::frame_order::PanelOrder::default());
    app.comp_selected = Some(0);

    assert!(app.switch_doc(0), "over to the other document");

    assert!(!app.story_open, "the Story Editor closed");
    assert!(
        app.story_docs.is_empty(),
        "and let go of the other document's pages — its write path re-encodes these"
    );
    assert!(app.story_bufs.is_empty(), "and of its text");
    assert_eq!(app.story_sel, None);
    assert_eq!(app.gen_sel, None, "generated-lines selection");
    assert_eq!(app.renaming, None, "layer rename buffer");
    assert_eq!(app.frame_delete_armed, None, "armed last-frame delete");
    assert!(app.eye_solo_backup.is_none(), "eye-solo visibility backup");
    assert!(app.last_selection.is_none(), "stored selection");
    assert!(app.frame_order.is_none(), "frame order");
    assert_eq!(app.comp_selected, None, "comps selection");
}

/// Rulers are the odd one out in that family: they belong to the
/// DOCUMENT, so a switch PARKS them rather than clearing them. Sitting
/// on the App, one chapter's perspective set went on snapping strokes in
/// the next one — and going back to the document that owns them, they
/// have to still be there, which a blanket clear would have broken.
#[test]
fn rulers_are_parked_with_their_document_not_carried_into_the_next_one() {
    let Some(mut app) = headless() else { return };
    small_draft(&mut app, 1, "Chapter two");
    dispatch(&mut app, AppCmd::NewComicCreate);
    assert_eq!(app.active_doc, 1);

    let edge = mn_core::Ruler::Line {
        a: [10.0, 10.0],
        b: [200.0, 120.0],
    };
    app.doc.rulers.items.push(edge);
    app.doc.rulers.on = true;

    assert!(app.switch_doc(0));
    assert!(
        app.doc.rulers.items.is_empty(),
        "chapter two's ruler followed the click into chapter one"
    );
    assert!(!app.doc.rulers.on, "and so did its snap switch");

    assert!(app.switch_doc(1));
    assert_eq!(
        app.doc.rulers.items,
        vec![edge],
        "the ruler came back with it"
    );
    assert!(
        app.doc.rulers.on,
        "snapping still on where it was turned on"
    );
}
