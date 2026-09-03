//! Comic pages: the page-slot types (`PageEntry`, dialog drafts) and the
//! App methods that stash/switch/create/import pages. Page geometry lives
//! in `mn_core::page`; the Pages PANEL lives in `ui`.

use super::App;
use mn_core::{Document, PageSetup, ResizeAnchor};

/// One page of the comic. The page being edited lives in `App::doc` and has
/// `bytes: None`; every other page is kept as encoded ORA bytes
/// (`mn_core::project` currency) plus a thumbnail for the Pages panel.
pub struct PageEntry {
    pub bytes: Option<Vec<u8>>,
    /// LAZY BLANK (owner freeze report 2026-08-26): a page that is still
    /// the work's own untouched template — (w, h, 1-based page number, for
    /// the book side). Never encoded until it must be (a folder save); a
    /// switch to it materializes the blank DOC directly, which is a
    /// build, not a 40-second ORA walk of a B4 page.
    pub blank: Option<(u32, u32, usize)>,
    pub thumb: Option<egui::TextureHandle>,
    /// Stable RUNTIME identity — this page, for as long as the session
    /// holds it, whatever index it drifts to. Caches key on it instead of
    /// the index: the reader's texture map was keyed by index and served
    /// the previous occupant's art after a reorder, because `rev` cannot
    /// tell two pages apart (a single-file `.mnc` loads every page at
    /// revision 0, and cmd.rs already warns about coincidental matches).
    ///
    /// PERSISTED since workflow audit §11 (`mn_core::project`'s
    /// `ProjectMeta::page_uids` / `FolderPageMeta::uid`): the ネーム
    /// promotion copies a work's page identities into the new work, and
    /// the stamp back onto the manuscript matches on them — which only
    /// means anything if they survive being saved and reopened. A work
    /// saved before §11 (or any page whose stored uid is 0) still gets a
    /// fresh runtime identity on load; uniqueness WITHIN a work is what
    /// the park LRU and the reader's texture map need, and
    /// [`PageEntry::bump_uid_floor`] keeps a later mint from colliding
    /// with an adopted one. Two DIFFERENT works may legitimately share
    /// identities — that is the promotion — and nothing reads a uid
    /// across works (`forget_document_caches` clears the reader map on
    /// every tab switch).
    pub uid: u64,
    /// Stable work-folder file identity (`pNNN.ora`); 0 until the first
    /// folder save assigns one — order changes never rename files.
    pub id: u32,
    /// Content revision; bumped whenever this page's bytes change.
    pub rev: u64,
    /// Revision already on disk in the work folder (skip-write hint).
    pub saved_rev: u64,
    /// Revision already written by the TEMP autosave folder (05 item 1).
    /// A temp write must NEVER advance `saved_rev` — that watermark means
    /// "safe in the work's real home", and advancing it here would make
    /// the next real Save As skip pages it believes are saved.
    pub autosaved_rev: u64,
    /// Revision the last export of this page WROTE (0 = this page has
    /// never been exported). `rev > exported_rev` is the whole of the
    /// unexported-pages reminder — see [`unexported_pages`]. Persisted
    /// with the work-folder index, defaulted for older works.
    pub exported_rev: u64,
    /// `doc.revision` at the moment `bytes` were encoded — makes stashing a
    /// no-op (and a re-encode bump-free) while the page was not edited.
    pub doc_rev: u64,
    /// TRIAGE 143: this page is a COMBINED spread (runtime flag — a reload
    /// from the work folder sees only a wide page; Split still works, it
    /// just halves whatever it finds). Badges the Pages panel row.
    pub spread: bool,
    /// Owner preview tier (2026-08-18): the decoded sharp preview (gray-8,
    /// EXPORT rules — drafts off), keyed on the page rev; LRU-evicted via
    /// `App::preview_order` beyond 32 pages. Rides the entry so reorders
    /// move it with the page.
    pub preview_img: Option<(u64, std::sync::Arc<image::GrayImage>)>,
    /// Display-size texture minted from the preview, + the cell height and
    /// page rev it was built at (re-mint on >25% drift or a rev move).
    pub prev_tex: Option<egui::TextureHandle>,
    pub prev_tex_px: f32,
    pub prev_tex_rev: u64,
    /// Canvas size (px), filled lazily from the stashed bytes (stack.xml
    /// only — no pixel decode). The reader's 1:1 moiré view needs each
    /// page's TRUE size; a combined spread is a wider page.
    pub canvas: Option<(u32, u32)>,
    /// Docking 2 phase 2: the display-size texture a PAGE PANE renders
    /// (ui/dock.rs). Deliberately SEPARATE from `prev_tex`: that one is
    /// keyed to the Pages palette's cell size, and a pane at 800px sharing
    /// it would fight the palette's 180px cell over the >25%-drift rule,
    /// re-minting every frame forever.
    pub pane_tex: Option<egui::TextureHandle>,
    pub pane_tex_px: f32,
    pub pane_tex_rev: u64,
    /// Workflow-audit #1: the page's LIVE `Document`, parked on
    /// switch-away beside the encoded bytes — a return to this page
    /// reinstalls it with undo history and revisions intact, instead of
    /// decoding a history-less copy. Only the most recent few pages hold
    /// one (`App::page_park_lru`); the bytes stay the one truth for
    /// save / export / preflight, so eviction costs only the history.
    pub parked: Option<Box<Document>>,
    /// `rev` at the moment `parked` was stored. Every direct byte writer
    /// (story editor, batch ops, resize-other-pages) bumps `rev` through
    /// `page_rev_next`, so a mismatch on arrival means the bytes moved on
    /// without the parked document — it is stale and must be dropped.
    pub parked_rev: u64,
}

