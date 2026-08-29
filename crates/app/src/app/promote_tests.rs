//! Workflow audit §11 (2026-08-30): the ネーム promotion path — "New work
//! from this work at a different dpi", and the stamp of that work's pages
//! back into the manuscript as fitted draft underlays.
//!
//! The four things that can go silently wrong, and so are pinned here:
//! the new work losing the page IDENTITIES (which is the only thing that
//! makes the later stamp land on the right page), the stamp rendering
//! through the EXPORT path and so stamping blank sheets (a ネーム is all
//! draft ink), a direct byte write leaving a parked live document to be
//! reinstalled over it, and the open page costing more than one undo
//! press.
//!
//! Same frugality rule as `new_document_tests`: 72 dpi drafts, one App at
//! a time. The stamp SOURCE is built as bytes, never as a second App —
//! two headless renderers alive at once is the memory trap `build.sh`
//! warns about.

use super::new_document_tests::{headless, scribble, small_draft};
use crate::cmd::{AppCmd, dispatch};

/// The paper a work of `pages` pages was created with.
fn new_comic(app: &mut crate::App, pages: u32) -> (u32, u32) {
    small_draft(app, pages, "Promotion");
    dispatch(app, AppCmd::NewComicCreate);
    app.page
        .as_ref()
        .expect("a new comic carries its page setup")
        .paper_px()
}

/// A one-layer page whose ONLY content is a full-bleed DRAFT layer — a
/// ネーム page, in other words. Near-black so the stamp can be told from
/// a blank sheet by colour, not merely by "a layer exists".
fn name_page_bytes(w: u32, h: u32) -> Vec<u8> {
    let mut doc = mn_core::Document::new(w, h);
    let img = image::RgbaImage::from_pixel(w, h, image::Rgba([8, 8, 8, 255]));
    let at = doc.add_layer_from_image("rough".to_owned(), &img);
    doc.layers[at].draft = true;
    // Drop the empty stock layer so the page is unambiguously "draft ink
    // only": with it present a drafts-off render could still be argued to
    // have painted something.
    if doc.layers.len() > 1 {
        let other = (0..doc.layers.len()).find(|&i| i != at).expect("two layers");
        doc.layers.remove(other);
        doc.active = 0;
    }
    mn_core::project::doc_to_bytes(&doc).expect("encode a name page")
}

/// Write a single-file `.mnc` ネーム work: `uids.len()` pages, each the
/// same full-bleed draft sheet, carrying the given page identities.
fn name_work(tag: &str, uids: &[u64], w: u32, h: u32) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mn-promote-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join(format!("{tag}.mnc"));
    let mut proj = mn_core::Project::new("ネーム".to_owned(), None, true);
    proj.meta.page_uids = uids.to_vec();
    proj.pages = uids.iter().map(|_| name_page_bytes(w, h)).collect();
    mn_core::project::save(&proj, &path).expect("write the name work");
    path
}

/// A non-open page's stashed bytes, decoded — the only truth about what a
/// direct byte write actually wrote.
fn page_doc(app: &crate::App, i: usize) -> mn_core::Document {
    let b = app.pages[i]
        .bytes
        .as_ref()
        .unwrap_or_else(|| panic!("page {} is stashed", i + 1));
    mn_core::project::bytes_to_doc(b).expect("decode the page")
}

fn draft_at(doc: &mn_core::Document) -> usize {
    doc.layers
        .iter()
        .position(|l| l.draft)
        .expect("a draft underlay")
}

/// Premultiplied (alpha sum, red sum) over a layer's tiles. A full-bleed
/// near-black stamp has a large alpha sum and a red sum near zero; a blank
/// or drafts-off render has either no alpha at all or red ≈ alpha.
fn ink(l: &mn_core::Layer) -> (u64, u64) {
    let (mut a, mut r) = (0u64, 0u64);
    for (_, t) in l.tiles() {
        for py in 0..mn_core::TILE_SIZE {
            for px in 0..mn_core::TILE_SIZE {
                let p = t.pixel(px, py);
                a += p[3] as u64;
                r += p[0] as u64;
            }
        }
    }
    (a, r)
}

