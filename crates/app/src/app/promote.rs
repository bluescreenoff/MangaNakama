//! Workflow audit §11 — the ネーム promotion path.
//!
//! ネーム (the thumbnail/storyboard stage) is where the manga is actually
//! decided, and it is drawn fast, small and iteratively. The audit's
//! explicit instruction was **not** to build a "name mode": build the
//! PROMOTION, which is findings 2+3+4 composed, so the paper route and the
//! digital route become the same route.
//!
//! Two operations, and they are halves of one thing:
//!
//! * **New work from this work…** — a second work with the same page
//!   count, the same page ORDER, the same paper and binding, at a
//!   different dpi (150 for name work). Every page is blank, seeded the
//!   way New Manga seeds one.
//! * **Stamp name pages as drafts…** — each page of that work rendered
//!   (draft ink INCLUDED: a ネーム is all draft ink) and placed into the
//!   corresponding manuscript page as a fitted 下書き underlay, through
//!   the batch import's placement rule.
//!
//! **What makes the second half land on the right page** is that the
//! first half copies the source's page identities (`PageEntry::uid`) into
//! the new work one for one, and those identities are PERSISTED
//! (`mn_core::project`'s `page_uids` / `FolderPageMeta::uid`). Page 7 of
//! the ネーム therefore still knows it belongs to page 7 of the manuscript
//! after both files have been closed, reopened, and reordered — which
//! index matching cannot survive, because reordering the chapter is a
//! normal thing to do between the ネーム and the 原稿. Works saved before
//! §11 carry no identities at all, and there the stamp falls back to page
//! ORDER and says so in the status line.
//!
//! Deliberately NOT warned about: a dpi mismatch between the two works.
//! That mismatch is the entire point of the feature — the underlay is
//! scaled to the target page's own pixels either way. A page-COUNT
//! mismatch DOES get a status note: that one means pages were added or
//! deleted on one side, and the artist should know before drawing over it.

use std::path::Path;

use super::App;
use super::pages::{PageEntry, fit_to_paper, place_draft_underlay};
use mn_core::Document;

/// The "New work from this work…" dialog's working state.
#[derive(Clone)]
pub struct PromoteDraft {
    /// Target resolution. 150 is the name-work default; typing the work's
    /// OWN dpi is the "duplicate this work, empty" case, which is why the
    /// field is a free number rather than a two-way switch.
    pub dpi: u32,
}

impl PromoteDraft {
    /// Low enough for a genuinely rough ネーム, high enough that nobody
    /// can promote a chapter into a 40 GB canvas by holding a drag.
    pub const MIN_DPI: u32 = 30;
    pub const MAX_DPI: u32 = 1200;
    pub const NAME_DPI: u32 = 150;
}

impl Default for PromoteDraft {
    fn default() -> Self {
        Self {
            dpi: Self::NAME_DPI,
        }
    }
}

/// A source work read off disk for the stamp: its pages' ORA bytes, their
/// stable identities (0 = none recorded), and a label for the layer names.
struct SourceWork {
    label: String,
    uids: Vec<u64>,
    pages: Vec<Vec<u8>>,
}

/// Read a `.mnc` — either flavour — as a stamp source.
fn load_source(path: &Path) -> Result<SourceWork, String> {
    let stem = |p: &Path| {
        p.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "name".to_owned())
    };
    match mn_core::project::sniff_kind(path) {
        mn_core::project::MncKind::WorkFolderIndex => {
            let wf = mn_core::project::load_folder(path).map_err(|e| e.to_string())?;
            // Every work folder's index is called `work.mnc`; the FOLDER is
            // what the artist named, so that is the label the layers wear.
            let label = path
                .parent()
                .filter(|d| !d.as_os_str().is_empty())
                .map(stem)
                .unwrap_or_else(|| stem(path));
            Ok(SourceWork {
                label,
                uids: wf.pages.iter().map(|p| p.uid).collect(),
                pages: wf.pages.into_iter().map(|p| p.bytes).collect(),
            })
        }
        mn_core::project::MncKind::Comic => {
            let proj = mn_core::project::load(path).map_err(|e| e.to_string())?;
            let mut uids = proj.meta.page_uids.clone();
            // A short (or absent) list is a work from before §11: pad with
            // "no identity" rather than mis-pairing on a truncated list.
            uids.resize(proj.pages.len(), 0);
            Ok(SourceWork {
                label: stem(path),
                uids,
                pages: proj.pages,
            })
        }
        mn_core::project::MncKind::Unknown => Err(format!(
            "{} is not a MangaNakama work (.mnc)",
            path.display()
        )),
    }
}

