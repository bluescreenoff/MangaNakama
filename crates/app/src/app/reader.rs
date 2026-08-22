//! The reader (owner top item, 2026-08-18): read the chapter the way a
//! reader sees it, WITHOUT leaving the app â the loop it kills is
//! export-all â WinRAR â .cbz â CDisplayEx â fix â re-export. Double
//! spreads by default, right-to-left by default, EXPORT rules (drafts
//! off â reading over your own blue roughs is not reading the chapter),
//! fullscreen, and the edit-and-return round trip.
//!
//! Rendering is two-tier, same as the Pages palette's preview tier: the
//! FIRST paint is the stashed `mnc/preview.png` (gray-8, 1600px â nearly
//! screen resolution already), and the SHARP pass renders one page per
//! frame through the GPU compositor at the displayed size, current
//! screen first, then the neighbouring screens as prefetch. Textures
//! are keyed on the page's content revision, so returning from an edit
//! re-renders exactly the pages that moved.

use super::App;

/// How pages pair into reader screens.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ReaderMode {
    Single,
    Double,
}

/// The bottom-strip options. Defaults: double spreads, RTL (manga),
/// fit-page, no page numbers, dark background.
pub struct ReaderOpts {
    pub mode: ReaderMode,
    /// Right-to-left reading (default from the work's binding).
    pub rtl: bool,
    /// Shift the pairing by one page (a chapter that starts on the
    /// wrong foot).
    pub offset: bool,
    /// Fit the whole spread (default) vs fill the width (scrollable).
    pub fit_width: bool,
    /// "n / total" under the pages.
    pub numbers: bool,
    /// Reader background: 0 black, 1 dark gray (default), 2 white.
    pub bg: u8,
    /// 1:1 zoom (the tone-moiré check, TODO v2 ideas): one canvas px per
    /// screen px, pannable. Overrides the fit modes while on; the sharp
    /// pass renders native, so what you squint at is the true rasterized
    /// tone. Session-only (like every reader option).
    pub zoom_100: bool,
}

/// Fullscreen bookkeeping (F11): the saved window style + rect for
/// restore. Plain data so App can hold it without win32 types.
#[derive(Clone, Copy)]
pub struct FsSaved {
    pub style: isize,
    pub rect: [i32; 4],
}

pub struct ReaderState {
    pub open: bool,
    /// Screen index: spread index in Double mode, page index in Single.
    pub screen: usize,
    /// True once the reader has been opened at all â gates the
    /// "Return to reader" menu item.
    pub visited: bool,
    pub opts: ReaderOpts,
    /// Per-page display size last frame (px) â the sharp pass renders to
    /// it; >25% drift re-renders.
    pub frame_px: (f32, f32),
    /// F11 state. `fs_used` = fullscreen was entered DURING this reader
    /// session (closing the reader then restores the window).
    pub fullscreen: bool,
    pub fs_used: bool,
    pub fs_saved: Option<FsSaved>,
    /// Reader v2 (TODO v2 ideas, the proofreading loop's second half):
    /// flagged pages â optional note. In-memory v1 (a proofreading pass
    /// is one session; persistence is the recorded follow-up). The F key
    /// and the â button flag the CURRENT screen; the flag list edits
    /// notes and jumps.
    pub flags: std::collections::HashMap<usize, String>,
    /// The flag list panel's visibility.
    pub show_flags: bool,
    /// Display textures per page: (content rev, is-sharp, minted size,
    /// handle). Placeholder textures come from the preview tier; the
    /// sharp pass replaces them. A moved rev re-renders â that is the
    /// edit-and-return round trip's "only changed pages re-render".
    ///
    /// Keyed by `PageEntry::uid`, NOT by page index: a reorder (or an
    /// insert/delete before a cached page) slides every later page into
    /// somebody else's slot, and the rev guard cannot catch that because
    /// two pages can carry the SAME rev — a single-file `.mnc` loads them
    /// all at revision 0. Under a uid key, a lookup for page X can only
    /// ever return page X's art.
    pub tex: std::collections::HashMap<u64, (u64, bool, (u32, u32), egui::TextureHandle)>,
}