/// The process-wide page-identity mint (see [`PageEntry::uid`]).
static UID_CLOCK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl PageEntry {
    /// The next runtime page identity. A process-wide counter rather than
    /// an App field so EVERY construction path gets one — including the
    /// `..PageEntry::active()` shorthands — and pages from two open tabs
    /// never collide. Starts at 1: 0 means "no such page".
    pub fn next_uid() -> u64 {
        UID_CLOCK.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// Lift the mint counter above every identity just ADOPTED from disk.
    ///
    /// Persisted uids (workflow audit §11) arrive from a previous session,
    /// where the counter started at 1 again. Without this, opening a saved
    /// 10-page work and then adding a page would mint uid 1 a second time
    /// — two pages of ONE work sharing an identity, which is exactly what
    /// the park LRU (`find(|e| e.uid == evict)`) and the reader's texture
    /// map cannot survive.
    pub fn bump_uid_floor(seen: u64) {
        UID_CLOCK.fetch_max(seen.saturating_add(1), std::sync::atomic::Ordering::Relaxed);
    }

    pub fn active() -> Self {
        Self {
            bytes: None,
            blank: None,
            thumb: None,
            uid: Self::next_uid(),
            id: 0,
            rev: 0,
            saved_rev: 0,
            autosaved_rev: 0,
            exported_rev: 0,
            doc_rev: 0,
            spread: false,
            preview_img: None,
            prev_tex: None,
            prev_tex_px: 0.0,
            prev_tex_rev: 0,
            canvas: None,
            pane_tex: None,
            pane_tex_px: 0.0,
            pane_tex_rev: 0,
            parked: None,
            parked_rev: 0,
        }
    }
}

/// Pages whose content moved since an export last wrote them — the whole
/// arithmetic of the unexported-pages reminder (owner ask 2026-08-22).
///
/// A work NOBODY has exported yet answers 0, on purpose: a chip on a work
/// that has never left the app is nagging, not reminding.
///
/// `live` is the page being edited and its document revision, when the
/// caller has one (a parked tab does not). The active page's `rev` only
/// moves when it stashes, so without this the reminder would wait for a
/// page switch to notice the two panels you just fixed.
pub(super) fn unexported_pages(pages: &[PageEntry], live: Option<(usize, u64)>) -> usize {
    if !pages.iter().any(|e| e.exported_rev > 0) {
        return 0;
    }
    pages
        .iter()
        .enumerate()
        .filter(|(i, e)| {
            e.rev > e.exported_rev || live.is_some_and(|(li, drev)| li == *i && drev != e.doc_rev)
        })
        .count()
}

/// TRIAGE 143: which spread operation the dialog is editing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SpreadOp {
    Combine,
    Split,
}

/// The New Manga dialog's working state.
#[derive(Clone)]
pub struct NewComicDraft {
    pub setup: PageSetup,
    pub pages: u32,
    pub binding_right: bool,
    pub story: String,
    /// Seed every page with a CSP-style frame border folder (mask + White
    /// layer + draw layer).
    pub frame_folder: bool,
}

impl Default for NewComicDraft {
    fn default() -> Self {
        Self {
            setup: PageSetup::presets().remove(0), // Shueisha A (Jump)
            pages: 1,
            binding_right: true,
            story: String::new(),
            frame_folder: true,
        }
    }
}

/// The Work Settings dialog's working state (edit after creation).
#[derive(Clone)]
pub struct WorkSettingsDraft {
    pub setup: PageSetup,
    pub binding_right: bool,
    pub story: String,
    /// Print story title + page number in the margins (outside the trim).
    pub print_margin_info: bool,
    /// Draw トンボ (register marks) in the paper margin on export.
    pub print_crop_marks: bool,
    /// Expression colour (preflight), spine width mm, cover page index.
    pub expression: mn_core::Expression,
    pub spine_mm: f32,
    pub cover: Option<usize>,
    /// Publisher/printer target. Picking one in the dialog restates
    /// `setup` + `binding_right` from it — through the draft, so the
    /// dialog's own Apply/geometry flow stays the only door.
    pub profile: Option<mn_core::profile::PublisherProfile>,
}

impl Default for WorkSettingsDraft {
    fn default() -> Self {
        Self {
            setup: PageSetup::presets().remove(0),
            binding_right: true,
            story: String::new(),
            print_margin_info: false,
            print_crop_marks: false,
            expression: mn_core::Expression::Mono,
            spine_mm: 0.0,
            cover: None,
            profile: None,
        }
    }
}

