use super::new_document_tests::{all_ink, headless, scribble, small_draft};
use crate::app::unsaved_autosave_path_for;
use crate::cmd::{AppCmd, dispatch};

/// `dirty()` answers for the document you are LOOKING AT, which is the
/// wrong question for the tab ×: the × destroys a document that may be
/// parked. Asking it was how one click threw away a drawing with no
/// prompt. `doc_dirty(i)` is what the × asks now, and it has to give a
/// DIFFERENT answer per tab — the thing one app-wide flag cannot say.
#[test]
fn doc_dirty_answers_for_the_tab_you_name_not_the_one_you_are_in() {
    let Some(mut app) = headless() else { return };
    scribble(&mut app); // tab 0 now holds unsaved work
    small_draft(&mut app, 1, "Clean");
    dispatch(&mut app, AppCmd::NewComicCreate);

    assert!(!app.dirty(), "the tab in front of you is clean");
    assert!(
        app.doc_dirty(0),
        "...and the parked one is not — × must ask"
    );
    assert!(!app.doc_dirty(1), "the active tab answers for itself");

    // The other direction, so this cannot pass by always saying "dirty":
    // clean tab 0, dirty tab 1, and the two answers swap.
    assert!(app.switch_doc(0));
    app.discard_changes();
    assert!(app.switch_doc(1));
    scribble(&mut app);
    assert!(app.dirty(), "the second scribble landed");
    assert!(!app.doc_dirty(0), "the parked tab has nothing to lose now");
    assert!(app.doc_dirty(1), "the active one does");
}

/// The autosave tick only ever encoded the ACTIVE document, so unsaved
/// work in a background tab was written NOWHERE and a crash took it with
/// nothing for the recovery prompt to offer. And every never-saved
/// document shared ONE `%TEMP%` path, so two dirty tabs produced one
/// file and one survivor.
#[test]
fn autosave_writes_every_dirty_background_tab_to_its_own_file() {
    let Some(mut app) = headless() else { return };
    // Slot 0 is deliberately left CLEAN and its path never touched: that
    // slot keeps the historical `%TEMP%` name a real crash writes, and a
    // test has no business writing over the owner's recovery file.
    app.discard_changes();

    small_draft(&mut app, 1, "Alpha");
    dispatch(&mut app, AppCmd::NewComicCreate);
    scribble(&mut app);
    assert!(app.dirty(), "tab 1 has unsaved work");

    small_draft(&mut app, 1, "Beta");
    dispatch(&mut app, AppCmd::NewComicCreate);
    scribble(&mut app);
    assert!(app.dirty(), "tab 2 has unsaved work");

    small_draft(&mut app, 1, "Untouched");
    dispatch(&mut app, AppCmd::NewComicCreate); // tab 3: clean, never saved
    small_draft(&mut app, 1, "Front");
    dispatch(&mut app, AppCmd::NewComicCreate); // tab 4: the active one

    let alpha = unsaved_autosave_path_for(1);
    let beta = unsaved_autosave_path_for(2);
    let untouched = unsaved_autosave_path_for(3);
    assert_ne!(alpha, beta, "two never-saved tabs cannot share one stash");
    for p in [&alpha, &beta, &untouched] {
        std::fs::remove_file(p).ok();
    }
    assert!(!app.doc_dirty(0), "slot 0 stays out of this — see above");

    assert_eq!(app.autosave_parked(), 2, "the dirty tabs, and only those");
    assert!(!untouched.exists(), "a clean background tab writes nothing");

    // Each stash holds ITS OWN document. This is the collision seen from
    // the only side that matters: whose work is in the file.
    let a = mn_core::project::load(&alpha).expect("tab 1's stash");
    let b = mn_core::project::load(&beta).expect("tab 2's stash");
    assert_eq!(a.meta.story, "Alpha");
    assert_eq!(b.meta.story, "Beta", "tab 1 overwrote tab 2's stash");

    for p in [&alpha, &beta] {
        std::fs::remove_file(p).ok();
    }
}

/// Autosaving a parked document means encoding it WITHOUT making it
/// active — and the page being edited is not in `pages[i].bytes` at all,
/// it lives in `doc` (the entry is emptied while the page is open). An
/// encoder that only walked `pages` would write that page out empty:
/// the autosave would be missing precisely the page you were drawing.
#[test]
fn a_parked_session_encodes_the_page_that_is_still_in_its_document() {
    let Some(mut app) = headless() else { return };
    small_draft(&mut app, 3, "Round trip");
    dispatch(&mut app, AppCmd::NewComicCreate);
    dispatch(&mut app, AppCmd::SelectPage(1));
    // Draw on an ART layer. A decoded page comes back with the frame
    // folder's header active, and that header's raster is DERIVED from
    // the frame vectors — re-made on load, so ink put there survives no
    // encode at all and would measure nothing here.
    dispatch(&mut app, AppCmd::AddLayer);
    let before = all_ink(&app);
    scribble(&mut app);
    assert!(all_ink(&app) > before, "the scribble landed on page 2");

    // Park it behind another tab. Nothing stashes the open page on the
    // way out — that is what `as_project` has to cope with.
    small_draft(&mut app, 1, "Front");
    dispatch(&mut app, AppCmd::NewComicCreate);
    let s = app.docs[1].as_ref().expect("tab 1 is parked");
    assert_eq!(s.page_index, 1);
    assert!(
        s.pages[1].bytes.is_none(),
        "the edited page is in `doc`, not in the entry — that is the point"
    );

    let proj = s.as_project().expect("the parked document encodes");
    let mut buf = std::io::Cursor::new(Vec::new());
    mn_core::project::save_to(&proj, &mut buf).expect("save");
    buf.set_position(0);
    let back = mn_core::project::load_from(buf).expect("load");
    assert_eq!(back.pages.len(), 3, "every page came back");
    assert!(!back.pages[1].is_empty(), "including the one being edited");

    // ...and it is the page that was DRAWN on, not a blank re-encode.
    let ink = |bytes: &[u8]| -> u64 {
        mn_core::project::bytes_to_doc(bytes)
            .expect("the page decodes")
            .layers
            .iter()
            .flat_map(|l| l.tiles())
            .map(|(_, t)| t.alpha_sum())
            .sum()
    };
    let (drawn, blank) = (ink(&back.pages[1]), ink(&back.pages[0]));
    assert!(
        drawn > blank,
        "the round trip carried the stroke that only existed in `doc` ({drawn} vs {blank})"
    );
}
