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
            thumb: None,
            uid: Self::next_uid(),
            id: 0,
            rev: 0,
            saved_rev: 0,
            doc_rev: 0,
            spread: false,
            preview_img: None,
            prev_tex: None,
            prev_tex_px: 0.0,
            prev_tex_rev: 0,
            canvas: None,
        }
    }
}

/// TRIAGE 143: which spread operation the dialog is editing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SpreadOp {
    Combine,
    Split,
}

/// The New Comic dialog's working state.
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
        }
    }
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
            self.dock_left
                .main_surface_mut()
                .push_to_first_leaf(Palette::Pages);
        } else {
            for dock in [&mut self.dock_left, &mut self.dock_right] {
                loop {
                    // (a plain `while let` pins the iterator's temporary
                    // borrow through the body — remove_tab needs &mut.)
                    let path = dock
                        .iter_all_tabs()
                        .find(|(_, t)| **t == Palette::Pages)
                        .map(|(p, _)| p);
                    let Some(path) = path else { break };
                    dock.remove_tab(path);
                }
            }
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
            uid: PageEntry::next_uid(),
            id: 0,
            rev,
            saved_rev: 0,
            doc_rev: 0,
            spread: false,
            preview_img: None,
            prev_tex: None,
            prev_tex_px: 0.0,
            prev_tex_rev: 0,
            canvas: None,
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
    pub fn adopt_folder_state(&mut self, next_id: u32, managed: Vec<String>) {
        self.folder_next_id = next_id;
        self.folder_managed = managed;
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
            next_id: self.folder_next_id,
            pages: self
                .pages
                .iter()
                .map(|e| mn_core::project::FolderPage {
                    id: e.id,
                    rev: e.rev,
                    saved_rev: e.saved_rev,
                    bytes: e.bytes.clone().unwrap_or_default(),
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
        let meta = mn_core::ProjectMeta::for_checks(
            self.story.clone(),
            self.binding_right,
            self.page.clone(),
            self.expression,
            self.spine_mm,
            self.cover,
        );
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
        if !changed && self.pages[i].bytes.is_some() {
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
    /// already editing, keeping the rulers.
    ///
    /// Rulers live on the `Document` so the document's one undo history can
    /// own them, but they are session-only — `doc_to_bytes` does not write
    /// them and `bytes_to_doc` hands back a default set. Every in-session
    /// re-decode (page switch, spread combine/split, page replace) would
    /// therefore silently wipe a perspective set the user built, where
    /// before rulers lived on the App and survived. Carrying them here
    /// keeps the old behaviour: rulers follow the TAB, not the page.
    /// (Opening a file or making a new document does NOT come through
    /// here — a different document gets its own empty set.)
    pub fn adopt_page_doc(&mut self, doc: Document) {
        let rulers = std::mem::take(&mut self.doc.rulers);
        self.doc = doc;
        self.doc.rulers = rulers;
    }

    /// Switch the editor to another page (decode-on-switch).
    pub fn switch_page(&mut self, i: usize) {
        if i == self.page_index || i >= self.pages.len() {
            return;
        }
        let was_clean = !self.dirty();
        let old = self.page_index;
        // The eye-solo snapshot belongs to the page being left.
        self.eye_solo_backup = None;
        if let Err(e) = self.stash_current_page() {
            self.set_error(format!("page stash failed: {e}"));
            return;
        }
        let Some(bytes) = self.pages[i].bytes.take() else {
            self.set_error(format!("page {} has no data", i + 1));
            return;
        };
        match mn_core::project::bytes_to_doc(&bytes) {
            Ok(doc) => {
                self.adopt_page_doc(doc);
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
                self.fit_to_view();
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
            Err(e) => {
                // Put the bytes back; the page is not lost.
                self.pages[i].bytes = Some(bytes);
                self.set_error(format!("page {} decode failed: {e}", i + 1));
            }
        }
        self.needs_redraw = true;
    }

    /// A fresh page document matching the project's page size, seeded with a
    /// frame border folder when the project asked for one.
    pub fn blank_page_doc(&self) -> Document {
        let (w, h) = self
            .page
            .as_ref()
            .map(|p| p.paper_px())
            .unwrap_or(self.doc.size);
        self.blank_page_doc_sized(w, h)
    }

    /// Same, at an explicit size (New Comic runs before `self.doc` exists).
    pub fn blank_page_doc_sized(&self, w: u32, h: u32) -> Document {
        let mut doc = Document::new(w, h);
        if self.seed_frame_folder {
            if let Some(p) = self.page.as_ref().filter(|p| p.has_guides()) {
                let border = (0.8 / 25.4 * p.dpi.max(1) as f32).max(2.0);
                doc.add_frame_folder(
                    "Frame 1",
                    mn_core::FrameSet::single_rect(p.inner_rect_px(), border),
                );
            }
        }
        doc
    }

    /// Convert a file (.ora or image) to page ORA bytes. Used by ImportPage
    /// and ReplacePage: accepts .ora directly or wraps images as single-layer docs.
    pub fn file_to_page_bytes(&self, path: &std::path::Path) -> Result<Vec<u8>, String> {
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if ext.eq_ignore_ascii_case("ora") {
            // Already ORA: read raw bytes.
            std::fs::read(path).map_err(|e| e.to_string())
        } else {
            // Assume image: import as single-layer doc, then encode to ORA.
            let img = image::open(path).map_err(|e| e.to_string())?;
            let rgba = img.to_rgba8();
            let (w, h) = (rgba.width(), rgba.height());
            let mut doc = mn_core::Document::new(w, h);
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Imported".to_owned());
            doc.add_layer_from_image(name, &rgba);
            // Drop the empty default "Layer 1" underneath.
            if doc.layers.len() > 1 && doc.layers[1].is_empty() {
                doc.layers.remove(1);
                doc.active = 0;
            }
            mn_core::project::doc_to_bytes(&doc).map_err(|e| e.to_string())
        }
    }
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