/// The Batch Import dialog's working state (workflow audit #4).
///
/// `start` is a 1-based PAGE SLOT — the Pages panel's row, the same index
/// `switch_page` takes — not the reading-order number a combined spread
/// doubles. The dialog counts slots because that is what the import fills.
#[derive(Default, Clone)]
pub struct BatchImportDraft {
    /// The picked files, already sorted by name: the order they become pages.
    pub files: Vec<std::path::PathBuf>,
    /// 1-based page slot the FIRST file lands on (defaults to the open page).
    pub start: usize,
    /// I03: what the last run actually wrote -- (page index, source file)
    /// for every underlay it placed. `App::batch_import_replay` re-reads
    /// these files, which is what lets one hand-made placement on the open
    /// page be stamped onto the rest of the chapter.
    pub placed: Vec<(std::path::PathBuf, usize)>,
}

/// The Change Canvas Size dialog's working state (CSP 基準位置 anchor).
#[derive(Clone, Copy)]
pub struct CanvasSizeDraft {
    pub w: u32,
    pub h: u32,
    pub anchor: ResizeAnchor,
    /// Resize EVERY page of the work, not only the open one, and move the
    /// work's default page size with it. Undo still covers the open page
    /// only — the others are written directly (see [`App::resize_other_pages`]).
    pub all_pages: bool,
}

/// The Change Work Resolution dialog's working state (`IO-060`, workflow
/// audit §10).
///
/// A dpi and a kernel, and that is the whole op: the paper stays the paper.
/// CSP's dialog interlocks W / H / resolution because CSP is resampling ONE
/// image; a work has as many pixel sizes as it has pages (a spread is
/// double-width), so the only number that means the same thing on all of
/// them is the resolution. The px consequence is stated per the work's own
/// page setup instead of being typed in.
#[derive(Clone)]
pub struct ResampleWorkDraft {
    pub dpi: u32,
    pub interp: mn_core::transform::Interp,
    /// W01: the paper to move TO, or `None` to keep the work's own. Only
    /// the GEOMETRY travels (paper/trim/inner/bleed/safety in mm) — the
    /// preset's own dpi is ignored, because the resolution field above is
    /// the one the artist is looking at while they decide.
    ///
    /// This is what makes the paper changeable after creation at all: Work
    /// Settings moves the guides but not a single pixel, so switching a B4
    /// chapter to B5 there left every page the wrong size for its own
    /// paper. Here the pages are rescaled onto the new paper in the same
    /// pass that changes the resolution.
    pub paper: Option<mn_core::PageSetup>,
}

impl Default for ResampleWorkDraft {
    fn default() -> Self {
        Self {
            dpi: 600,
            // The reduction kernel: this dialog exists mostly to go DOWN
            // (600 → 350 is the common ask), and that is the case where
            // bilinear drops hairlines outright.
            interp: mn_core::transform::Interp::HighAccuracy,
            paper: None,
        }
    }
}

impl App {
    // --- comic pages -------------------------------------------------------

    pub fn is_comic(&self) -> bool {
        // Multi-page, or any work metadata (a 1-page comic still saves as a
        // work folder — an ORA would drop its story/setup).
        self.pages.len() > 1 || self.page.is_some() || !self.story.trim().is_empty()
    }

    /// A multi-page work or one with a story — i.e. something the Pages
    /// palette is FOR. A plain illustration (opened image, empty-story
    /// single page) is not: its Pages tab auto-closes (owner report
    /// 2026-08-16 — "it's not a manga").
    pub fn is_manga_project(&self) -> bool {
        self.pages.len() > 1 || !self.story.trim().is_empty()
    }

    /// Reconcile the Pages palette with the current document: present for
    /// manga works, closed for plain images. Manual View ▸ Palettes ▸ Pages
    /// re-opens it (and it stays until the next document switch — the same
    /// CSP-style auto-management CSP applies to its EX page manager).
    pub fn sync_pages_palette(&mut self) {
        use crate::ui::dock::Palette;
        let manga = self.is_manga_project();
        let open = crate::ui::dock::is_open(self, Palette::Pages);
        if manga == open {
            return;
        }
        if manga {
            // Never the canvas leaf: reopen() upholds the class rule.
            // UNFOCUSED: this is automatic, not a request for the Pages
            // palette, and CSP's default workspace shows Layers (parity
            // P0-3). Focusing it here put page thumbnails on top of the
            // Layer palette on every manga launch.
            crate::ui::dock::reopen_unfocused(self, Palette::Pages);
        } else {
            crate::ui::dock::close_palette(self, Palette::Pages);
        }
    }

    /// Monotonic page-content revision (folder-save skip hint).
    pub fn page_rev_next(&mut self) -> u64 {
        self.page_clock = self.page_clock.max(self.doc.revision) + 1;
        self.page_clock
    }

    /// A NEW page entry: fresh content revision, never saved anywhere yet.
    /// Folder ids are assigned at the first folder save.
    pub fn fresh_page(
        &mut self,
        bytes: Option<Vec<u8>>,
        thumb: Option<egui::TextureHandle>,
    ) -> PageEntry {
        let rev = self.page_rev_next();
        PageEntry {
            bytes,
            thumb,
            rev,
            ..PageEntry::active()
        }
    }

