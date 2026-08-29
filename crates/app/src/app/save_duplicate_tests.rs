//! `IO-003` Save Duplicate — the copy lands, and the work you are in does
//! not move. Both halves matter: a duplicate that quietly re-pointed
//! `doc_path` would send the next Ctrl+S to the copy, and one that marked
//! the work clean would let the real file's unsaved hours close without a
//! prompt.

use super::new_document_tests::{all_ink, headless, scribble, small_draft};
use crate::cmd::{AppCmd, dispatch};

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("mn-dup-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A single-page work: the `.ora` copy is written, and every piece of
/// save-state the original carries is exactly where it was.
#[test]
fn a_duplicate_lands_on_disk_and_leaves_the_original_where_it_was() {
    let Some(mut app) = headless() else {
        return;
    };
    let dir = tmp("ora");
    let home = dir.join("chapter.ora");
    dispatch(&mut app, AppCmd::SaveOraPath(home.clone()));
    assert_eq!(app.doc_path.as_deref(), Some(home.as_path()));
    assert!(!app.dirty(), "a save marks it clean");

    // Draw, so the copy carries something the file on disk does not — and
    // so there is a dirty flag with something to lose.
    scribble(&mut app);
    assert!(app.dirty(), "the stroke made it dirty");
    let ink = all_ink(&app);

    let copy = dir.join("send-this.ora");
    dispatch(&mut app, AppCmd::SaveDuplicatePath(copy.clone()));

    assert!(copy.exists(), "the duplicate landed: {}", app.status);
    assert_eq!(
        app.doc_path.as_deref(),
        Some(home.as_path()),
        "you are still in the original"
    );
    assert!(app.dirty(), "…and it is still unsaved");
    assert_eq!(
        all_ink(&app),
        ink,
        "the document itself was not touched"
    );
    // The real file on disk is untouched too — the duplicate went to the
    // other path, not over the top of the work.
    let reopened = mn_core::ora::load(&home).expect("the original still reads");
    let on_disk: u64 = reopened
        .layers
        .iter()
        .flat_map(|l| l.tiles())
        .map(|(_, t)| t.alpha_sum())
        .sum();
    assert!(on_disk < ink, "the original file predates the stroke");
    // And the COPY has it.
    let dup = mn_core::ora::load(&copy).expect("the duplicate reads");
    let dup_ink: u64 = dup
        .layers
        .iter()
        .flat_map(|l| l.tiles())
        .map(|(_, t)| t.alpha_sum())
        .sum();
    // ORA stores 8-bit PNG and the document is fix15, so the round trip is
    // lossy by a hair — "the same page", not bit equality. What matters is
    // that the copy has the stroke and the file we are still working in
    // does not.
    assert!(dup_ink > on_disk, "the copy is today's state, not the file's");
    assert!(
        (dup_ink as f64 - ink as f64).abs() / (ink as f64) < 1e-3,
        "…and it is the whole page: {dup_ink} vs {ink}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A comic duplicated into a work FOLDER: every page must be written, even
/// the ones already safe in the original's folder. The incremental
/// watermark is the trap — borrowed for the copy, given straight back, so
/// the original's next real Save still knows what it owes.
#[test]
fn a_work_folder_duplicate_writes_every_page_and_gives_the_ledger_back() {
    let Some(mut app) = headless() else {
        return;
    };
    small_draft(&mut app, 3, "duplicate");
    dispatch(&mut app, AppCmd::NewComicCreate);
    if app.pages.len() < 3 {
        return; // no comic: nothing to duplicate
    }
    let dir = tmp("work");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let index = home.join(mn_core::project::WORKFOLDER_INDEX);
    dispatch(&mut app, AppCmd::SaveOraPath(index.clone()));
    assert_eq!(app.doc_path.as_deref(), Some(index.as_path()), "{}", app.status);
    let pages = app.pages.len();
    let ledger: Vec<(u32, u64)> = app.pages.iter().map(|e| (e.id, e.saved_rev)).collect();
    let managed = app.folder_managed.clone();
    let dirty = app.dirty();

    let away = dir.join("away");
    std::fs::create_dir_all(&away).unwrap();
    dispatch(
        &mut app,
        AppCmd::SaveDuplicatePath(away.join(mn_core::project::WORKFOLDER_INDEX)),
    );

    // Every page is in the copy — the incremental skip would have left
    // most of them out, and nothing would have said so.
    let written = std::fs::read_dir(&away)
        .unwrap()
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            mn_core::project::is_workfolder_file(&n) && n != mn_core::project::WORKFOLDER_INDEX
        })
        .count();
    assert_eq!(written, pages, "all {pages} pages in the copy: {}", app.status);
    let proj = mn_core::project::load_folder(&away.join(mn_core::project::WORKFOLDER_INDEX))
        .expect("the duplicate folder opens");
    assert_eq!(proj.pages.len(), pages);

    // …and the original is untouched, ledger included.
    assert_eq!(app.doc_path.as_deref(), Some(index.as_path()));
    assert_eq!(app.dirty(), dirty, "the dirty flag did not move");
    assert_eq!(
        app.pages.iter().map(|e| (e.id, e.saved_rev)).collect::<Vec<_>>(),
        ledger,
        "the incremental watermark was borrowed and given back"
    );
    assert_eq!(app.folder_managed.clone(), managed, "and so was the file list");

    // Duplicating onto its own folder is a mistake, not a save.
    dispatch(&mut app, AppCmd::SaveDuplicatePath(index.clone()));
    assert!(app.status.contains("use Save"), "{}", app.status);

    let _ = std::fs::remove_dir_all(&dir);
}