impl Default for ReaderState {
    fn default() -> Self {
        Self {
            open: false,
            screen: 0,
            visited: false,
            opts: ReaderOpts {
                mode: ReaderMode::Double,
                rtl: true,
                offset: false,
                fit_width: false,
                numbers: false,
                bg: 1,
                zoom_100: false,
            },
            frame_px: (600.0, 800.0),
            fullscreen: false,
            fs_used: false,
            fs_saved: None,
            flags: Default::default(),
            show_flags: false,
            tex: Default::default(),
        }
    }
}

impl ReaderState {
    /// >25% drift between the displayed size and what a texture was
    /// minted at (re-render hysteresis, same constant as the palette).
    fn size_drift(&self, minted: &(u32, u32)) -> bool {
        let (w, h) = self.frame_px;
        let (mw, mh) = (minted.0 as f32, minted.1 as f32);
        (w - mw).abs() > mw * 0.25 || (h - mh).abs() > mh * 0.25
    }
}

/// The reader's content revision for page `i`: the live doc's revision
/// for the page being edited, the stashed bytes' rev for the rest.
fn page_rev(app: &App, i: usize) -> u64 {
    if i == app.page_index {
        app.doc.revision
    } else {
        app.pages.get(i).map(|e| e.rev).unwrap_or(0)
    }
}

/// The work folder's reader state (v2.1): flagged pages + where he
/// stopped, persisted beside work.mnc so a proofreading pass survives
/// restarts and projects never see each other's state.
///
/// The position is a PAGE, not a screen. Screen indices depend on the
/// view mode and the shift-pair offset, and BOTH are session-only — a
/// Single-mode screen 90 restored into the default Double mode landed
/// near page 180. The old key was `last` (a screen); it is renamed here
/// so an old file's value is an unknown field and gets ignored rather
/// than read as a page. `serde(default)` keeps that from costing the
/// flags: a sidecar with no `last_page` still loads its notes.
#[derive(serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
struct ReaderSidecar {
    last_page: usize,
    /// Flagged page -> note ("" = flagged, not yet described).
    flags: std::collections::BTreeMap<usize, String>,
}

/// CSP-EX-style spread pairing, shared with the Pages palette: the cover
/// alone, then facing pairs in binding order â right-bound (JP) shows
/// [2|1], [4|3]â¦: the earlier page of a pair is the right-hand page.
/// `offset` shifts the pairing by one (page 0 alone on the far side).
pub fn spread_groups(n: usize, binding_right: bool, offset: bool) -> Vec<[Option<usize>; 2]> {
    let mut groups: Vec<[Option<usize>; 2]> = Vec::new();
    if n == 0 {
        return groups;
    }
    // Within a pair, the EARLIER page sits on the reading-start side:
    // right for a right-bound (JP) book, left for a left-bound one. This
    // must hold whatever index a pair starts at — the old parity-on-raw-
    // index rule flipped every spread once the offset shifted pairing onto
    // odd starts, so a right-to-left chapter read left-then-right (the
    // in-file test pinned the wrong expectation and kept the suite green).
    let earlier = if binding_right { 1usize } else { 0 };
    let mut i = 0usize;
    if !offset {
        // Cover alone: page 1 of a right-bound book is a left-hand page
        // (mirror of the Western recto), so the lone cover sits opposite
        // the reading-start side.
        let mut g = [None, None];
        g[1 - earlier] = Some(0);
        groups.push(g);
        i = 1;
    }
    // The offset ("shift pair") case pairs from page 0 with NO lone cover —
    // that is what "the chapter starts on the wrong foot" means. The old
    // code produced TWO lone screens ([0], [1], [2|3]), shifting by two.
    while i < n {
        let mut g = [None, None];
        g[earlier] = Some(i);
        if i + 1 < n {
            g[1 - earlier] = Some(i + 1);
        }
        groups.push(g);
        i += 2;
    }
    groups
}