    /// A fresh SPREAD entry (runtime-flagged; ids still assigned at the
    /// first folder save).
    pub fn fresh_spread(&mut self, bytes: Option<Vec<u8>>) -> PageEntry {
        let mut e = self.fresh_page(bytes, None);
        e.spread = true;
        e
    }

    /// Drop all work-folder bookkeeping (new/open document).
    pub fn reset_folder_state(&mut self) {
        self.folder_managed.clear();
        self.folder_next_id = 0;
    }

    /// Adopt work-folder bookkeeping after opening a work folder (cmd.rs is a
    /// sibling module — the fields stay private to `app`).
    ///
    /// It also lifts the page clock over the revisions the work arrived
    /// with. The clock starts at 0 every launch, so a work whose pages sit
    /// at revision 4000 would otherwise hand its next edit a LOWER number:
    /// `save_folder` skips a page with `rev <= saved_rev`, and the export
    /// reminder compares the same way — both would have gone quiet about
    /// the page just edited. Revisions only ever move forward.
    pub fn adopt_folder_state(&mut self, next_id: u32, managed: Vec<String>) {
        self.folder_next_id = next_id;
        self.folder_managed = managed;
        let highest = self
            .pages
            .iter()
            .map(|e| e.rev.max(e.exported_rev))
            .max()
            .unwrap_or(0);
        self.page_clock = self.page_clock.max(highest);
    }

    /// This work's unexported-page count (see [`unexported_pages`]), with
    /// the page currently being drawn on taken into account.
    pub fn unexported_pages(&self) -> usize {
        unexported_pages(&self.pages, Some((self.page_index, self.doc.revision)))
    }

    /// Record that page `i`'s image was just written out by an export.
    ///
    /// The ACTIVE page stashes first: its `rev` only moves at stash time,
    /// so recording the revision it still carries would mark the export
    /// stale the moment the page landed in its slot — the reminder would
    /// nag about the pages it just wrote.
    pub fn note_page_exported(&mut self, i: usize) {
        if i >= self.pages.len() {
            return;
        }
        if i == self.page_index && self.pages[i].doc_rev != self.doc.revision {
            let was_live = self.pages[i].bytes.is_none();
            if let Err(e) = self.stash_current_page() {
                self.set_error(format!("export note skipped: {e}"));
                return;
            }
            if was_live {
                // The active page's bytes live in `doc` (the invariant
                // every other stash caller restores the same way).
                self.pages[i].bytes = None;
            }
        }
        // At least 1: a page exported at revision 0 (single-file `.mnc`
        // pages all load there) must still count as "has been exported",
        // which is the gate that keeps a never-exported work quiet.
        self.pages[i].exported_rev = self.pages[i].rev.max(1);
    }

    /// The pages' stable identities in reading order — what a `.mnc` save
    /// records as `ProjectMeta::page_uids` so the ネーム promotion's
    /// page-to-page mapping survives a reopen (workflow audit §11).
    pub fn page_uids(&self) -> Vec<u64> {
        self.pages.iter().map(|e| e.uid).collect()
    }

    /// Adopt the identities a work arrived from disk with, minting a fresh
    /// one wherever the file recorded none (0 — a work saved before §11),
    /// and lifting the mint floor over everything adopted. `stored` may be
    /// shorter than `self.pages`.
    pub fn adopt_page_uids(&mut self, stored: &[u64]) {
        // The floor moves FIRST, so the fresh identities minted for any
        // unrecorded page below cannot collide with an adopted one.
        if let Some(hi) = stored.iter().copied().max() {
            PageEntry::bump_uid_floor(hi);
        }
        for (i, e) in self.pages.iter_mut().enumerate() {
            e.uid = match stored.get(i).copied().unwrap_or(0) {
                0 => PageEntry::next_uid(),
                uid => uid,
            };
        }
    }

    /// Render a Pages-panel thumbnail of the current document. Each call
    /// mints a FRESH texture: `load_texture` under one shared name replaces
    /// the texture every entry holding that handle shows — every stashed
    /// page thumb would have aliased the live one.
    ///
    /// Owner preview tier: renders at the size the pane actually shows
    /// (was a hardcoded 112×160), so the ACTIVE page stays sharp at any
    /// zoom. Below 112 the icon render is fine; the clamp above keeps a
    /// dragged-huge pane from minting page-sized textures per revision.
    pub fn thumb_of_current(&mut self) -> egui::TextureHandle {
        self.pages_thumb_seq += 1;
        let aspect = self.doc.size.1.max(1) as f32 / self.doc.size.0.max(1) as f32;
        let tw = self.pages_cell_px.clamp(112.0, 1200.0);
        let th = (tw * aspect).clamp(112.0, 1600.0);
        self.pages_thumb_px = th;
        // F1 (audit r69-78): drafts OFF — the same truth as every other
        // page's stored preview. Whether the GRID should show roughs the
        // way CSP does is still the owner's parked question; until he
        // answers, the palette must not show two different pages.
        let img = render_offscreen_drafts_off(
            &mut self.renderer,
            &mut self.doc,
            tw.round() as u32,
            th.round() as u32,
        );
        let (w, h) = (img.width() as usize, img.height() as usize);
        let ci = egui::ColorImage::from_rgba_unmultiplied([w, h], img.as_raw());
        self.shell.ctx.load_texture(
            format!("mn.page.live.{}", self.pages_thumb_seq),
            ci,
            egui::TextureOptions::LINEAR,
        )
    }

