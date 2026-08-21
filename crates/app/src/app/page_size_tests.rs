use super::new_document_tests::{headless, small_draft};
use crate::cmd::{AppCmd, dispatch};
use mn_core::{Document, ResizeAnchor, TileIdx};

const OPAQUE: [u16; 4] = [1, 2, 3, 32768];

/// Put / read one opaque pixel on the bottom layer — the same probe
/// `doc.rs`'s resize tests use, lifted to a `Document` a page carries.
fn put(doc: &mut Document, x: i32, y: i32) {
    let ti = TileIdx::of_pixel(x, y);
    let (ox, oy) = ti.origin();
    doc.layers[0]
        .tile_mut(ti)
        .set_pixel((x - ox) as usize, (y - oy) as usize, OPAQUE);
}

fn alpha_at(doc: &Document, x: i32, y: i32) -> u16 {
    let ti = TileIdx::of_pixel(x, y);
    let (ox, oy) = ti.origin();
    doc.layers[0]
        .tile(ti)
        .map(|t| t.pixel((x - ox) as usize, (y - oy) as usize)[3])
        .unwrap_or(0)
}

fn parked_doc(app: &crate::App, i: usize) -> Document {
    let b = app.pages[i].bytes.as_ref().expect("parked page carries bytes");
    mn_core::project::bytes_to_doc(b).expect("parked page decodes")
}

/// The single-page half of "change a work's page size after creation":
/// the anchor decides where the old content lands, and nothing is
/// resampled. The negative control matters as much as the positive one —
/// an all-pages writer that fired unconditionally would pass a test that
/// only looked at the open page.
#[test]
fn resizing_the_open_page_honours_the_anchor_and_leaves_the_others_alone() {
    let Some(mut app) = headless() else { return };
    small_draft(&mut app, 3, "Anchors");
    dispatch(&mut app, AppCmd::NewComicCreate);
    let (w, h) = app.doc.size;
    let parked_before = app.pages[1].bytes.clone();

    put(&mut app.doc, 10, 10);
    app.canvas_size_draft.w = w + 100;
    app.canvas_size_draft.h = h + 100;
    app.canvas_size_draft.anchor = ResizeAnchor::TopLeft;
    app.canvas_size_draft.all_pages = false;
    dispatch(&mut app, AppCmd::ResizeCanvasApply);

    assert_eq!(app.doc.size, (w + 100, h + 100));
    assert_eq!(alpha_at(&app.doc, 10, 10), OPAQUE[3], "top-left pins in place");
    assert!(!app.doc.can_undo(), "structural: the history is cleared");
    assert_eq!(
        app.pages[1].bytes, parked_before,
        "the box was unticked — page 2 keeps its bytes byte for byte"
    );

    // Centre, from the size it now has: the content moves by half the
    // growth on each axis.
    let (w, h) = app.doc.size;
    app.canvas_size_draft.w = w + 100;
    app.canvas_size_draft.h = h + 100;
    app.canvas_size_draft.anchor = ResizeAnchor::Center;
    dispatch(&mut app, AppCmd::ResizeCanvasApply);
    assert_eq!(app.doc.size, (w + 100, h + 100));
    assert_eq!(alpha_at(&app.doc, 60, 60), OPAQUE[3], "centred: +50 on both axes");
    assert_eq!(alpha_at(&app.doc, 10, 10), 0, "source vacated");
}

/// All pages: the parked pages are decoded, resized and re-encoded through
/// the batch bytes door. Undo covers the open page only, so what this test
/// pins is that the OTHER pages actually moved — and that the round trip
/// left the active-page invariant (bytes live in `doc`) intact.
#[test]
fn all_pages_resize_reaches_a_parked_page_and_doubles_a_spread() {
    let Some(mut app) = headless() else { return };
    small_draft(&mut app, 3, "All pages");
    dispatch(&mut app, AppCmd::NewComicCreate);
    let (w, h) = app.doc.size;

    // A combined spread parked at double width: it must take double the
    // NEW width, not be squashed to a single page.
    let spread = mn_core::project::doc_to_bytes(&Document::new(w * 2, h)).unwrap();
    let mut e = app.fresh_page(Some(spread), None);
    e.spread = true;
    app.pages.push(e);

    let rev_before = app.pages[1].rev;
    app.canvas_size_draft.w = w + 100;
    app.canvas_size_draft.h = h + 100;
    app.canvas_size_draft.anchor = ResizeAnchor::Center;
    app.canvas_size_draft.all_pages = true;
    dispatch(&mut app, AppCmd::ResizeCanvasApply);

    assert_eq!(app.doc.size, (w + 100, h + 100), "the open page too");
    assert!(
        app.pages[app.page_index].bytes.is_none(),
        "active-page invariant restored: bytes live in `doc`"
    );
    for i in [1, 2] {
        assert_eq!(
            parked_doc(&app, i).size,
            (w + 100, h + 100),
            "page {} was resized in place",
            i + 1
        );
        assert!(app.pages[i].thumb.is_none(), "page {}: thumbnail dropped", i + 1);
    }
    assert!(app.pages[1].rev > rev_before, "fresh content revision");
    assert_eq!(
        parked_doc(&app, 3).size,
        ((w + 100) * 2, h + 100),
        "the spread stayed a spread"
    );
}

/// The default a NEW page inherits has to move with the work, or the next
/// Add Page silently re-introduces the old geometry — the half of this
/// feature that lives in the manifest rather than in any document.
#[test]
fn all_pages_resize_moves_the_default_for_pages_added_later() {
    let Some(mut app) = headless() else { return };
    small_draft(&mut app, 2, "Defaults");
    dispatch(&mut app, AppCmd::NewComicCreate);
    let (w, h) = app.doc.size;
    assert_eq!(
        app.page.as_ref().map(|s| s.paper_px()),
        Some((w, h)),
        "the work starts out agreeing with its pages"
    );

    app.canvas_size_draft.w = w + 100;
    app.canvas_size_draft.h = h + 100;
    app.canvas_size_draft.anchor = ResizeAnchor::Center;
    app.canvas_size_draft.all_pages = true;
    dispatch(&mut app, AppCmd::ResizeCanvasApply);

    assert_eq!(
        app.page.as_ref().map(|s| s.paper_px()),
        Some((w + 100, h + 100)),
        "the work's paper followed (px→mm→px round trip)"
    );
    dispatch(&mut app, AppCmd::AddPage);
    assert_eq!(app.doc.size, (w + 100, h + 100), "the added page is the new size");
}
