use super::*;
use crate::cmd::{AppCmd, dispatch};

/// An App on a headless renderer — `None` only where a test may legally
/// skip; see [`headless_renderer`] for when that is and is not allowed.
pub(super) fn headless() -> Option<App> {
    Some(App::new(headless_renderer()?, (1280, 860), 1.0))
}

pub(super) fn all_ink(app: &App) -> u64 {
    app.doc
        .layers
        .iter()
        .flat_map(|l| l.tiles())
        .map(|(_, t)| t.alpha_sum())
        .sum()
}

pub(super) fn scribble(app: &mut App) {
    app.begin_stroke(PointerKind::Mouse);
    // SCREEN coordinates: `push_batch` runs them through
    // `viewport.to_canvas` itself (live pointer batches are client-space).
    // Aim near the middle of the 1280×860 view or the stroke lands off
    // the page and silently inks nothing — which is exactly how the first
    // version of this test failed.
    let batch: Vec<PenSample> = (0..40)
        .map(|i| PenSample {
            x: 520.0 + i as f32 * 6.0,
            y: 430.0,
            pressure: 0.9,
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_ms: i as f64 * 8.0,
        })
        .collect();
    app.push_batch(&batch);
    app.end_stroke();
}

/// A small new comic — see the module note on why the dpi is turned down.
pub(super) fn small_draft(app: &mut App, pages: u32, story: &str) {
    app.new_doc_draft.setup.dpi = 72;
    app.new_doc_draft.pages = pages;
    app.new_doc_draft.story = story.into();
}

/// The owner reported that a line drawn before `File ▸ New` was still on
/// the page afterwards. This is the DIFFERENTIAL that settles it: build
/// the same new comic twice, once after drawing and once from a clean
/// start, and compare the ink.
///
/// It HAS to be differential, because a new comic is not empty — it
/// seeds a frame folder whose page-sized raster dwarfs any stroke. A
/// naive "assert no ink" reads that frame as leftover art and fails for
/// the wrong reason. (It did, on the first attempt at this test. The
/// number it printed was a full page of opaque pixels.)
#[test]
fn a_new_project_is_identical_whether_or_not_you_drew_first() {
    // One app at a time: the first is dropped before the second exists.
    let clean = {
        let Some(mut app) = headless() else { return };
        small_draft(&mut app, 1, "");
        dispatch(&mut app, AppCmd::NewComicCreate);
        all_ink(&app)
    };
    let drew = {
        let Some(mut app) = headless() else { return };
        scribble(&mut app);
        assert!(all_ink(&app) > 0, "the scribble must land first");
        small_draft(&mut app, 1, "");
        dispatch(&mut app, AppCmd::NewComicCreate);
        assert!(app.doc_path.is_none(), "a new comic is not a file yet");
        all_ink(&app)
    };
    assert_eq!(
        drew, clean,
        "a new project carried ink over from the document before it"
    );
}

/// The stashed pages must be as clean as the visible one — a leftover on
/// page 2 would only surface when the owner switched to it.
#[test]
fn every_page_of_a_new_comic_matches_its_book_side() {
    // Was "every page matches the first" — that exact uniformity WAS a
    // bug (owner 2026-08-22): a setup with a binding offset must seed
    // FACING frames mirrored (ノド/小口 swap sides page by page), so page
    // 2 — the right page of a right-bound book — legitimately differs
    // from pages 1 and 3, which are left pages. Same-side pages still
    // match exactly, which keeps the original leak-detection value.
    let Some(mut app) = headless() else { return };
    scribble(&mut app);
    small_draft(&mut app, 4, "");
    // An offset-carrying setup, dpi dropped after so the test stays small.
    app.new_doc_draft.setup = mn_core::page::PageSetup::presets()
        .into_iter()
        .find(|p| p.name.contains("Shueisha"))
        .expect("offset preset");
    app.new_doc_draft.setup.dpi = 72;
    assert!(app.new_doc_draft.setup.inner_offset_mm.0 > 0.0);
    app.new_doc_draft.binding_right = true;
    dispatch(&mut app, AppCmd::NewComicCreate);
    assert_eq!(app.pages.len(), 4);

    let ink_of = |app: &mut App, i: usize| {
        dispatch(app, AppCmd::SelectPage(i));
        all_ink(app)
    };
    let p1 = ink_of(&mut app, 0);
    let p2 = ink_of(&mut app, 1);
    let p3 = ink_of(&mut app, 2);
    let p4 = ink_of(&mut app, 3);
    assert_eq!(p1, p3, "pages 1 and 3 are both left pages");
    assert_eq!(p2, p4, "pages 2 and 4 are both right pages");
    assert_ne!(p1, p2, "facing pages mirror the binding offset");
}