    /// The preview tier's long-edge cap: a B4 600 dpi page lands at
    /// 1132×1600, roughly 150–400 KB of gray line art per page.
    pub const PREVIEW_LONG_EDGE: u32 = 1600;
    /// Decoded previews held in memory before the LRU evicts (~32 pages).
    pub const PREVIEW_CACHE_CAP: usize = 32;

    /// Render the current page as the PREVIEW tier PNG bytes for
    /// `mnc/preview.png`: EXPORT rules — drafts OFF — gray-8. See
    /// [`render_offscreen_drafts_off`] for the visibility-flip contract.
    pub fn render_page_preview_png(&mut self) -> Result<Vec<u8>, String> {
        let (dw, dh) = self.doc.size;
        if dw == 0 || dh == 0 {
            return Err("empty page".into());
        }
        let (pw, ph) = if dw >= dh {
            (
                Self::PREVIEW_LONG_EDGE,
                ((Self::PREVIEW_LONG_EDGE as u64 * dh as u64) / dw as u64).max(1) as u32,
            )
        } else {
            (
                ((Self::PREVIEW_LONG_EDGE as u64 * dw as u64) / dh as u64).max(1) as u32,
                Self::PREVIEW_LONG_EDGE,
            )
        };
        let Self { renderer, doc, .. } = self;
        let img = render_offscreen_drafts_off(renderer, doc, pw, ph);
        // Gray-8 (mono manga preview) keeps the entry small and the LRU
        // light: 1132×1600 is ~1.8 MB decoded instead of ~7.3 RGBA.
        let gray = image::DynamicImage::ImageRgba8(img).to_luma8();
        let mut png = Vec::new();
        image::DynamicImage::ImageLuma8(gray)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        Ok(png)
    }

    /// The page's decoded preview (LRU-cached on the entry, evicted via
    /// `preview_order`): decodes `mnc/preview.png` from the stashed bytes
    /// on first ask, re-decodes when the page's rev moved. The ACTIVE
    /// page has no bytes — its live thumb already renders at display size.
    pub fn preview_for(&mut self, i: usize) -> Option<std::sync::Arc<image::GrayImage>> {
        let rev = self.pages.get(i)?.rev;
        if let Some((r, img)) = &self.pages[i].preview_img
            && *r == rev
        {
            let img = img.clone();
            self.preview_order.retain(|&x| x != i);
            self.preview_order.push_back(i);
            return Some(img);
        }
        let bytes = self.pages[i].bytes.as_ref()?;
        let img = std::sync::Arc::new(mn_core::project::page_preview(bytes)?);
        self.pages[i].preview_img = Some((rev, img.clone()));
        self.preview_order.retain(|&x| x != i);
        self.preview_order.push_back(i);
        while self.preview_order.len() > Self::PREVIEW_CACHE_CAP {
            let evict = self.preview_order.pop_front()?;
            if let Some(e) = self.pages.get_mut(evict) {
                e.preview_img = None;
            }
        }
        Some(img)
    }

    /// Run the print preflight (TRIAGE 132): work-level checks over the
    /// metadata, page-level checks over every page's content — the active
    /// page straight from `doc`, the rest decoded from their stashed ORA
    /// bytes (unchanged while another page is being edited, which is why
    /// the cache key is the active doc's revision + page). Findings carry
    /// page context in their messages already.
    /// [`Self::run_preflight`] behind its cache key (palette opened, page
    /// switched, active page edited, work metadata edited). The palette
    /// and the export gate share this door: a run decodes every stashed
    /// page, which is too much to pay twice for one Export All.
    pub fn preflight_cached(&mut self) -> Vec<mn_core::PreflightFinding> {
        let stale = self.preflight_findings.is_none()
            || self.preflight_stale
            || self.preflight_rev != self.doc.revision
            || self.preflight_page != self.page_index;
        if stale {
            self.preflight_findings = Some(self.run_preflight());
        }
        self.preflight_findings.clone().unwrap_or_default()
    }

    pub fn run_preflight(&mut self) -> Vec<mn_core::PreflightFinding> {
        let mut meta = mn_core::ProjectMeta::for_checks(
            self.story.clone(),
            self.binding_right,
            self.page.clone(),
            self.expression,
            self.spine_mm,
            self.cover,
        );
        // The profile IS a preflight input (page-count multiple, screen
        // ruling); set after construction so for_checks stays six params.
        meta.profile = self.profile.clone();
        let mut out = mn_core::preflight::run_work(&meta, self.pages.len());
        if let Some(setup) = &self.page {
            let mut check = |i: usize, doc: &Document| {
                out.extend(mn_core::preflight::run_page(setup, &meta, i, doc));
            };
            check(self.page_index, &self.doc);
            for (i, e) in self.pages.iter().enumerate() {
                if i == self.page_index {
                    continue;
                }
                if let Some(b) = &e.bytes {
                    if let Ok(doc) = mn_core::project::bytes_to_doc(b) {
                        check(i, &doc);
                    }
                }
            }
        }
        self.preflight_rev = self.doc.revision;
        self.preflight_page = self.page_index;
        self.preflight_stale = false;
        out
    }