impl App {
    // --- reader state -----------------------------------------------------

    pub fn reader_open(&mut self) {
        self.reader.open = true;
        self.reader.visited = true;
        if self.reader.screen >= self.reader_screens() {
            self.reader.screen = 0;
        }
        // Reader v2.1: a work folder carries its own reader state — flags
        // AND the last-read page — in `mnc-reader.json` beside work.mnc
        // (the r106 recorded follow-up: a proofreading pass survives
        // restarts and projects never see each other's state).
        let last = if let Some((last_page, flags)) = self.reader_load_state() {
            self.reader.flags = flags;
            last_page
        } else {
            // The r106 app-level fallback (ui.txt `reader_page=`) — the
            // only memory a folderless session has.
            self.layout.reader_page
        };
        // The saved position is a PAGE: map it to whichever screen shows
        // it under the mode/offset in force NOW. A page that no longer
        // exists maps to nothing and starts from the front.
        if self.reader.screen == 0
            && last > 0
            && let Some(s) = self.reader_screen_of_page(last)
        {
            self.reader.screen = s;
            self.set_status(format!(
                "reader — resumed at page {} (F flags a mistake)",
                last + 1
            ));
            self.needs_redraw = true;
            return;
        }
        self.set_status("reader — ←/→ turns, F11 fullscreen, F flags, Esc exits");
        self.needs_redraw = true;
    }
    pub fn reader_close(&mut self) {
        self.reader.open = false;
        // Reader v2: remember where he stopped (ui.txt via UiLayout).
        self.layout.note_reader_page(self.reader_screen_first_page());
        // v2.1: the work folder's sidecar carries flags + last page.
        self.reader_save_state();
        if self.reader.fs_used {
            self.reader_set_fullscreen(false);
        }
        self.needs_redraw = true;
    }

    /// Reader v2: flag the CURRENT screen's pages (any-flagged â unflag
    /// all) â the "this hand is wrong" marker during a proofreading pass.
    pub fn reader_toggle_flag_here(&mut self) {
        let cells = self.reader_screen_pages(self.reader.screen);
        let pages: Vec<usize> = cells.iter().flatten().copied().collect();
        if pages.is_empty() {
            return;
        }
        if pages.iter().any(|p| self.reader.flags.contains_key(p)) {
            for p in &pages {
                self.reader.flags.remove(p);
            }
            self.set_status("flag removed");
        } else {
            for p in &pages {
                self.reader.flags.entry(*p).or_default();
            }
            self.set_status(format!(
                "page{} flagged â F again to unflag, ⚑ opens the list",
                if pages.len() > 1 { "s" } else { "" }
            ));
        }
        self.needs_redraw = true;
        self.reader_save_state();
    }

    /// Reader v2: note a flagged page (the flag list's text fields).
    pub fn reader_set_note(&mut self, page: usize, note: &str) {
        if self.reader.flags.contains_key(&page) {
            self.reader.flags.insert(page, note.to_owned());
            self.reader_save_state();
        }
    }

    /// Reader v2: unflag one page (the flag list's ✕).
    pub fn reader_unflag(&mut self, page: usize) {
        if self.reader.flags.remove(&page).is_some() {
            self.reader_save_state();
        }
    }

    /// Reader v2.1: 1:1 zoom — the tone-moiré check (TODO's last v2-ideas
    /// row). One canvas px per screen px, drag pans; the fit modes return
    /// on the second press.
    pub fn reader_toggle_zoom(&mut self) {
        self.reader.opts.zoom_100 = !self.reader.opts.zoom_100;
        self.set_status(if self.reader.opts.zoom_100 {
            "1:1 — tone moiré check (drag to pan; 1 or the bar button returns to fit)"
        } else {
            "reader — fit view"
        });
        self.needs_redraw = true;
    }