/// Pair source pages with target pages. Returns the pairs as
/// `(source index, target index)` plus whether they were found by
/// IDENTITY — `false` means the order fallback ran, which the status line
/// says out loud because it is the case that can silently land page 4's
/// ネーム on page 4 of a chapter that gained a page since.
///
/// Identity wins whenever the two works have even one in common: a work
/// that was promoted carries them, and mixing the two rules inside one
/// stamp would put some pages where they belong and some where they
/// happen to sit.
fn pair_pages(src: &[u64], dst: &[u64]) -> (Vec<(usize, usize)>, bool) {
    let mut pairs = Vec::new();
    let mut taken = vec![false; dst.len()];
    for (si, &u) in src.iter().enumerate() {
        if u == 0 {
            continue;
        }
        // A duplicate identity (two pages of one work claiming the same
        // one) can only come from a hand-edited file; first free wins, and
        // the second copy simply finds no page.
        if let Some(ti) = (0..dst.len()).find(|&ti| !taken[ti] && dst[ti] == u) {
            taken[ti] = true;
            pairs.push((si, ti));
        }
    }
    if !pairs.is_empty() {
        return (pairs, true);
    }
    ((0..src.len().min(dst.len())).map(|i| (i, i)).collect(), false)
}

/// Render a whole source page WITH its draft ink, at its own pixel size.
///
/// The export path ([`super::pages::render_offscreen_drafts_off`]) is
/// exactly wrong here: a ネーム is drawn entirely on draft layers, and
/// hiding them would stamp a blank sheet onto every manuscript page —
/// the silent failure this function exists to not have.
fn render_name_page(renderer: &mut mn_gpu::Renderer, doc: &Document) -> image::RgbaImage {
    /// Long-edge cap, the reader's number and the reader's reason: a 600
    /// dpi B4 page is taller than common 8192 texture limits and its
    /// readback blows wgpu's max buffer size. A ネーム promoted at 150 dpi
    /// is nowhere near it; a same-dpi duplicate could be.
    const MAX_EDGE: u32 = 4096;
    let (dw, dh) = (doc.size.0.max(1), doc.size.1.max(1));
    let long = dw.max(dh);
    let (rw, rh) = if long > MAX_EDGE {
        let s = MAX_EDGE as f32 / long as f32;
        (
            ((dw as f32 * s).round() as u32).max(1),
            ((dh as f32 * s).round() as u32).max(1),
        )
    } else {
        (dw, dh)
    };
    // Each page is a DIFFERENT document whose layer indices mean different
    // things; the compositor caches tiles against those indices, so the
    // cache left by the previous page must not be believed.
    renderer.invalidate();
    renderer.render_offscreen(doc, rw, rh)
}

impl App {
    /// **Workflow audit §11, first half.** Build a second work with this
    /// one's page count, page ORDER, paper, binding and story at a chosen
    /// dpi, every page blank.
    ///
    /// It lands in a NEW TAB — the same `push_doc_slot` line `File ▸ New`
    /// uses, and for the same reason: the manuscript must still be there
    /// afterwards. Both works are therefore open at once, which is what
    /// the promotion is for (draw the ネーム beside the chapter). The new
    /// work has no path yet; the artist names it at the first save, and
    /// its page identities are written into that file, which is what makes
    /// [`App::stamp_name_pages`] able to find its way home later.
    ///
    /// Pages are seeded through the New Manga path — `PageEntry::blank`
    /// markers materialized by `blank_page_doc_at`, so promoting a
    /// 40-page chapter costs no ORA encoding at all until something is
    /// actually saved.
    pub fn promote_new_work(&mut self) -> String {
        self.promote_open = false;
        let Some(setup) = self.page.clone() else {
            return "new work: this work has no page setup to copy — Page ▸ Work settings…".into();
        };
        if self.pages.is_empty() {
            return "new work: this work has no pages".into();
        }
        let mut setup = setup;
        setup.dpi = self
            .promote
            .dpi
            .clamp(PromoteDraft::MIN_DPI, PromoteDraft::MAX_DPI);
        // Write the clamp back, so the dialog and the status line can never
        // disagree about what was actually built.
        self.promote.dpi = setup.dpi;
        let dpi = setup.dpi;
        let (w, h) = setup.paper_px();

        // Everything the new work inherits, read BEFORE the tab is parked.
        // `(identity, is a combined spread)` per page, in order.
        let plan: Vec<(u64, bool)> = self.pages.iter().map(|e| (e.uid, e.spread)).collect();
        let n = plan.len();
        let story = self.story.clone();
        let binding_right = self.binding_right;
        let seed_frames = self.seed_frame_folder;
        let from = self
            .doc_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned());