    /// Serialize the active document back into its page slot (and refresh its
    /// that was not edited since its last encode is not re-encoded — and its
    /// revision stays put, so folder saves keep skipping it.
    pub fn stash_current_page(&mut self) -> Result<(), String> {
        self.end_stroke();
        self.commit_text_edit();
        let i = self.page_index;
        let changed = self.pages[i].doc_rev != self.doc.revision;
        // A lazy-blank page that was never touched stashes to NOTHING: its
        // template marker still describes it exactly (and encoding a B4
        // blank is the 40-second walk this marker exists to avoid).
        if !changed && (self.pages[i].bytes.is_some() || self.pages[i].blank.is_some()) {
            return Ok(());
        }
        // Owner preview tier: the sharp preview rides the page bytes
        // (mnc/preview.png, export rules). A preview failure never blocks
        // the stash — the old bytes' preview (or none) is still correct
        // for the last-saved content.
        let preview = self.render_page_preview_png().ok();
        let bytes = mn_core::project::doc_to_bytes_with(&self.doc, preview.as_deref())
            .map_err(|e| e.to_string())?;
        let thumb = self.thumb_of_current();
        let rev = if changed { self.page_rev_next() } else { 0 };
        let e = &mut self.pages[i];
        e.bytes = Some(bytes);
        e.thumb = Some(thumb);
        // It has real content now — the template marker is spent.
        e.blank = None;
        if changed {
            e.rev = rev;
            e.doc_rev = self.doc.revision;
        }
        Ok(())
    }

    /// Install a document REBUILT FOR THE PAGE THE TAB IS ALREADY ON — a
    /// spread combined or split, a page's art replaced by an import.
    ///
    /// The page did not change under the artist here, only its content, so
    /// the ruler set carries: those rebuilt documents are assembled from
    /// scratch and arrive with no rulers of their own, and dropping the
    /// grid because a spread was split would be the app throwing work
    /// away. A page TURN is the opposite case and does not come through
    /// here — see [`App::switch_page`], where rulers are per page.
    ///
    /// A doc that does bring its own geometry keeps it: it saved that set
    /// and it is the truth for this page.
    ///
    /// Returns the document it replaced. The ruler carry CLONES rather than
    /// takes: the leaving document may be parked (workflow-audit #1), and a
    /// parked document stripped of its rulers would disagree with its own
    /// stashed bytes.
    pub fn adopt_page_doc(&mut self, doc: Document) -> Document {
        let old = std::mem::replace(&mut self.doc, doc);
        if !self.doc.rulers.has_geometry() {
            // Whole set including the on/special switches: a page without
            // geometry has nothing to say about them either.
            self.doc.rulers = old.rulers.clone();
        }
        old
    }

    /// Park the leaving page's live document beside its bytes — a return to
    /// this page reinstalls it, undo history and revisions intact. Newest
    /// wins a small LRU (a parked page is a full-size document plus its
    /// history); an evicted page loses only its history, the bytes remain
    /// the truth for save / export / preflight.
    fn park_page_doc(&mut self, i: usize, doc: Document) {
        const CAP: usize = 2;
        // A never-touched template page (its `blank` marker is unspent)
        // rebuilds instantly and has no history — not worth a slot.
        if self.pages[i].blank.is_some() {
            return;
        }
        let uid = self.pages[i].uid;
        self.pages[i].parked = Some(Box::new(doc));
        self.pages[i].parked_rev = self.pages[i].rev;
        // Recency list: also purge uids whose page is gone or no longer
        // holds a park, so a dead entry can never squat on a CAP slot.
        let live: Vec<u64> = self
            .pages
            .iter()
            .filter(|e| e.parked.is_some())
            .map(|e| e.uid)
            .collect();
        self.page_park_lru
            .retain(|&u| u != uid && live.contains(&u));
        self.page_park_lru.push(uid);
        while self.page_park_lru.len() > CAP {
            let evict = self.page_park_lru.remove(0);
            if let Some(e) = self.pages.iter_mut().find(|e| e.uid == evict) {
                e.parked = None;
            }
        }
    }

