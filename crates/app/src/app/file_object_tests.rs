//! Row 166 file objects, app half: the command wiring, the refresh doors
//! and what the Layers palette row says about a broken link.
//!
//! The document rules (derive, refresh, relink, undo, the fit box) are
//! pinned in `mn_core::file_object`'s own tests, which need no GPU. What
//! can only be tested here is the part that goes through `App`: the
//! commands, the status lines, and the row.

use super::new_document_tests::headless;
use crate::cmd::{AppCmd, dispatch};
use std::path::{Path, PathBuf};

/// A flat opaque PNG on disk. Opaque so "did the picture change?" is a
/// real question about pixels rather than about alpha.
fn png(tag: &str, rgb: [u8; 3]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mn-fo-app-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let p = dir.join(format!("{tag}.png"));
    write(&p, rgb);
    p
}

fn write(p: &Path, rgb: [u8; 3]) {
    image::RgbaImage::from_pixel(80, 80, image::Rgba([rgb[0], rgb[1], rgb[2], 255]))
        .save(p)
        .expect("write the test png");
    // The change test is (mtime, length) and both files here are the same
    // length: without a gap a rewrite inside one millisecond compares equal.
    std::thread::sleep(std::time::Duration::from_millis(5));
}

/// The straight-RGB the CPU composite shows at the canvas centre.
fn centre(app: &crate::App) -> [u8; 3] {
    let img = mn_core::export::composite(&app.doc, mn_core::Background::Transparent);
    let px = img.get_pixel(app.doc.size.0 / 2, app.doc.size.1 / 2).0;
    [px[0], px[1], px[2]]
}

/// `FO-001` end to end through the command: the layer lands, the composite
/// shows the file, the row is marked as a file object, and it is ONE undo
/// press — the property that makes the import feel like a single act.
#[test]
fn import_file_object_places_the_image_and_undoes_in_one_press() {
    let Some(mut app) = headless() else { return };
    let src = png("import", [210, 40, 50]);
    let layers = app.doc.layers.len();

    dispatch(&mut app, AppCmd::ImportFileObjectPath(src.clone()));

    assert_eq!(app.doc.layers.len(), layers + 1, "the layer landed");
    let at = app.doc.active;
    assert_eq!(
        app.doc.layers[at].file_object().map(|f| f.path.clone()),
        Some(src),
        "the layer keeps the link"
    );
    assert_eq!(centre(&app), [210, 40, 50], "and the page shows the file");

    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(app.doc.layers.len(), layers, "one press takes it back");
}

/// `FO-008`: the explicit Update command re-reads a changed source; an
/// unchanged one says so rather than staying silent (a command you pressed
/// on purpose must answer).
#[test]
fn update_file_objects_picks_up_a_changed_source() {
    let Some(mut app) = headless() else { return };
    let src = png("update", [10, 200, 60]);
    dispatch(&mut app, AppCmd::ImportFileObjectPath(src.clone()));
    assert_eq!(centre(&app), [10, 200, 60]);

    dispatch(&mut app, AppCmd::UpdateFileObjects);
    assert!(
        app.status.contains("up to date"),
        "an unchanged source still answers: {:?}",
        app.status
    );

    // The artist redrew the background in the other app.
    write(&src, [30, 60, 220]);
    dispatch(&mut app, AppCmd::UpdateFileObjects);
    assert_eq!(centre(&app), [30, 60, 220], "the page re-read the file");
    assert!(
        app.status.contains("updated"),
        "and said so: {:?}",
        app.status
    );
}

/// The focus-regain door is the automatic half of the same story, and it is
/// SILENT when nothing changed — it runs on every alt-tab.
#[test]
fn regaining_focus_refreshes_quietly() {
    let Some(mut app) = headless() else { return };
    let src = png("focus", [10, 200, 60]);
    dispatch(&mut app, AppCmd::ImportFileObjectPath(src.clone()));
    app.set_status("something else entirely");
    app.refresh_file_objects_quiet();
    assert_eq!(
        app.status,
        "something else entirely",
        "an idle alt-tab must not talk"
    );

    write(&src, [220, 60, 30]);
    app.refresh_file_objects_quiet();
    assert_eq!(centre(&app), [220, 60, 30]);
    assert!(app.status.contains("updated"), "{:?}", app.status);
}

/// Row 166 door 4, the paid deferral: `set_doc_path` fires once per WORK,
/// so a page hop inside a work folder used to miss a changed source until
/// the next alt-tab. The hop itself is the arrival moment now — and it
/// keeps the quiet discipline: a hop with nothing changed leaves the
/// "page N" line alone.
#[test]
fn a_page_hop_picks_up_a_changed_source() {
    let Some(mut app) = headless() else { return };
    super::new_document_tests::small_draft(&mut app, 2, "PageHop");
    dispatch(&mut app, AppCmd::NewComicCreate);
    let src = png("pagehop", [10, 200, 60]);
    dispatch(&mut app, AppCmd::ImportFileObjectPath(src.clone()));

    // The artist redrew the background while page 2 was open.
    app.switch_page(1);
    write(&src, [220, 60, 30]);
    app.switch_page(0);
    assert_eq!(
        centre(&app),
        [220, 60, 30],
        "the hop itself re-read the changed source"
    );
    assert!(app.status.contains("updated"), "{:?}", app.status);

    // And the quiet half: a hop with nothing changed does not talk over
    // the page line.
    app.switch_page(1);
    assert!(
        app.status.starts_with("page "),
        "an idle hop keeps its own status: {:?}",
        app.status
    );
}

