use super::new_document_tests::{all_ink, headless, scribble, small_draft};
use crate::cmd::{AppCmd, dispatch};

/// Opening a file must never be the gesture that loses work: with a
/// drawn-on canvas in front of you, an open lands in a NEW tab.
#[test]
fn opening_a_file_beside_unsaved_work_uses_a_new_tab() {
    let Some(mut app) = headless() else { return };
    // Something worth keeping in tab 1.
    scribble(&mut app);
    let kept = all_ink(&app);

    // Write a document to disk from a SECOND tab, then close that tab so
    // only the drawn-on one is left with a real file to open.
    small_draft(&mut app, 1, "Saved");
    dispatch(&mut app, AppCmd::NewComicCreate);
    let dir = std::env::temp_dir().join(format!("mn-opentab-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("chapter.ora");
    dispatch(&mut app, AppCmd::SaveOraPath(file.clone()));
    assert!(file.is_file(), "the fixture saved");
    assert!(app.close_doc(1));
    assert_eq!(app.doc_count(), 1);
    assert_eq!(all_ink(&app), kept, "back on the drawing");

    dispatch(&mut app, AppCmd::OpenOraPath(file.clone()));
    assert_eq!(app.doc_count(), 2, "the file opened in its own tab");
    assert!(app.switch_doc(0));
    assert_eq!(all_ink(&app), kept, "the drawing is untouched");

    std::fs::remove_dir_all(&dir).ok();
}

/// ...but an untouched blank canvas is not work, and filling the tab
/// strip with empty documents would be its own bug.
#[test]
fn opening_a_file_onto_an_untouched_blank_reuses_the_tab() {
    let Some(mut app) = headless() else { return };
    let dir = std::env::temp_dir().join(format!("mn-opentab2-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("chapter.ora");
    scribble(&mut app);
    dispatch(&mut app, AppCmd::SaveOraPath(file.clone()));

    // A fresh app, nothing drawn, nothing saved.
    drop(app);
    let Some(mut app) = headless() else { return };
    assert_eq!(app.doc_count(), 1);
    dispatch(&mut app, AppCmd::OpenOraPath(file.clone()));
    assert_eq!(app.doc_count(), 1, "no empty tab left behind");
    assert!(app.doc_path.is_some());

    std::fs::remove_dir_all(&dir).ok();
}
