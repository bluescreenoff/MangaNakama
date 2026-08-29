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
    /// Not persisted — `id` is the on-disk identity; this one only has to
    /// be unique while the app is running.
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

impl PageEntry {
    /// The next runtime page identity. A process-wide counter rather than
    /// an App field so EVERY construction path gets one — including the
    /// `..PageEntry::active()` shorthands — and pages from two open tabs
    /// never collide. Starts at 1: 0 means "no such page".
    pub fn next_uid() -> u64 {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
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

    /// File names the current pages map to in a work folder (the managed set).
    pub fn page_file_names(&self) -> Vec<String> {
        self.pages
            .iter()
            .map(|e| mn_core::project::page_file_name(e.id))
            .collect()
    }

    /// Is `p` the index of the work folder we are currently editing?
    pub fn is_our_work_index(&self, p: &std::path::Path) -> bool {
        self.doc_path.as_deref() == Some(p)
    }

    /// Save the whole work into `index`'s folder — the native multi-page
    /// format: `work.mnc` (tiny index) + `pNNN.ora` side by side, rewriting
    /// only pages whose revision advanced (see `mn_core::project::save_folder`
    /// for the atomicity story). Refuses to touch a folder that already holds
    /// work files that are not ours.
    pub fn save_work_folder(&mut self, index: &std::path::Path) -> Result<String, String> {
        let dir = index
            .parent()
            .ok_or_else(|| "work folder path has no parent".to_owned())?
            .to_path_buf();
        if !self.is_our_work_index(index) {
            if let Ok(rd) = std::fs::read_dir(&dir) {
                let foreign = rd.flatten().any(|e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    mn_core::project::is_workfolder_file(&n)
                        && !self.folder_managed.iter().any(|m| *m == n)
                });
                if foreign {
                    return Err("folder already holds work files — pick an empty folder".into());
                }
            }
        }
        self.stash_current_page()?;
        let wf = mn_core::project::WorkFolder {
            story: self.story.clone(),
            binding_right: self.binding_right,
            setup: self.page.clone(),
            expression: self.expression,
            spine_mm: self.spine_mm,
            cover: self.cover,
            template_page: self.template_page,
            profile: self.profile.clone(),
            next_id: self.folder_next_id,
            pages: self
                .pages
                .iter()
                .map(|e| mn_core::project::FolderPage {
                    id: e.id,
                    rev: e.rev,
                    saved_rev: e.saved_rev,
                    exported_rev: e.exported_rev,
                    // A still-blank template page materializes HERE — the
                    // one place bytes are truly required (the save). This
                    // is the lazy-blank design's single deliberate cost.
                    bytes: match (&e.bytes, e.blank) {
                        (Some(b), _) => b.clone(),
                        (None, Some((bw, bh, n))) => {
                            let doc = self.blank_page_doc_at(bw, bh, n);
                            mn_core::project::doc_to_bytes(&doc).unwrap_or_default()
                        }
                        (None, None) => Vec::new(),
                    },
                })
                .collect(),
        };
        let (ids, written) = mn_core::project::save_folder(&wf, &dir, &self.folder_managed)
            .map_err(|e| e.to_string())?;
        for (e, &id) in self.pages.iter_mut().zip(&ids) {
            e.id = id;
            e.saved_rev = e.rev.max(1);
        }
        // The active page keeps living in `doc`, not in bytes.
        self.pages[self.page_index].bytes = None;
        self.folder_managed = self
            .pages
            .iter()
            .map(|e| mn_core::project::page_file_name(e.id))
            .collect();
        let max_id = self.pages.iter().map(|e| e.id).max().unwrap_or(0);
        self.folder_next_id = self.folder_next_id.max(max_id + 1);
        Ok(format!(
            "saved work folder {} ({} pages, {written} rewritten)",
            dir.display(),
            self.pages.len()
        ))
    }

