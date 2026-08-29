use super::new_document_tests::{headless, small_draft};
use crate::cmd::{AppCmd, dispatch};

/// A flat opaque PNG on disk. Opaque on purpose: an all-transparent image
/// would pass "the page is the right size" while proving nothing about
/// where the pixels went.
fn png(tag: &str, w: u32, h: u32) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mn-import-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let p = dir.join(format!("{tag}.png"));
    image::RgbaImage::from_pixel(w, h, image::Rgba([20, 30, 40, 255]))
        .save(&p)
        .expect("write the test png");
    p
}

fn has_ink(l: &mn_core::Layer) -> bool {
    l.tiles().any(|(_, t)| t.alpha_sum() > 0)
}

/// The paper a work of `pages` pages was created with.
fn new_comic(app: &mut crate::App, pages: u32) -> (u32, u32) {
    small_draft(app, pages, "Import");
    dispatch(app, AppCmd::NewComicCreate);
    app.page
        .as_ref()
        .expect("a new comic carries its page setup")
        .paper_px()
}

/// Workflow audit #2. A photographed ネーム used to become a page of the
/// PHOTO's size — a foreign-paper page in the middle of the chapter, whose
/// content then exported as art. The import must inherit the work's paper
/// and land the photo as a draft underlay at the bottom of the stack.
#[test]
fn an_imported_page_takes_the_works_paper_and_lands_as_a_draft() {
    let Some(mut app) = headless() else { return };
    let paper = new_comic(&mut app, 2);
    let before = app.pages.len();

    // Deliberately a different SHAPE as well as a different size.
    let p = png("phone-photo", 300, 424);
    dispatch(&mut app, AppCmd::ImportPagePath(p));

    assert_eq!(app.pages.len(), before + 1, "the page was inserted");
    assert_eq!(
        app.doc.size, paper,
        "the imported page is the WORK's paper, not the image's"
    );
    assert!(
        app.doc.layers[0].draft,
        "the underlay is a draft layer: on screen, never in the export"
    );
    assert!(
        has_ink(&app.doc.layers[0]),
        "the photo actually landed on that layer"
    );
    assert!(
        app.doc.layers[1..].iter().all(|l| !l.draft),
        "only the underlay is a draft — the drawing layers above it print"
    );
    // The part-19 trap: tiles existing proves nothing about the SCREEN.
    // The seeded folder's White base hides everything below the folder
    // across the panel interior, so the imported page's folder must skip
    // it (CSP's "Fill inside the frame" off) or the underlay is invisible
    // exactly where you draw. A blank page keeps its White (control).
    assert!(
        !app.doc.layers.iter().any(|l| l.name == "White"),
        "an imported page's frame folder has no White base over the underlay"
    );
    dispatch(&mut app, AppCmd::AddPage);
    assert!(
        app.doc.layers.iter().any(|l| l.name == "White"),
        "a blank page still seeds the White base"
    );
}

/// The aspect mismatch is a fact the human has to decide about (crop the
/// photo, or accept margins), so it goes in the status line — reshaping the
/// chapter's paper around one photo is the thing we refuse to do.
#[test]
fn an_imported_page_of_a_different_shape_says_so() {
    let Some(mut app) = headless() else { return };
    let (pw, ph) = new_comic(&mut app, 1);

    dispatch(&mut app, AppCmd::ImportPagePath(png("wrong-shape", 300, 300)));
    assert!(
        app.status.contains("not the page's shape"),
        "the mismatch must be announced, not swallowed: {}",
        app.status
    );

    // The negative control: an image already on the paper's shape says
    // nothing, or the note would be noise on every well-prepared scan.
    dispatch(&mut app, AppCmd::ImportPagePath(png("right-shape", pw, ph)));
    assert!(
        !app.status.contains("not the page's shape"),
        "an image that fits the paper exactly gets no complaint: {}",
        app.status
    );
}

/// A work with no `PageSetup` is a plain canvas, not a manga project —
/// there is no paper to inherit, so the old image-sized page stands. This
/// is the guard on the fix above, not a feature of it.
#[test]
fn a_plain_canvas_work_still_imports_the_image_at_its_own_size() {
    let Some(mut app) = headless() else { return };
    assert!(app.page.is_none(), "the boot document is a plain canvas");

    dispatch(&mut app, AppCmd::ImportPagePath(png("plain", 200, 260)));
    assert_eq!(app.doc.size, (200, 260), "the image's own pixel size");
}

/// Workflow audit #3a. An image wider or taller than the page used to be
/// centred and CLIPPED by `add_layer_from_image` — the overhang was simply
/// gone, with nothing said.
#[test]
fn importing_a_layer_scales_an_oversized_image_to_fit() {
    let Some(mut app) = headless() else { return };
    let (pw, ph) = new_comic(&mut app, 1);

    dispatch(
        &mut app,
        AppCmd::ImportImagePath(png("huge", pw * 2, ph * 2)),
    );
    assert!(
        app.status.contains(&format!("scaled to {pw}x{ph}")),
        "an oversized import is fitted to the page, and says so: {}",
        app.status
    );

    // Never enlarged: scaling a small asset up is a guess, and the
    // placement transform is where a human makes that guess.
    let (sw, sh) = (pw / 4, ph / 4);
    dispatch(&mut app, AppCmd::ImportImagePath(png("small", sw, sh)));
    assert!(
        !app.status.contains("scaled to"),
        "a small image keeps its pixels: {}",
        app.status
    );
}

/// Workflow audit #3b. Placement is a gesture, not a guess: the layer lands
/// and the transform we already have is armed, so the first thing the user
/// does is put the image where it goes.
#[test]
fn importing_a_layer_arms_the_placement_transform() {
    let Some(mut app) = headless() else { return };
    let (pw, ph) = new_comic(&mut app, 1);

    dispatch(&mut app, AppCmd::ImportImagePath(png("place", pw / 3, ph / 3)));
    assert!(
        app.transform_drag.is_some(),
        "the transform is armed for placement: {}",
        app.status
    );
    assert!(
        app.status.contains("Enter commits"),
        "and the status line says how to finish it: {}",
        app.status
    );
}

/// Workflow audit #3c. The draft route (a second File-menu item — the
/// import is a bare OS picker with nowhere to hang a checkbox) sets the
/// flag, and IO-043's selection-as-mask half is untouched by any of it.
#[test]
fn the_draft_import_route_flags_the_layer() {
    let Some(mut app) = headless() else { return };
    let (pw, ph) = new_comic(&mut app, 1);

    dispatch(
        &mut app,
        AppCmd::ImportImageDraftPath(png("rough", pw / 3, ph / 3)),
    );
    let at = app.doc.active;
    assert!(
        app.doc.layers[at].draft,
        "imported as a draft: {}",
        app.status
    );
    assert!(has_ink(&app.doc.layers[at]), "with the image on it");
}