/// The watcher's watch set: every file-object link on the active page,
/// resolved where `resolve` already finds it — and a link that resolves
/// NOWHERE still hands over its raw path, because a restore at the
/// original location is the repair a wake should catch.
#[test]
fn watch_links_cover_resolved_and_broken_references() {
    let Some(mut app) = headless() else { return };
    let src = png("watchset", [9, 9, 9]);
    dispatch(&mut app, AppCmd::ImportFileObjectPath(src.clone()));
    assert_eq!(app.file_object_watch_links(), vec![src.clone()]);

    // Broken: gone from its home and not beside the work (no doc path) —
    // the raw absolute path is still the honest watch answer.
    std::fs::remove_file(&src).expect("delete the source");
    app.refresh_file_objects_quiet();
    assert_eq!(
        app.file_object_watch_links(),
        vec![src],
        "a broken link still watches its original folder"
    );
}

/// A vanished source: the page keeps its picture, the row is flagged, and
/// `FO-009` repairs it — all without a modal anywhere.
#[test]
fn a_missing_source_flags_the_row_and_relink_repairs_it() {
    let Some(mut app) = headless() else { return };
    let a = png("gone", [210, 40, 50]);
    let b = png("replacement", [40, 210, 50]);
    dispatch(&mut app, AppCmd::ImportFileObjectPath(a.clone()));
    let at = app.doc.active;

    std::fs::remove_file(&a).expect("delete the source");
    app.refresh_file_objects_quiet();

    assert!(
        app.doc.layers[at].file_object().unwrap().missing,
        "the row can say the link is broken"
    );
    assert_eq!(centre(&app), [210, 40, 50], "the last picture is still there");
    assert!(app.status.contains("missing"), "{:?}", app.status);

    // The picker's guard: the command is offered on every row, so the
    // "is that a file object?" answer must come before any dialog.
    assert_eq!(app.relink_target(Some(at)), Some(at));
    let plain = app.doc.add_layer("plain");
    assert_eq!(app.relink_target(Some(plain)), None);

    app.doc.set_active(at);
    dispatch(&mut app, AppCmd::RelinkFileObjectPath(at, b.clone()));
    assert_eq!(centre(&app), [40, 210, 50], "re-derived from the new file");
    let fo = app.doc.layers[at].file_object().unwrap();
    assert_eq!(fo.path, b);
    assert!(!fo.missing);
}

/// The derived raster is not hand-editable, through the REAL stroke path:
/// a pen stroke on a file object must leave the picture exactly as the file
/// drew it. (`row_glyph` and the per-type tool bar are pinned in
/// `ui::layers`' own tests, where those two are visible.)
#[test]
fn a_file_object_refuses_the_brush() {
    let Some(mut app) = headless() else { return };
    let src = png("refuse", [1, 2, 3]);
    dispatch(&mut app, AppCmd::ImportFileObjectPath(src));
    let at = app.doc.active;
    assert!(!app.doc.layers[at].paintable(), "raster edits refuse");

    let before = centre(&app);
    super::new_document_tests::scribble(&mut app);
    assert_eq!(
        centre(&app),
        before,
        "a stroke on a file object changes nothing — the next refresh would \
         throw it away, so it never lands"
    );
}

/// The whole point of the row: a background placed as a file object, saved,
/// reopened, and picked up again after the source changed. Also the
/// no-source case — the file opens and shows the picture on a machine that
/// has never seen the background.
#[test]
fn a_file_object_survives_save_and_reload_and_still_updates() {
    let Some(mut app) = headless() else { return };
    let src = png("chapter-bg", [200, 100, 20]);
    dispatch(&mut app, AppCmd::ImportFileObjectPath(src.clone()));

    let bytes = mn_core::project::doc_to_bytes(&app.doc).expect("save");
    app.doc = mn_core::project::bytes_to_doc(&bytes).expect("load");
    let at = app
        .doc
        .layers
        .iter()
        .position(|l| l.file_object().is_some())
        .expect("the link survived the round trip");
    assert_eq!(centre(&app), [200, 100, 20], "so did the picture");

    write(&src, [20, 100, 200]);
    app.refresh_file_objects_quiet();
    assert_eq!(centre(&app), [20, 100, 200], "and the link is still live");

    // The other machine: same bytes, source nowhere to be found.
    std::fs::remove_file(&src).expect("delete the source");
    let bytes = mn_core::project::doc_to_bytes(&app.doc).expect("save");
    app.doc = mn_core::project::bytes_to_doc(&bytes).expect("load");
    assert_eq!(centre(&app), [20, 100, 200], "the page still opens right");
    app.refresh_file_objects_quiet();
    assert!(
        app.doc.layers[at].file_object().unwrap().missing,
        "and it is honest about the link"
    );
    assert_eq!(centre(&app), [20, 100, 200], "without losing the picture");
}
