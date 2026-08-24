//! 05 item 1 — a pathless work autosaves into an incremental TEMP work
//! folder instead of one monolithic zip. The invariants that make it
//! safe: only dirty pages re-encode, `saved_rev` never advances (the
//! real home must not be lied to about what is saved), recovery finds
//! the folder, and the stash dies whole when the work gains a path.

use super::new_document_tests::{headless, scribble, small_draft};
use crate::app::unsaved_autosave_folder_for;
use crate::cmd::{AppCmd, dispatch};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

fn page_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ora")))
        .collect();
    v.sort();
    v
}

fn mtime(p: &Path) -> SystemTime {
    std::fs::metadata(p).unwrap().modified().unwrap()
}

/// The owner's actual footgun: a never-saved multi-page work re-encoded
/// EVERYTHING on the UI thread each tick. Now the first tick writes each
/// page once (the folder starts empty) and every later tick re-encodes
/// only pages that changed — and the temp watermark that makes that
/// cheap is `autosaved_rev`, never `saved_rev`.
#[test]
fn pathless_autosave_writes_only_dirty_pages_and_never_saved_rev() {
    let Some(mut app) = headless() else { return };
    // Slot 0 keeps the historical stash name (a real crash file may live
    // there); the new comic lands in the next slot, as in the parked
    // tests.
    app.discard_changes();
    small_draft(&mut app, 3, "TempFolder");
    dispatch(&mut app, AppCmd::NewComicCreate);
    assert!(app.doc_path.is_none(), "the work under test is pathless");
    let slot = app.active_doc;
    let index = unsaved_autosave_folder_for(slot);
    let dir = index.parent().unwrap().to_path_buf();
    let _ = std::fs::remove_dir_all(&dir);

    scribble(&mut app); // the active page (page 0) is dirty
    dispatch(&mut app, AppCmd::Autosave);
    assert!(index.is_file(), "the temp work-folder index exists");
    assert_eq!(page_files(&dir).len(), 3, "first autosave: every page once");

    // Next tick: only the page that changed again may re-encode.
    std::thread::sleep(std::time::Duration::from_millis(30));
    let before: Vec<_> = page_files(&dir).iter().map(|p| mtime(p)).collect();
    scribble(&mut app);
    dispatch(&mut app, AppCmd::Autosave);
    let files = page_files(&dir);
    assert_eq!(files.len(), 3, "no page files appear or vanish");
    let mut rewritten = 0;
    for (p, t) in files.iter().zip(&before) {
        if mtime(p) > *t {
            rewritten += 1;
        }
    }
    assert_eq!(rewritten, 1, "exactly the still-dirty page re-encoded");
    assert!(
        mtime(&files[0]) > before[0],
        "and it is page 0 (p001.ora), the one that was scribbled on"
    );

    // THE watermark trap: a temp write must never tell the real home a
    // page is saved — the next Save As rewrites everything it should.
    for e in &app.pages {
        assert_eq!(
            e.saved_rev, 0,
            "a temp autosave advanced saved_rev — Save As would skip pages"
        );
        assert_eq!(
            e.autosaved_rev,
            e.rev.max(1),
            "the temp watermark did advance"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// A single-file work with a path keeps its shadow BESIDE ITSELF (the
/// sibling `.autosave.mnc` recovery ranks against the file it shadows);
/// only a PATHLESS work goes to the temp folder.
#[test]
fn a_saved_single_file_work_still_autosaves_beside_itself() {
    let Some(mut app) = headless() else { return };
    app.discard_changes();
    // Slot 1 is deliberately burned as a filler so this test's document
    // lands in slot 2 — the pathless test above runs in PARALLEL on slot
    // 1's folder, and this test ends by deleting its own slot's folder to
    // prove the non-write. Two tests, one slot, one flake.
    small_draft(&mut app, 1, "Filler");
    dispatch(&mut app, AppCmd::NewComicCreate);
    small_draft(&mut app, 1, "Shadowed");
    dispatch(&mut app, AppCmd::NewComicCreate);
    scribble(&mut app);
    // Give it a real path by saving single-file (the inline build the
    // Autosave sibling arm itself uses; App has no `as_project` — that is
    // the parked-session encoder).
    let dir = std::env::temp_dir().join(format!(
        "mn-autosave-folder-{}-saved",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let doc = dir.join("shadowed.mnc");
    app.stash_current_page().unwrap();
    let mut proj =
        mn_core::Project::new(app.story.clone(), app.page.clone(), app.binding_right);
    proj.pages = app
        .pages
        .iter()
        .map(|e| e.bytes.clone().unwrap_or_default())
        .collect();
    app.pages[app.page_index].bytes = None;
    mn_core::project::save(&proj, &doc).expect("save the single-file work");
    app.set_doc_path(Some(doc.clone()));
    app.mark_saved();
    scribble(&mut app);

    dispatch(&mut app, AppCmd::Autosave);
    let side = crate::recovery::sibling_autosave(&doc);
    assert!(side.is_file(), "the shadow sits beside the document");
    let slot_folder = unsaved_autosave_folder_for(app.active_doc)
        .parent()
        .unwrap()
        .to_path_buf();
    let _ = std::fs::remove_dir_all(&slot_folder); // not ours; must not exist
    assert!(
        !slot_folder.exists(),
        "a pathed work must not write into the pathless temp folder"
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&side);
}