/// **The first half.** A promoted work is the same chapter at another
/// resolution: same page count, same order, same paper and binding, blank
/// pages — and the same page IDENTITIES, which is the load-bearing part.
#[test]
fn new_work_from_work_keeps_count_order_uids_and_setup_at_the_new_dpi() {
    let Some(mut app) = headless() else { return };
    new_comic(&mut app, 4);
    let uids: Vec<u64> = app.pages.iter().map(|e| e.uid).collect();
    let paper_mm = app.page.as_ref().expect("setup").paper_mm;
    let binding = app.binding_right;
    let story = app.story.clone();
    let tabs = app.doc_count();

    app.promote.dpi = 36; // half of the draft comic's 72
    dispatch(&mut app, AppCmd::PromoteNewWorkApply);

    assert_eq!(
        app.doc_count(),
        tabs + 1,
        "the promotion opens a NEW TAB — the manuscript is still there"
    );
    assert_eq!(app.pages.len(), 4, "same page count");
    assert_eq!(
        app.pages.iter().map(|e| e.uid).collect::<Vec<_>>(),
        uids,
        "same page identities, in the same ORDER — this is what the stamp \
         matches on later"
    );
    let setup = app.page.as_ref().expect("the new work carries a setup");
    assert_eq!(setup.dpi, 36, "at the chosen dpi");
    assert_eq!(setup.paper_mm, paper_mm, "the same paper, in millimetres");
    assert_eq!(app.binding_right, binding);
    assert_eq!(app.story, story);
    assert!(app.doc_path.is_none(), "a promoted work is not a file yet");
    let (w, h) = setup.paper_px();
    assert_eq!(app.doc.size, (w, h), "page 1 is the new pixel size");
    assert!(
        app.pages.iter().all(|e| e.blank.is_some()),
        "every page is a still-lazy blank — promoting a chapter encodes nothing"
    );
    for (i, e) in app.pages.iter().enumerate() {
        assert_eq!(
            e.blank.map(|(bw, bh, _)| (bw, bh)),
            Some((w, h)),
            "page {} is the new paper",
            i + 1
        );
    }
    // The dpi is clamped, not trusted: a promotion cannot make a 40 GB page.
    app.promote.dpi = 99_999;
    dispatch(&mut app, AppCmd::PromoteNewWorkApply);
    assert_eq!(
        app.page.as_ref().expect("setup").dpi,
        crate::app::PromoteDraft::MAX_DPI
    );
}

/// **The second half, on the open page.** The stamp records the whole
/// stack once (the `comps.rs` pre-image pattern), so it costs exactly one
/// undo press — and the underlay lands directly ABOVE the page's White
/// base, inside the frame folder, because White paints the panel interior
/// opaque and an underlay beneath it is invisible exactly where the
/// drawing happens.
#[test]
fn stamp_lands_above_the_white_base_in_one_undo_press() {
    let Some(mut app) = headless() else { return };
    let (pw, ph) = new_comic(&mut app, 2);
    let uid0 = app.pages[0].uid;
    let src = name_work("open", &[uid0], pw, ph);

    let layers = app.doc.layers.len();
    let steps = app.doc.undo_len();
    dispatch(&mut app, AppCmd::StampNamePagesPath(src));

    assert!(
        app.status.contains("1 page(s)") && app.status.contains("matched by page identity"),
        "{}",
        app.status
    );
    assert_eq!(
        app.doc.layers.len(),
        layers + 1,
        "the underlay landed on the LIVE document, not on stashed bytes"
    );
    let u = draft_at(&app.doc);
    let w = app
        .doc
        .layers
        .iter()
        .position(|l| l.name == "White")
        .expect("a page seeded blank still has its White base");
    assert_eq!(u, w + 1, "directly above the White base, not under it");
    assert_eq!(
        app.doc.layers[u].depth, app.doc.layers[w].depth,
        "and INSIDE the frame folder, so the panel mask still applies"
    );
    assert!(
        app.doc.layers[u + 1..].iter().all(|l| !l.draft),
        "nothing above it is a draft — the ink layers still print"
    );
    assert_eq!(
        app.doc.undo_len(),
        steps + 1,
        "ONE undo step for the stamp, not one per helper call"
    );
    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(app.doc.layers.len(), layers, "and one press takes it back");
    assert!(!app.doc.layers.iter().any(|l| l.draft));
}

/// **The second half, on a page that is not open.** It rides the same
/// byte round trip the batch import does, and — the invariant workflow
/// audit #1 left behind — it BUMPS the page's content revision, so a
/// parked live document sitting in that slot is stale and the arriving
/// switch decodes what the stamp wrote instead of reinstalling the page
/// as it was.
#[test]
fn a_stamp_on_a_parked_page_rides_the_bytes_and_bumps_the_rev() {
    let Some(mut app) = headless() else { return };
    let (pw, ph) = new_comic(&mut app, 2);
    // Give page 2 real content (a still-blank template page is never
    // parked), then leave it, which parks it.
    app.switch_page(1);
    scribble(&mut app);
    app.switch_page(0);
    assert!(
        app.pages[1].parked.is_some(),
        "page 2 parked its live document on the way out"
    );
    let before = app.pages[1].rev;
    let uid1 = app.pages[1].uid;
    let src = name_work("parked", &[uid1], pw, ph);

    dispatch(&mut app, AppCmd::StampNamePagesPath(src));

    assert!(app.pages[1].rev > before, "a fresh content revision");
    assert!(app.pages[1].thumb.is_none(), "stale thumbnail dropped");
    assert!(
        app.pages[1].parked.is_some() && app.pages[1].parked_rev != app.pages[1].rev,
        "the park is still in the slot, but the bump marked it stale"
    );
    let d = page_doc(&app, 1);
    let u = draft_at(&d);
    let w = d
        .layers
        .iter()
        .position(|l| l.name == "White")
        .expect("the White base");
    assert_eq!(u, w + 1, "same placement rule off the open page");
    assert!(
        !app.doc.layers.iter().any(|l| l.draft),
        "and the OPEN page, which no name page claimed, was left alone"
    );
    // The proof that matters: arriving shows what the stamp wrote.
    app.switch_page(1);
    assert!(
        app.doc.layers.iter().any(|l| l.draft),
        "the underlay is on the page, so the stale park was NOT installed"
    );
}