    /// Switch the editor to another page (decode-on-switch).
    pub fn switch_page(&mut self, i: usize) {
        if i == self.page_index || i >= self.pages.len() {
            return;
        }
        // Direct-feel rule: a transform float is modal — bake it on the page
        // it was lifted on BEFORE the doc under it is swapped (otherwise its
        // commit would stamp into the arriving page).
        self.commit_open_float();
        let was_clean = !self.dirty();
        let old = self.page_index;
        // The eye-solo snapshot belongs to the page being left.
        self.eye_solo_backup = None;
        if let Err(e) = self.stash_current_page() {
            self.set_error(format!("page stash failed: {e}"));
            return;
        }
        // The arriving doc, best source first: the page's own PARKED live
        // document (workflow-audit #1 — history and revisions intact) when
        // its bytes have not moved on without it; else decoded bytes; else
        // a still-blank template page MATERIALIZED directly (the lazy-blank
        // path — a build, not a decode, and nothing was ever encoded for
        // it).
        let parked = {
            let e = &mut self.pages[i];
            match e.parked.take() {
                // A direct byte writer (story editor, batch ops, resize)
                // bumped `rev` past the park — the parked document is
                // stale. Drop it and decode what actually happened.
                Some(doc) if e.parked_rev == e.rev => Some(*doc),
                _ => None,
            }
        };
        if parked.is_some() {
            let uid = self.pages[i].uid;
            self.page_park_lru.retain(|&u| u != uid);
        }
        let arriving = match parked {
            Some(doc) => {
                // The active-page invariant (bytes live in `doc`) holds on
                // this path too.
                self.pages[i].bytes = None;
                doc
            }
            None => match self.pages[i].bytes.take() {
                Some(bytes) => match mn_core::project::bytes_to_doc(&bytes) {
                    Ok(doc) => doc,
                    Err(e) => {
                        self.set_error(format!("page {} failed to decode: {e}", i + 1));
                        return;
                    }
                },
                None => match self.pages[i].blank {
                    Some((bw, bh, n)) => self.blank_page_doc_at(bw, bh, n),
                    None => {
                        self.set_error(format!("page {} has no data", i + 1));
                        return;
                    }
                },
            },
        };
        {
            let doc = arriving;
            let leaving_size = self.doc.size;
            // Owner ruling 2026-09-04: rulers belong to a PAGE, like CSP's
            // per-page ruler layer. Perspective changes per scene, so the
            // arriving page brings its OWN set (it rides the page bytes as
            // `mnc/rulers.json`) and nothing at all when nothing was ever
            // built there — no `adopt_page_doc` carry on a page turn.
            let leaving = std::mem::replace(&mut self.doc, doc);
            // Nothing app-side may still point into the set we just left:
            // an in-flight grab holds indices, an armed creation belongs to
            // the page it was armed on.
            self.ruler_move = None;
            self.ruler_drag = None;
            self.ruler_pending = None;
            self.curve_pending = None;
            // Frame-border curves are DERIVED per page, and the arriving
            // page's set already carries its own; `sync_frame_rulers`
            // (through `renumber_frames`, below) retracts by value against
            // this list, so it must describe the page we are ON.
            self.frame_rulers = self
                .doc
                .layers
                .iter()
                .filter_map(|l| l.frames())
                .flat_map(|fs| fs.ruler_curves())
                .filter(|c| self.doc.rulers.curves.contains(c))
                .collect();
            self.park_page_doc(old, leaving);
            self.page_index = i;
            // The page's bytes now equal the decoded doc — record its
            // revision so an untouched stash is a no-op.
            self.pages[i].doc_rev = self.doc.revision;
            // A clean document stays clean across a page switch even
            // though the decoded page carries fresh revisions.
            if was_clean {
                self.saved_revision = self.doc.revision;
            }
            self.renderer.invalidate();
            // Workflow-audit #1, cheap half: same paper = same viewport.
            // The "same spot, next page" pass (a panel-by-panel background
            // sweep across the chapter) keeps its zoom, pan and rotation;
            // only a page of a different size re-fits.
            if self.doc.size != leaving_size {
                self.fit_to_view();
            }
            self.layer_thumbs.clear();
            // The reading order is per-page (different geometry).
            self.renumber_frames();
            // The Story Editor (if open) holds DECODED page copies. The
            // page we just left has FRESH bytes now — its old entry was
            // `None` (it was live), and the page we arrived at is live
            // now, so its decode must go. Without this, editing a field
            // on the just-left page re-encoded a decode from when the
            // editor OPENED, replacing everything drawn on that page
            // since — the same wholesale-replace hazard the tab-switch
            // path already guards (forget_document_caches), open here
            // through the page door.
            if self.story_open {
                if self.story_docs.len() != self.pages.len() {
                    self.story_open_refresh();
                } else {
                    self.story_docs[old] = self.pages[old]
                        .bytes
                        .as_ref()
                        .and_then(|b| mn_core::project::bytes_to_doc(b).ok());
                    self.story_docs[i] = None;
                    self.story_sel = None;
                    self.story_rebuffer();
                }
            }
            self.set_status(format!("page {}", i + 1));
            // Row 166 door 4 (the paid deferral in `file_object.rs`'s
            // module doc): the arriving page's links were last checked
            // when the WORK opened — a hop inside a work folder must not
            // miss a redrawn background until the next alt-tab. Quiet
            // door: keeps the "page N" line when nothing changed, and
            // after it when something did.
            self.refresh_file_objects_quiet();
            // An armed leak repair never survives a page hop — the refill
            // validates again at run time, but a gesture owed to page A
            // must not draw its barrier over page B.
            if self.fill_repair.take().is_some() {
                self.set_status("fill repair stood down — the page moved");
            }
        }
        self.needs_redraw = true;
    }