    /// Autosave the whole work into a TEMP work folder — `index` is
    /// [`crate::app::unsaved_autosave_folder_for`]'s
    /// `%TEMP%\MangaNakama-autosave[-N]\work.mnc` (05 item 1: the
    /// pathless-work crash net). Same per-dirty-page incremental format
    /// as [`Self::save_work_folder`], with two deliberate differences:
    ///
    /// * the skip key is each page's `autosaved_rev` watermark, and ONLY
    ///   that advances — `saved_rev` still means "safe in the work's real
    ///   home", so a later Save As rewrites every page it should.
    /// * no stale-file cleanup and no foreign-file refusal: the folder is
    ///   ours by construction (slot-keyed under `%TEMP%`) and dies whole
    ///   in `recovery::clear_unsaved_stash`.
    pub fn autosave_work_folder(&mut self, index: &std::path::Path) -> Result<String, String> {
        let dir = index
            .parent()
            .ok_or_else(|| "autosave folder path has no parent".to_owned())?
            .to_path_buf();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        self.stash_current_page()?;
        let wf = mn_core::project::WorkFolder {
            story: self.story.clone(),
            binding_right: self.binding_right,
            setup: self.page.clone(),
            expression: self.expression,
            spine_mm: self.spine_mm,
            cover: self.cover,
            template_page: self.template_page,
            profile: self.profile.clone(),
            next_id: self.folder_next_id,
            pages: self
                .pages
                .iter()
                .map(|e| mn_core::project::FolderPage {
                    id: e.id,
                    rev: e.rev,
                    // THE TRAP THIS WHOLE METHOD EXISTS FOR: the temp
                    // watermark is the skip key, and it is the only one
                    // this write advances.
                    saved_rev: e.autosaved_rev,
                    exported_rev: e.exported_rev,
                    bytes: e.bytes.clone().unwrap_or_default(),
                })
                .collect(),
        };
        let (ids, written) =
            mn_core::project::save_folder(&wf, &dir, &[]).map_err(|e| e.to_string())?;
        for (e, &id) in self.pages.iter_mut().zip(&ids) {
            e.id = id;
            e.autosaved_rev = e.rev.max(1);
        }
        // The active page keeps living in `doc`, not in bytes.
        self.pages[self.page_index].bytes = None;
        let max_id = self.pages.iter().map(|e| e.id).max().unwrap_or(0);
        self.folder_next_id = self.folder_next_id.max(max_id + 1);
        Ok(format!("{} page(s) -> {}", written, dir.display()))
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

    /// Resize every OTHER page of the work to `w × h`, pinning content to
    /// `anchor`. The OPEN page is not touched here — its caller takes it
    /// through the normal canvas door so undo, caches and the renderer all
    /// see it.
    ///
    /// Same bytes round trip as `App::batch_other_pages` /
    /// `AppCmd::CompApplyAllPages`: stash the open page first, decode each
    /// parked page's bytes, edit the decoded document, re-encode, hand it a
    /// fresh content revision and drop its thumbnail (the Pages panel and
    /// the rev-keyed sharp preview both cache on that). The decoded
    /// documents never become `self.doc`, so the `adopt_page_doc` ruler trap
    /// cannot bite. Afterwards the active-page invariant is restored: bytes
    /// live in `doc`.
    ///
    /// A COMBINED spread is a double-width page, so it takes `2w × h` — the
    /// same 1.5× width test the export split uses decides, because the
    /// `spread` flag is runtime-only and a reloaded work has none.
    ///
    /// These writes are DIRECT: undo covers the open page only. Returns
    /// (resized, unreadable).
    pub fn resize_other_pages(
        &mut self,
        w: u32,
        h: u32,
        anchor: ResizeAnchor,
        normal_w: Option<u32>,
    ) -> (usize, usize) {
        if let Err(e) = self.stash_current_page() {
            self.set_error(format!("other pages skipped: {e}"));
            return (0, self.pages.len().saturating_sub(1));
        }
        let (mut done, mut failed) = (0usize, 0usize);
        for i in 0..self.pages.len() {
            if i == self.page_index {
                continue; // the live document already took it
            }
            // A still-LAZY blank never encodes for this: resizing a blank
            // page is re-marking its template size, which keeps the whole
            // point of the marker (no ORA walk for untouched pages).
            if self.pages[i].bytes.is_none() && self.pages[i].blank.is_some() {
                let spread = self.pages[i].spread;
                let target = if spread {
                    (w.saturating_mul(2).max(1), h)
                } else {
                    (w, h)
                };
                let n = self.pages[i].blank.unwrap().2;
                self.pages[i].blank = Some((target.0, target.1, n));
                done += 1;
                continue;
            }
            let Some(bytes) = self.pages[i].bytes.as_deref() else {
                failed += 1;
                continue;
            };
            let Ok(mut doc) = mn_core::project::bytes_to_doc(bytes) else {
                failed += 1;
                continue;
            };
            let target = if crate::cmd::is_spread_page(&doc, self.pages[i].spread, normal_w) {
                (w.saturating_mul(2).max(1), h)
            } else {
                (w, h)
            };
            if doc.size == target {
                continue;
            }
            doc.resize_canvas(target.0, target.1, anchor);
            let Ok(nb) = mn_core::project::doc_to_bytes(&doc) else {
                failed += 1;
                continue;
            };
            let rev = self.page_rev_next();
            let e = &mut self.pages[i];
            e.bytes = Some(nb);
            e.rev = rev;
            e.doc_rev = 0;
            e.thumb = None;
            // The reader's 1:1 view reads this cache, not the bytes.
            e.canvas = Some(target);
            done += 1;
        }
        // Restore the active-page invariant (bytes live in `doc`).
        self.pages[self.page_index].bytes = None;
        self.mark_pages_dirty();
        self.mark_dirty();
        (done, failed)
    }

    /// Install a document decoded for the page (or pages) THIS TAB is
    /// already editing.
    ///
    /// Rulers persist per page now (`mnc/rulers.json`), so a page that
    /// carries its OWN set wins — that is the perspective grid the user
    /// built on that page. A page with no saved set still inherits the
    /// tab's working set (the old carry behaviour): rulers keep following
    /// the artist onto fresh pages, and a saved grid is never clobbered
    /// by the page you happened to switch away from. (Opening a file or
    /// making a new document does NOT come through here — a different
    /// document gets its own set as loaded.)
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
            let leaving = self.adopt_page_doc(doc);
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
    fn seeded_page_doc(&self, w: u32, h: u32, number1: usize, fill_white: bool) -> Document {
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

    /// Convert a file (.ora or image) to page ORA bytes, plus a status note
    /// when the file did not sit squarely on the paper. Used by ImportPage
    /// and ReplacePage; `number1` is the reading-order number the resulting
    /// page will carry, which decides the seeded frame's ノド/小口 side.
    ///
    /// **Workflow audit #2.** An image used to become a page of the IMAGE's
    /// own pixel size: a phone photo of a ネーム dropped into a B4/600 dpi
    /// chapter turned into a foreign-paper page with no trim, no bleed, no
    /// 基本枠 and no dpi, and — being an ordinary raster layer — its content
    /// EXPORTED as art. So the image branch now builds the work's own page
    /// (`blank_page_doc_at`, the same seeding a blank page gets) and places
    /// the photo in it scaled to fit, as a 下書き draft layer at the bottom
    /// of the stack: on screen, never in the export, drawn over.
    ///
    /// A work with no `PageSetup` is a plain canvas, not a manga project —
    /// there is no paper to inherit, so there the image's own size is still
    /// the only size there is and the old behaviour stands.
    pub fn file_to_page_bytes(
        &self,
        path: &std::path::Path,
        number1: usize,
    ) -> Result<(Vec<u8>, Option<String>), String> {
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if ext.eq_ignore_ascii_case("ora") {
            // Already ORA: read raw bytes.
            return std::fs::read(path).map(|b| (b, None)).map_err(|e| e.to_string());
        }
        // Assume image.
        let Some((pw, ph)) = self.page.as_ref().map(|p| p.paper_px()) else {
            // Plain canvas: import as a single-layer doc at the image's size.
            let rgba = image::open(path).map_err(|e| e.to_string())?.to_rgba8();
            let mut doc = mn_core::Document::new(rgba.width(), rgba.height());
            doc.add_layer_from_image(image_layer_name(path), &rgba);
            // Drop the empty default "Layer 1" underneath.
            if doc.layers.len() > 1 && doc.layers[1].is_empty() {
                doc.layers.remove(1);
                doc.active = 0;
            }
            let bytes = mn_core::project::doc_to_bytes(&doc).map_err(|e| e.to_string())?;
            return Ok((bytes, None));
        };
        let (name, fitted, note) = underlay_from_file(path, pw, ph)?;
        // fill_white = false (CSP's "Fill inside the frame" off): the
        // underlay goes to the BOTTOM of the stack, and the seeded
        // folder's White base would hide it across the whole panel
        // interior — an invisible 下書き is no 下書き. Export is
        // unchanged either way: the draft never prints, and panels
        // composite to paper white with or without the base.
        let mut doc = self.seeded_page_doc(pw, ph, number1, false);
        place_draft_underlay(&mut doc, name, &fitted);
        let bytes = mn_core::project::doc_to_bytes(&doc).map_err(|e| e.to_string())?;
        Ok((bytes, note))
    }

    /// Workflow audit #4 — CSP EX's *File ▸ Import ▸ Batch import*: the
    /// "I named the whole chapter on paper" step. Each picked file becomes
    /// the 下書き underlay of ONE page, in name order, starting at the
    /// dialog's page slot, and pages are ADDED when there are more images
    /// than pages.
    ///
    /// Two doors, by whether the target page exists:
    ///
    /// * **it exists** — the page keeps everything it has; only the
    ///   underlay is inserted, at [`underlay_slot`]. The OPEN page takes
    ///   that through `self.doc` with the whole stack recorded ONCE, so its
    ///   change is a single undo press; every other page is decoded from
    ///   its bytes, edited, and re-encoded.
    /// * **it does not** — a NEW page of the work's own paper through the
    ///   finding-2 door ([`App::file_to_page_bytes`]).
    ///
    /// The byte writes are the round trip `batch_other_pages` /
    /// [`App::resize_other_pages`] use, with the invariant that matters
    /// most since workflow audit #1: each written page takes a fresh
    /// content revision from `page_rev_next`, which is exactly what makes a
    /// parked live document stale (`PageEntry::parked_rev`) so a later
    /// switch decodes what the batch wrote instead of reinstalling the
    /// page as it was. Undo covers the OPEN page only — the dialog says so.
    ///
    /// The deferred half of the audit's row: CSP places the rectangle once
    /// with handles on page 1 and reuses it. That needs a cross-page
    /// placement gesture we do not have; every image is scale-to-fit here.
    pub fn batch_import_pages(&mut self) -> String {
        let files = std::mem::take(&mut self.batch_import.files);
        if files.is_empty() {
            return "batch import: no files were picked".into();
        }
        let Some((pw, ph)) = self.page.as_ref().map(|p| p.paper_px()) else {
            return "batch import: this work has no page setup — File ▸ Import Image as Draft…"
                .into();
        };
        if let Err(e) = self.stash_current_page() {
            return format!("batch import: {e}");
        }
        // 1-based slot -> index, clamped to "append at the end": a start
        // past the end would otherwise leave a hole of pages nobody asked
        // for between the chapter and the roughs.
        let start = self.batch_import.start.clamp(1, self.pages.len() + 1) - 1;
        let (mut written, mut added, mut failed) = (0usize, 0usize, 0usize);
        // ONE note, not N: twenty photos off the same phone all mismatch
        // the paper the same way, and twenty copies of that sentence is
        // not twenty times the information.
        let mut note: Option<String> = None;
        for (i, path) in files.iter().enumerate() {
            let target = start + i;
            if target >= self.pages.len() {
                // Past the end: a new page, the finding-2 way.
                let number1 = self.page_number1(self.pages.len());
                match self.file_to_page_bytes(path, number1) {
                    Ok((bytes, n)) => {
                        note = note.or(n);
                        let e = self.fresh_page(Some(bytes), None);
                        self.pages.push(e);
                        added += 1;
                    }
                    Err(e) => {
                        failed += 1;
                        self.set_error(format!("batch import: {} — {e}", path.display()));
                    }
                }
                continue;
            }
            let (name, fitted, n) = match underlay_from_file(path, pw, ph) {
                Ok(v) => v,
                Err(e) => {
                    failed += 1;
                    self.set_error(format!("batch import: {} — {e}", path.display()));
                    continue;
                }
            };
            note = note.or(n);
            if target == self.page_index {
                // THE OPEN PAGE. Record the pre-image once and then edit
                // the stack directly (the `comps.rs` pattern): going
                // through `add_layer_from_image` + `move_layer` would push
                // two structure groups, and the artist would need two undo
                // presses to take one import back.
                let before = self.doc.layers.clone();
                let active_before = self.doc.active;
                self.doc
                    .record_structure("Batch import underlay", before, active_before);
                place_draft_underlay(&mut self.doc, name, &fitted);
                self.renderer.invalidate();
                self.layer_thumbs.clear();
                written += 1;
                continue;
            }
            // A still-LAZY blank page has no bytes to decode — materialize
            // its template the way `save_work_folder` does.
            let blank = self.pages[target].blank;
            let mut doc = match self.pages[target].bytes.as_deref() {
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
            place_draft_underlay(&mut doc, name, &fitted);
            let Ok(nb) = mn_core::project::doc_to_bytes(&doc) else {
                failed += 1;
                continue;
            };
            let rev = self.page_rev_next();
            let e = &mut self.pages[target];
            e.bytes = Some(nb);
            // It has real content now — the template marker is spent.
            e.blank = None;
            // THE park-staleness bump: `switch_page` compares this against
            // `parked_rev` and drops a parked document that no longer
            // matches the bytes.
            e.rev = rev;
            e.doc_rev = 0;
            e.thumb = None;
            written += 1;
        }
        // Restore the active-page invariant (bytes live in `doc`).
        self.pages[self.page_index].bytes = None;
        self.mark_pages_dirty();
        self.mark_dirty();
        let mut s = format!("batch import: {written} page(s) written, {added} added");
        if let Some(n) = note {
            s.push_str(&format!(" — {n}"));
        }
        if failed > 0 {
            s.push_str(&format!(" — {failed} file(s) could not be read"));
        }
        s
    }
}

/// The layer name an imported image takes: the file's stem.
fn image_layer_name(path: &std::path::Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Imported".to_owned())
}

/// Scale `rgba` to sit inside a `pw × ph` page, plus the status note when
/// the file did not sit squarely on the paper.
///
/// Letterboxing is the honest answer to a mismatched aspect — the
/// alternative is reshaping a whole chapter's paper around one photo. Say
/// it in the status line and let the human decide.
pub(super) fn fit_to_paper(
    rgba: image::RgbaImage,
    pw: u32,
    ph: u32,
) -> (image::RgbaImage, Option<String>) {
    let (iw, ih) = (rgba.width(), rgba.height());
    let s = (pw as f32 / iw as f32).min(ph as f32 / ih as f32);
    let (tw, th) = (
        ((iw as f32 * s).round() as u32).max(1),
        ((ih as f32 * s).round() as u32).max(1),
    );
    let note = ((tw, th) != (pw, ph)).then(|| {
        format!(
            "{iw}x{ih} is not the page's shape — fitted to {tw}x{th} inside {pw}x{ph}, with margins"
        )
    });
    let fitted = if (tw, th) == (iw, ih) {
        rgba
    } else {
        image::imageops::resize(&rgba, tw, th, image::imageops::FilterType::Lanczos3)
    };
    (fitted, note)
}

/// Read an image file as a fitted 下書き underlay for a `pw × ph` page:
/// the layer name, the fitted pixels, and the aspect note.
///
/// The shared front half of BOTH import doors — `file_to_page_bytes`
/// (workflow audit #2) and the batch import (#4) — so the two can never
/// end up fitting the same photo differently.
pub(super) fn underlay_from_file(
    path: &std::path::Path,
    pw: u32,
    ph: u32,
) -> Result<(String, image::RgbaImage, Option<String>), String> {
    let rgba = image::open(path).map_err(|e| e.to_string())?.to_rgba8();
    let (fitted, note) = fit_to_paper(rgba, pw, ph);
    Ok((image_layer_name(path), fitted, note))
}

/// Where a 下書き underlay lands in a page's stack, as `(slot, depth)`.
///
/// **The rule.** Normally the very BOTTOM of the stack at root depth: a
/// rough is what you draw over, so nothing already on the page may end up
/// underneath it.
///
/// **The exception** is CSP's "Fill inside the frame" White base. A page
/// that was blank or drawn carries one at the bottom of its frame folder,
/// and it paints the whole panel interior opaque — an underlay below it is
/// invisible exactly where the drawing happens (the part-19 lesson, and
/// the reason `file_to_page_bytes` seeds an IMPORTED page's folder with
/// `fill_white = false`). So on such a page the underlay goes directly
/// ABOVE the White base and INSIDE the folder, at the White's depth:
/// visible in the panel, listed in the palette, still under every ink
/// layer.
///
/// With several stacked frame folders the LOWEST White wins. An underlay
/// hidden by a folder above it is a visibility disappointment; an underlay
/// on top of that folder's ink would be a wrecked page.
fn underlay_slot(doc: &Document) -> (usize, u8) {
    match doc
        .layers
        .iter()
        .position(|l| !l.folder && l.name == "White")
    {
        Some(w) => (w + 1, doc.layers[w].depth),
        None => (0, 0),
    }
}

/// Put `img` into `doc` as a 下書き draft underlay at [`underlay_slot`].
/// Returns the index it landed at.
///
/// Records NOTHING: the byte-writing callers hold documents whose history
/// is thrown away with them, and the OPEN-page caller records the whole
/// stack ONCE beforehand so its change is a single undo press.
///
/// The layer is built in a throwaway document of the same size purely to
/// reuse `add_layer_from_image`'s centring; doing that on the real document
/// would push its own "New layer" structure group, and lowering the result
/// into place a second one.
pub(super) fn place_draft_underlay(
    doc: &mut Document,
    name: String,
    img: &image::RgbaImage,
) -> usize {
    let (slot, depth) = underlay_slot(doc);
    let mut scratch = Document::new(doc.size.0, doc.size.1);
    let at = scratch.add_layer_from_image(name, img);
    let mut layer = scratch.layers.remove(at);
    layer.depth = depth;
    layer.draft = true;
    doc.layers.insert(slot, layer);
    // The active layer must still be the layer it was: the insert shifted
    // everything at or above the slot up by one.
    if doc.active >= slot {
        doc.active += 1;
    }
    doc.touch();
    slot
}

/// EXPORT-rules offscreen render (drafts hidden) of ANY document at
/// `w×h` — the shared engine of the preview tier and the reader's sharp
/// pass. Draft visibility is flipped and restored around the render: the
/// compositor's own cascade does the work, the invalidate() pair forces
/// the rebuilds, and it is presentation-only (no revision bump, no undo
/// traffic). Free function so callers can split-borrow renderer/doc out
/// of `App`.
pub(super) fn render_offscreen_drafts_off(
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