    /// Page `i`'s TRUE canvas size: the live doc for the active page, the
    /// stashed ORA's stack.xml for the rest (cached on the entry — no
    /// pixel decode). The 1:1 view needs exact pixels; a combined spread
    /// is a wider page than `doc.size`.
    pub fn reader_page_canvas(&mut self, i: usize) -> (u32, u32) {
        if i == self.page_index {
            return self.doc.size;
        }
        let Some(e) = self.pages.get_mut(i) else {
            return self.doc.size;
        };
        if e.canvas.is_none() {
            let sz = e
                .bytes
                .as_ref()
                .and_then(|b| mn_core::ora::ora_canvas_size(b));
            if sz.is_some() {
                e.canvas = sz;
            }
        }
        e.canvas.unwrap_or(self.doc.size)
    }

    // --- reader sidecar (v2.1) --------------------------------------------

    /// The work folder's reader-state file, when the session is
    /// folder-backed (`doc_path` = the work.mnc index). Folderless
    /// sessions keep the in-memory v1 + the ui.txt fallback.
    fn reader_sidecar_path(&self) -> Option<std::path::PathBuf> {
        let p = self.doc_path.as_ref()?;
        if p.file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case("work.mnc"))
        {
            p.parent().map(|d| d.join("mnc-reader.json"))
        } else {
            None
        }
    }

    /// Persist flags + last screen to the sidecar (tmp + rename, atomic —
    /// a crash never leaves a half-written file). A failed write never
    /// blocks the reader: the state just stays in memory (same policy as
    /// a failed preview render).
    pub fn reader_save_state(&mut self) {
        let Some(path) = self.reader_sidecar_path() else {
            return;
        };
        let sc = ReaderSidecar {
            last_page: self.reader_screen_first_page(),
            flags: self
                .reader
                .flags
                .iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect(),
        };
        let Ok(json) = serde_json::to_string(&sc) else {
            return;
        };
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json)
            .and_then(|_| std::fs::rename(&tmp, &path))
            .is_err()
        {
            self.set_status("reader state not saved (folder unwritable)");
        }
    }

    /// Load the sidecar, dropping flags for pages that no longer exist.
    /// `None` when there is no folder, no file, or the JSON is unreadable
    /// (stale/corrupt sidecars start fresh rather than guess).
    fn reader_load_state(&mut self) -> Option<(usize, std::collections::HashMap<usize, String>)> {
        let path = self.reader_sidecar_path()?;
        let s = std::fs::read_to_string(path).ok()?;
        let sc: ReaderSidecar = serde_json::from_str(&s).ok()?;
        let n = self.pages.len();
        Some((
            sc.last_page,
            sc.flags.into_iter().filter(|(p, _)| *p < n).collect(),
        ))
    }

    /// Reader v2: open the screen that shows page `i` (the flag list's Go).
    pub fn reader_goto_page(&mut self, i: usize) {
        if let Some(s) = self.reader_screen_of_page(i) {
            self.reader.screen = s;
            self.needs_redraw = true;
        }
    }

    /// Which screen shows page `i` under the CURRENT mode/offset — the
    /// half of the persistence contract that turns a saved page back
    /// into a position. `None` for a page that no longer exists.
    pub fn reader_screen_of_page(&self, i: usize) -> Option<usize> {
        if i >= self.pages.len() {
            return None;
        }
        (0..self.reader_screens()).find(|&s| self.reader_screen_pages(s).contains(&Some(i)))
    }

    /// The FIRST page of the screen being read — the mode-independent
    /// position the reader persists. A spread's earlier page, whichever
    /// cell the binding puts it in.
    pub fn reader_screen_first_page(&self) -> usize {
        self.reader_screen_pages(self.reader.screen)
            .iter()
            .flatten()
            .copied()
            .min()
            .unwrap_or(0)
    }

    pub fn reader_return(&mut self) {
        self.reader.open = true;
        self.needs_redraw = true;
    }

    /// The display texture for page `i` — the ONE place the reader's
    /// texture map is read (the overlay draws through it too), so the
    /// index-to-identity step cannot be forgotten at a call site.
    pub fn reader_tex(&self, i: usize) -> Option<&(u64, bool, (u32, u32), egui::TextureHandle)> {
        self.reader.tex.get(&self.page_uid(i))
    }

    /// Page `i`'s runtime identity; 0 (no page) past the end.
    fn page_uid(&self, i: usize) -> u64 {
        self.pages.get(i).map_or(0, |e| e.uid)
    }

    /// Number of reader screens under the current mode/pairing.
    pub fn reader_screens(&self) -> usize {
        match self.reader.opts.mode {
            ReaderMode::Single => self.pages.len(),
            ReaderMode::Double => spread_groups(
                self.pages.len(),
                self.binding_right,
                self.reader.opts.offset,
            )
            .len(),
        }
    }

    /// The page indices the given screen shows ([left, right] cells as
    /// laid out on screen; in RTL the right cell reads first).
    pub fn reader_screen_pages(&self, screen: usize) -> [Option<usize>; 2] {
        match self.reader.opts.mode {
            ReaderMode::Single => [None, Some(screen.min(self.pages.len().saturating_sub(1)))],
            ReaderMode::Double => {
                let groups = spread_groups(
                    self.pages.len(),
                    self.binding_right,
                    self.reader.opts.offset,
                );
                groups.get(screen).copied().unwrap_or([None, None])
            }
        }
    }

    /// Turn `delta` screens (delta < 0 = backwards). No clamping loop â
    /// the UI disables the ends.
    pub fn reader_turn(&mut self, delta: i32) {
        let s = self.reader.screen as i32 + delta;
        let max = self.reader_screens() as i32 - 1;
        self.reader.screen = s.clamp(0, max.max(0)) as usize;
        self.needs_redraw = true;
    }

    /// The reading-direction-aware turn: which horizontal delta a click
    /// on the LEFT third of the reader means (next in RTL, prev in LTR).
    pub fn reader_left_delta(&self) -> i32 {
        if self.reader.opts.rtl { 1 } else { -1 }
    }

    /// The edit-and-return round trip: jump into the editor on page `i`,
    /// remembering where the reader was.
    pub fn reader_edit_page(&mut self, i: usize) {
        if i >= self.pages.len() {
            return;
        }
        self.reader.open = false;
        if self.reader.fs_used {
            self.reader_set_fullscreen(false);
        }
        if i != self.page_index {
            self.switch_page(i);
        }
        self.set_status("editing â Manga â¸ Return to reader when done");
        self.needs_redraw = true;
    }

    /// F11. Applies through the window handle when we have one (tests
    /// run headless with hwnd == 0 â state only).
    pub fn reader_toggle_fullscreen(&mut self) {
        self.reader_set_fullscreen(!self.reader.fullscreen);
    }

    pub fn reader_set_fullscreen(&mut self, on: bool) {
        if on == self.reader.fullscreen {
            return;
        }
        self.reader.fullscreen = on;
        if on {
            self.reader.fs_used = true;
        }
        if self.hwnd != 0 {
            unsafe {
                crate::win32::set_window_fullscreen(self.hwnd, on, &mut self.reader.fs_saved)
            };
        }
        self.needs_redraw = true;
    }

    // --- reader rendering -------------------------------------------------

    /// Per-frame reader work: mint preview placeholders for the current
    /// screen, then ONE sharp render (current screen first, then the
    /// neighbours as prefetch). Never more than one per frame â a page
    /// turn fills in within a few frames, and the placeholder is on
    /// screen instantly.
    pub fn reader_frame(&mut self) {
        if !self.reader.open || self.pages.is_empty() {
            return;
        }
        let max = self.reader_screens() - 1;
        let cur = self.reader.screen.min(max);
        let order: Vec<usize> = {
            let mut v: Vec<[Option<usize>; 2]> = vec![self.reader_screen_pages(cur)];
            if cur + 1 <= max {
                v.push(self.reader_screen_pages(cur + 1));
            }
            if cur > 0 {
                v.push(self.reader_screen_pages(cur - 1));
            }
            v.iter().flat_map(|g| g.iter().copied().flatten()).collect()
        };

        // Placeholders: preview-tier textures for pages with no texture
        // at all (cheap, whole current screen).
        for &i in &order {
            let rev = page_rev(self, i);
            let stale = self.reader_tex(i).is_none_or(|(r, ..)| *r != rev);
            if stale
                && i != self.page_index
                && let Some(gray) = self.preview_for(i)
            {
                let (w, h) = gray.dimensions();
                let mut ci = egui::ColorImage::new(
                    [w as usize, h as usize],
                    vec![egui::Color32::WHITE; (w as usize) * (h as usize)],
                );
                for y in 0..h as usize {
                    for x in 0..w as usize {
                        ci[(x, y)] =
                            egui::Color32::from_gray(gray.get_pixel(x as u32, y as u32)[0]);
                    }
                }
                let uid = self.page_uid(i);
                let t = self.shell.ctx.load_texture(
                    format!("mn.reader.{uid}"),
                    ci,
                    egui::TextureOptions::LINEAR,
                );
                self.reader.tex.insert(uid, (rev, false, (w, h), t));
            }
        }

        // One sharp render per frame, in screen order. At 1:1 each page
        // renders at its OWN canvas size (a spread is wider), so the
        // shared frame_px drift check does not apply there.
        for &i in &order {
            let rev = page_rev(self, i);
            let needs = self.reader_tex(i).is_none_or(|(r, sharp, sz, _)| {
                *r != rev || !*sharp || (!self.reader.opts.zoom_100 && self.reader.size_drift(sz))
            });
            if needs {
                self.reader_render_sharp(i);
                break;
            }
        }

        // Bound the texture map — it grew one full-page texture per page
        // turned, forever (at 1:1 that was hundreds of MB across a
        // chapter). Past the cap, keep only the current screen and its
        // prefetch neighbours; anything else re-renders on revisit.
        const TEX_CAP: usize = 12;
        if self.reader.tex.len() > TEX_CAP {
            let keep: std::collections::HashSet<u64> =
                order.iter().map(|&i| self.page_uid(i)).collect();
            self.reader.tex.retain(|k, _| keep.contains(k));
        }
    }

    /// The sharp pass: export rules (drafts off) at the displayed size,
    /// through the GPU compositor. The active page renders from the live
    /// doc; others decode from their stashed bytes. The canvas thrash is
    /// bounded â while the reader is open the editor is not drawing, so
    /// the shared canvas is free real estate, and closing the reader
    /// pays exactly one rebuild back.
    fn reader_render_sharp(&mut self, i: usize) {
        // At 1:1 the render is the page's native size (the moiré check is
        // honest only on true pixels); otherwise the displayed size.
        let (tw, th) = if self.reader.opts.zoom_100 {
            let (w, h) = self.reader_page_canvas(i);
            // Long-edge cap. A 600 dpi B4 page is 6071×8598 — ~208 MB as
            // RGBA and TALLER than common 8192 texture limits, and a
            // combined spread's readback row-padding blows wgpu's 256 MB
            // max_buffer_size outright. Below the cap (A4/B5 at 300–350
            // dpi) 1:1 stays true; above it the page scales instead of
            // OOMing. The honest fix for monster pages is a visible-crop
            // render through render_offscreen_vp — recorded in TODO.
            const MAX_EDGE: f32 = 4096.0;
            let s = (MAX_EDGE / w.max(h).max(1) as f32).min(1.0);
            (w as f32 * s, h as f32 * s)
        } else {
            self.reader.frame_px
        };
        let (w, h) = (tw.round().max(1.0) as u32, th.round().max(1.0) as u32);
        let img = if i == self.page_index {
            let Self { renderer, doc, .. } = self;
            super::pages::render_offscreen_drafts_off(renderer, doc, w, h)
        } else {
            let Some(bytes) = self.pages.get(i).and_then(|e| e.bytes.clone()) else {
                return;
            };
            let Ok(mut doc) = mn_core::project::bytes_to_doc(&bytes) else {
                return;
            };
            let Self { renderer, .. } = self;
            super::pages::render_offscreen_drafts_off(renderer, &mut doc, w, h)
        };
        let rev = if i == self.page_index {
            self.doc.revision
        } else {
            self.pages[i].rev
        };
        let (iw, ih) = (img.width(), img.height());
        let ci = egui::ColorImage::from_rgba_unmultiplied([iw as usize, ih as usize], img.as_raw());
        let uid = self.page_uid(i);
        let t = self.shell.ctx.load_texture(
            format!("mn.reader.{uid}"),
            ci,
            egui::TextureOptions::LINEAR,
        );
        self.reader.tex.insert(uid, (rev, true, (iw, ih), t));
    }
}