        // Direct-feel rule, and the same two lines `NewComicCreate` opens
        // with: a text edit or a lifted float belongs to the document about
        // to be parked, not to the one being built.
        self.commit_text_edit();
        self.commit_open_float();
        self.push_doc_slot();

        // Binding and frame seeding BEFORE any page is built: every seeded
        // frame's book side keys on them.
        self.page = setup.has_guides().then_some(setup);
        self.binding_right = binding_right;
        self.seed_frame_folder = seed_frames;
        self.story = story;
        // A combined spread is a double-width page; carrying that keeps the
        // ネーム's page ORDER — and its page NUMBERS — the chapter's.
        let spread_px = |spread: bool| {
            if spread {
                (w.saturating_mul(2).max(1), h)
            } else {
                (w, h)
            }
        };
        let (w0, h0) = spread_px(plan[0].1);
        self.doc = self.blank_page_doc_at(w0, h0, 1);
        self.pages = Vec::with_capacity(n);
        let mut number1 = 1usize;
        for (i, &(uid, spread)) in plan.iter().enumerate() {
            let (pw, ph) = spread_px(spread);
            let mut e = if i == 0 {
                PageEntry::active()
            } else {
                self.fresh_page(None, None)
            };
            // THE LINE THE SECOND HALF DEPENDS ON.
            e.uid = uid;
            e.spread = spread;
            // Lazy blank (the New Manga freeze fix): nothing is encoded
            // until a save actually needs this page's bytes.
            e.blank = Some((pw, ph, number1));
            self.pages.push(e);
            number1 += if spread { 2 } else { 1 };
        }
        self.page_index = 0;
        self.pages[0].doc_rev = self.doc.revision;
        self.set_doc_path(None);
        self.reset_folder_state();
        self.renderer.invalidate();
        self.layer_thumbs.clear();
        self.fit_to_view();
        self.mark_saved();
        self.mark_dirty();
        format!(
            "new work: {n} blank page(s) at {dpi} dpi ({w}×{h} px), same order and page \
             identities as {} — draw the ネーム here, save it, then Page ▸ Stamp name pages \
             as drafts… back in the manuscript",
            from.as_deref().unwrap_or("the work it came from")
        )
    }

    /// **Workflow audit §11, second half.** Render every page of the work
    /// at `src` and place it into the page of THIS work it belongs to, as
    /// a fitted 下書き draft underlay.
    ///
    /// Placement is the batch import's rule, through the same
    /// [`place_draft_underlay`]: directly ABOVE the page's White base and
    /// inside its frame folder (the lowest White wins), because White
    /// paints the panel interior opaque and an underlay beneath it is
    /// invisible exactly where the drawing happens. A page with no White
    /// base takes the bottom of the stack.
    ///
    /// Two doors, exactly as the batch import has: the OPEN page takes the
    /// underlay through `self.doc` with the whole stack recorded ONCE
    /// (the `comps.rs` pre-image pattern) so it costs a single undo press;
    /// every other page is decoded from its bytes, edited, re-encoded, and
    /// given a fresh content revision from `page_rev_next` — the bump is
    /// what makes a parked live document stale (`PageEntry::parked_rev`)
    /// so a later switch decodes what the stamp wrote instead of
    /// reinstalling the page as it was. Undo covers the open page only.
    pub fn stamp_name_pages(&mut self, src: &Path) -> String {
        if self.doc_path.as_deref() == Some(src) {
            return "stamp: that is this work — pick the ネーム work".into();
        }
        let source = match load_source(src) {
            Ok(s) => s,
            Err(e) => return format!("stamp: {e}"),
        };
        if source.pages.is_empty() {
            return "stamp: that work has no pages".into();
        }
        if let Err(e) = self.stash_current_page() {
            return format!("stamp: {e}");
        }
        let (pairs, by_uid) = pair_pages(&source.uids, &self.page_uids());
        let skipped = source.pages.len() - pairs.len();
        let (mut stamped, mut failed) = (0usize, 0usize);
        // ONE note for the batch, the batch import's rule: N copies of the
        // same sentence is not N times the information.
        let mut note: Option<String> = None;

        for &(si, ti) in &pairs {
            let Ok(name_doc) = mn_core::project::bytes_to_doc(&source.pages[si]) else {
                failed += 1;
                continue;
            };
            let img = render_name_page(&mut self.renderer, &name_doc);
            let layer_name = format!("{} p{}", source.label, si + 1);

            if ti == self.page_index {
                // THE OPEN PAGE. Fit to the page being drawn on, record the
                // pre-image once, then edit the stack directly: going
                // through `add_layer_from_image` + `move_layer` would push
                // two structure groups for one stamp.
                let (tw, th) = self.doc.size;
                let (fitted, n) = fit_to_paper(img, tw, th);
                note = note.or(n);
                let before = self.doc.layers.clone();
                let active_before = self.doc.active;
                self.doc
                    .record_structure("Stamp name page", before, active_before);
                place_draft_underlay(&mut self.doc, layer_name, &fitted);
                self.renderer.invalidate();
                self.layer_thumbs.clear();
                stamped += 1;
                continue;
            }

            // A still-LAZY blank page has no bytes to decode — materialize
            // its template the way `save_work_folder` and the batch import
            // both do.
            let blank = self.pages[ti].blank;
            let mut doc = match self.pages[ti].bytes.as_deref() {
                Some(b) => match mn_core::project::bytes_to_doc(b) {
                    Ok(d) => d,
                    Err(_) => {
                        failed += 1;
                        continue;
                    }
                },
                None => match blank {
                    Some((bw, bh, n1)) => self.blank_page_doc_at(bw, bh, n1),
                    None => {
                        failed += 1;
                        continue;
                    }
                },
            };
            // Fit to THIS page's own pixels, not the work's default paper:
            // a combined spread is a double-width page and half a stamp on
            // it would be worse than none.
            let (fitted, n) = fit_to_paper(img, doc.size.0, doc.size.1);
            note = note.or(n);
            place_draft_underlay(&mut doc, layer_name, &fitted);
            let Ok(nb) = mn_core::project::doc_to_bytes(&doc) else {
                failed += 1;
                continue;
            };
            let rev = self.page_rev_next();
            let e = &mut self.pages[ti];
            e.bytes = Some(nb);
            // It has real content now — the template marker is spent.
            e.blank = None;
            // THE park-staleness bump.
            e.rev = rev;
            e.doc_rev = 0;
            e.thumb = None;
            stamped += 1;
        }

        // Restore the active-page invariant (bytes live in `doc`), and drop
        // the tile cache the foreign renders left describing another
        // document.
        self.pages[self.page_index].bytes = None;
        self.renderer.invalidate();
        self.layer_thumbs.clear();
        self.mark_pages_dirty();
        self.mark_dirty();

        let mut s = format!(
            "stamp: {stamped} page(s) from {} as draft underlays, matched by page {}",
            source.label,
            if by_uid { "identity" } else { "ORDER" }
        );
        if !by_uid {
            // Two very different reasons land here, and conflating them
            // would tell the artist a comforting lie in the dangerous one.
            s.push_str(if source.uids.iter().any(|&u| u != 0) {
                " (none of that work's page identities are in this work — is it \
                 the right ネーム?)"
            } else {
                " (that work carries no page identities — it was not made by \
                 New work from this work)"
            });
        }
        if skipped > 0 {
            s.push_str(&format!(
                " — {skipped} name page(s) had no page here and were skipped"
            ));
        }
        // A dpi mismatch is the POINT and says nothing; a page-count
        // mismatch means pages were added or deleted on one side.
        if source.pages.len() != self.pages.len() {
            s.push_str(&format!(
                " — page counts differ ({} there, {} here)",
                source.pages.len(),
                self.pages.len()
            ));
        }
        if let Some(n) = note {
            s.push_str(&format!(" — {n}"));
        }
        if failed > 0 {
            s.push_str(&format!(" — {failed} page(s) could not be read"));
        }
        s
    }
}