/// A ネーム is drawn entirely on DRAFT layers. Rendering the source
/// through the export path (`render_offscreen_drafts_off`) would stamp a
/// blank sheet onto every manuscript page and nothing would look broken —
/// which is why this is a differential on the PIXELS, not on "a layer
/// exists".
#[test]
fn a_name_page_is_stamped_with_its_draft_ink() {
    let Some(mut app) = headless() else { return };
    let (pw, ph) = new_comic(&mut app, 1);
    let uid0 = app.pages[0].uid;
    let src = name_work("draftink", &[uid0], pw, ph);

    dispatch(&mut app, AppCmd::StampNamePagesPath(src));

    let u = draft_at(&app.doc);
    let (a, r) = ink(&app.doc.layers[u]);
    assert!(
        a > 0,
        "the stamp put pixels on the page (drafts-off would leave none)"
    );
    assert!(
        r * 4 < a,
        "and they are the ネーム's near-black ink, not a blank white sheet \
         (alpha {a}, red {r})"
    );
}

/// Identity matching is the point: a name page whose identity nothing here
/// claims is SKIPPED and counted, rather than dropped onto whatever page
/// happens to sit at its index. The page-count note rides the same line.
#[test]
fn name_pages_with_no_matching_page_are_skipped_and_counted() {
    let Some(mut app) = headless() else { return };
    let (pw, ph) = new_comic(&mut app, 3);
    // Three name pages: only the middle one names a page of this work,
    // and it names page 2 — not page 2 by position, page 2 by identity.
    let uid1 = app.pages[1].uid;
    let src = name_work("partial", &[900_001, uid1, 900_003], pw, ph);

    dispatch(&mut app, AppCmd::StampNamePagesPath(src));

    assert!(
        app.status.contains("1 page(s)") && app.status.contains("2 name page(s) had no page here"),
        "{}",
        app.status
    );
    assert!(
        app.status.contains("matched by page identity"),
        "one identity in common is enough to use identities: {}",
        app.status
    );
    assert!(
        !app.doc.layers.iter().any(|l| l.draft),
        "page 1 was not claimed and took nothing"
    );
    assert!(
        page_doc(&app, 1).layers.iter().any(|l| l.draft),
        "page 2 — the one whose identity matched — took the underlay"
    );
    assert!(
        app.pages[2].bytes.is_none() && app.pages[2].blank.is_some(),
        "page 3 was not claimed either — still the untouched lazy blank it \
         was created as, not even encoded"
    );
}

/// A work that carries NO page identities (saved before §11, or never
/// promoted) still stamps — by page ORDER — and the status line says so,
/// because that is the case where a chapter that gained a page since
/// would quietly land every ネーム one page out.
#[test]
fn a_work_with_no_identities_falls_back_to_page_order_and_says_so() {
    let Some(mut app) = headless() else { return };
    let (pw, ph) = new_comic(&mut app, 2);
    let src = name_work("legacy", &[0, 0], pw, ph);

    dispatch(&mut app, AppCmd::StampNamePagesPath(src));

    assert!(
        app.status.contains("matched by page ORDER")
            && app.status.contains("carries no page identities"),
        "{}",
        app.status
    );
    assert!(app.doc.layers.iter().any(|l| l.draft), "page 1 stamped");
    assert!(
        page_doc(&app, 1).layers.iter().any(|l| l.draft),
        "page 2 stamped"
    );
}

/// The identities have to survive the round trip through a file, or the
/// whole promotion is a same-session trick: the ネーム is drawn over
/// days, and the manuscript is reopened before it is stamped.
#[test]
fn page_identities_survive_a_save_and_reopen() {
    let Some(mut app) = headless() else { return };
    new_comic(&mut app, 3);
    let uids: Vec<u64> = app.pages.iter().map(|e| e.uid).collect();
    let dir = std::env::temp_dir().join(format!("mn-promote-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("roundtrip.mnc");

    dispatch(&mut app, AppCmd::SaveOraPath(path.clone()));
    assert!(path.exists(), "the work was written: {}", app.status);
    dispatch(&mut app, AppCmd::OpenOraPath(path));

    assert_eq!(
        app.pages.iter().map(|e| e.uid).collect::<Vec<_>>(),
        uids,
        "the reopened work knows which pages it has"
    );
    // And the mint floor moved with them: a page added now must not
    // collide with an adopted identity.
    dispatch(&mut app, AppCmd::AddPage);
    let fresh = app.pages.iter().map(|e| e.uid).collect::<Vec<_>>();
    let mut sorted = fresh.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), fresh.len(), "no two pages share an identity");
}