#[cfg(test)]
mod tests {
    use super::spread_groups;

    /// The pairing the Pages palette ships (cover alone, then facing
    /// pairs, earlier page on the reading-start side), the offset shift,
    /// and the degenerate sizes. If this ever changes, the panel and the
    /// reader change TOGETHER â they share the fn.
    #[test]
    fn pairing_covers_shifts_and_respects_binding() {
        // Right-bound (JP), 4 pages: [cover], [2|1], [3].
        assert_eq!(
            spread_groups(4, true, false),
            vec![[Some(0), None], [Some(2), Some(1)], [None, Some(3)]]
        );
        // Offset ("shift pair"): the chapter starts on the wrong foot, so
        // there is NO lone cover — pairing runs from page 0, earlier page
        // still on the reading-start (right) side: [1|0], [3|2].
        assert_eq!(
            spread_groups(4, true, true),
            vec![[Some(1), Some(0)], [Some(3), Some(2)]]
        );
        // Left-bound: cover sits right, then consecutive pairs [1|2], [3].
        assert_eq!(
            spread_groups(4, false, false),
            vec![[None, Some(0)], [Some(1), Some(2)], [Some(3), None]]
        );
        // Degenerate.
        assert!(spread_groups(0, true, false).is_empty());
        assert_eq!(spread_groups(1, true, false), vec![[Some(0), None]]);
        // Offset on a 1-page work must not eat the page (it shows alone on
        // the reading-start side — there is no cover slot in offset mode).
        assert_eq!(spread_groups(1, true, true), vec![[None, Some(0)]]);
    }

    /// The Pages palette lays every row out on the SAME two-cell grid, so
    /// a cell is one size for the whole palette (owner report 2026-08-22:
    /// "page 1 renders much bigger than pages 2-3"). The guarantee the
    /// panel leans on is structural: every group is a fixed `[Option; 2]`,
    /// so the lone cover occupies ONE half of its row — the half it would
    /// sit in for the binding — and never the whole width. The 3-page
    /// right-bound work from the report is the case.
    #[test]
    fn every_row_is_a_two_cell_grid() {
        let groups = spread_groups(3, true, false);
        assert_eq!(
            groups,
            vec![[Some(0), None], [Some(2), Some(1)]],
            "cover alone on the left (right-bound), then the 3|2 spread"
        );
        // Same slot count per row => the palette's `avail / 2` cell width
        // is the width of EVERY cell, lone cover included.
        for g in &groups {
            assert_eq!(g.len(), 2);
        }
        // The cover keeps its binding-side half rather than being centred
        // or stretched: right-bound => the left slot.
        assert_eq!(groups[0][0], Some(0));
        assert_eq!(groups[0][1], None);
    }
}