    /// 1-based reading-order number of the page entry `i` STARTS at — a
    /// combined spread occupies two numbers, so parity downstream of one
    /// stays honest.
    pub fn page_number1(&self, entry: usize) -> usize {
        1 + self.pages[..entry.min(self.pages.len())]
            .iter()
            .map(|e| if e.spread { 2 } else { 1 })
            .sum::<usize>()
    }

    /// The current page's book side (`Some(true)` = right page). `None` =
    /// a combined spread, which spans the fold and has both sides.
    pub fn current_page_right(&self) -> Option<bool> {
        let e = self.pages.get(self.page_index)?;
        if e.spread {
            return None;
        }
        Some(mn_core::page::PageSetup::page_is_right(
            self.page_number1(self.page_index),
            self.binding_right,
        ))
    }

    /// A fresh page document matching the project's page size, seeded with a
    /// frame border folder when the project asked for one. Sided for the
    /// slot AddPage fills — right after the current page — so the seeded
    /// frame sits on the correct ノド/小口 offset (the owner's 2026-08-22
    /// report: pages 2 and 3 wore the SAME offset, like two right pages).
    pub fn blank_page_doc(&self) -> Document {
        let (w, h) = self
            .page
            .as_ref()
            .map(|p| p.paper_px())
            .unwrap_or(self.doc.size);
        self.blank_page_doc_at(w, h, self.next_page_number1())
    }

    /// The reading-order number the slot AddPage and ImportPage fill —
    /// right after the current page — will carry. A combined spread under
    /// the cursor eats two numbers, so the page after it starts two on.
    pub fn next_page_number1(&self) -> usize {
        self.page_number1(self.page_index)
            + if self.pages.get(self.page_index).is_some_and(|e| e.spread) {
                2
            } else {
                1
            }
    }

    /// Same, at an explicit size (New Manga runs before `self.doc` exists).
    /// Page 1's side by the book rule.
    pub fn blank_page_doc_sized(&self, w: u32, h: u32) -> Document {
        self.blank_page_doc_at(w, h, 1)
    }

    /// The seeding core: `number1` decides which book side the frame's
    /// binding offset mirrors to.
    pub fn blank_page_doc_at(&self, w: u32, h: u32, number1: usize) -> Document {
        self.seeded_page_doc(w, h, number1, true)
    }

    /// The shared seeding, with CSP's "Fill inside the frame" choice
    /// exposed: a blank page wants the White base, an imported-photo page
    /// must NOT have it — the underlay lands at the BOTTOM of the stack,
    /// and the White base would hide it across the whole panel interior
    /// (the part-19 lesson: the White layer's job is to hide what is
    /// below the folder).
    pub(super) fn seeded_page_doc(
        &self,
        w: u32,
        h: u32,
        number1: usize,
        fill_white: bool,
    ) -> Document {
        let mut doc = Document::new(w, h);
        if self.seed_frame_folder {
            if let Some(p) = self.page.as_ref().filter(|p| p.has_guides()) {
                let border = (0.8 / 25.4 * p.dpi.max(1) as f32).max(2.0);
                let right = mn_core::page::PageSetup::page_is_right(number1, self.binding_right);
                doc.add_frame_folder_with(
                    "Frame 1",
                    mn_core::FrameSet::single_rect(p.inner_rect_px_on(right), border),
                    fill_white,
                );
            }
        }
        doc
    }
}

/// EXPORT-rules offscreen render (drafts hidden) of ANY document at
/// `w×h` — the shared engine of the preview tier and the reader's sharp
/// pass. Draft visibility is flipped and restored around the render: the
/// compositor's own cascade does the work, the invalidate() pair forces
/// the rebuilds, and it is presentation-only (no revision bump, no undo
/// traffic). Free function so callers can split-borrow renderer/doc out
/// of `App`.
pub(crate) fn render_offscreen_drafts_off(
    renderer: &mut mn_gpu::Renderer,
    doc: &mut Document,
    w: u32,
    h: u32,
) -> image::RgbaImage {
    let hidden: Vec<usize> = doc
        .layers
        .iter()
        .enumerate()
        .filter(|(_, l)| l.draft && l.visible)
        .map(|(i, _)| i)
        .collect();
    // F2 (audit r69-78): a draft-free page — the common finished chapter
    // — pays nothing. The double invalidate() drops the ENTIRE tile
    // cache (pool-truncated); skipping it makes the reader's sharp pass
    // and every preview render free on draft-free pages.
    if hidden.is_empty() {
        return renderer.render_offscreen(doc, w, h);
    }
    // F2: the visibility flip restores on ANY unwind — a panic mid-render
    // must not leave his drafts hidden in the live document. (A Drop
    // guard cannot hold `&mut doc` through the render, so: catch,
    // restore, resume.)
    for &i in &hidden {
        doc.layers[i].visible = false;
    }
    renderer.invalidate();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        renderer.render_offscreen(doc, w, h)
    }));
    for &i in &hidden {
        doc.layers[i].visible = true;
    }
    renderer.invalidate();
    match result {
        Ok(img) => img,
        Err(e) => std::panic::resume_unwind(e),
    }
}
