//! On-canvas text editing (T tool) and text-object dragging (O tool).
//!
//! Editing model: a session edits ONE item on ONE text layer. Every keystroke
//! mutates the layer's `TextSet` **directly** (no history) and re-rasterizes,
//! so the canvas is always live; `commit()` quietly restores the session's
//! starting state and then issues one `Document::set_texts`, which pushes a
//! single undo step for the whole session — one Ctrl+Z per text box on the
//! DOCUMENT's stack.
//!
//! **Inside the session there is a second, smaller undo stack** (owner
//! report, 2026-08-19: *"Ctrl+Z seems to remove an entire text rather than
//! edit something in it"* — which is what a document-only stack does, and
//! not what any editor does). While you are typing, Ctrl+Z steps back
//! through your edits; when that stack runs out it ends the session, and
//! undoes the box itself only if the session actually left a document step
//! behind — otherwise the press would eat somebody else's. Typing coalesces
//! into word-sized steps rather than one per character; a Tool Property
//! change is a step too, and a whole value-bar drag is one of them.
//!
//! All indices are UTF-16 code units end to end (`core::text` contract).
//! Geometry: the engine answers in unrotated box-local px; conversions to
//! canvas go through `TextItem::to_local`/`to_canvas`.

use std::sync::Arc;

use crate::app::App;
use crate::cmd::{AppCmd, Tool};
use mn_core::LayerKind;
use mn_core::text::{self as ct, RenderedText, StyleFlag, TextHandle, TextItem, TextSet};

/// How far (px, canvas at zoom 1 = screen-scaled by callers) the rotate
/// lollipop floats above the box; screen-constant via /zoom at call sites.
pub const ROTATE_OFFSET_SCREEN: f32 = 22.0;

pub struct TextEditState {
    pub layer: usize,
    pub index: usize,
    /// The layer's whole vector state when the session began; the single undo
    /// step's pre-image, and what `cancel()` restores.
    pub before: TextSet,
    /// Selection = `anchor..caret` (caret is the moving end).
    pub caret: u32,
    pub anchor: u32,
    /// Cross-axis goal for repeated up/down (column) motion.
    pub goal: Option<f32>,
    /// This session created the layer itself (an empty commit removes it).
    pub new_layer: bool,
    /// UTF-16 high surrogate waiting for its pair (WM_CHAR arrives in halves).
    pub pending_surrogate: Option<u16>,
    /// IN-EDITOR undo: the item as it stood BEFORE each mutating burst, with
    /// the caret to put back. Oldest first; capped, because a session is not
    /// a place to keep an unbounded history.
    pub undo: Vec<(TextItem, u32)>,
    /// True while the last mutation was plain typing, so a run of characters
    /// collapses into ONE undo step. Anything else — a space, a newline, a
    /// deletion, a style or furigana change — ends the run, which is what
    /// makes Ctrl+Z step back by word instead of by letter.
    pub typing_run: bool,
    /// True between a value bar's first changed frame and its release, so the
    /// DRAG gets one pre-image instead of one per frame (`typing_run` for
    /// sliders). Any other edit clears it, the same way it ends a typing run.
    pub bar_run: bool,
}

impl TextEditState {
    pub fn selection(&self) -> (u32, u32) {
        (self.anchor.min(self.caret), self.anchor.max(self.caret))
    }

    pub fn has_selection(&self) -> bool {
        self.anchor != self.caret
    }
}

/// T-tool press gesture: dragging a selection inside the edited item, or
/// dragging out a fixed wrap box on empty canvas.
pub enum TextGesture {
    Select,
    Box { start: (f32, f32), cur: (f32, f32) },
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TextDragMode {
    MoveWhole,
    Handle(TextHandle),
}

/// Object-tool drag on a text box; same lifecycle as `BalloonObjDrag` — the
/// document keeps the original until release.
pub struct TextObjDrag {
    pub layer: usize,
    pub index: usize,
    pub mode: TextDragMode,
    pub start: (f32, f32),
    pub cur: (f32, f32),
    pub orig: TextItem,
}

impl TextObjDrag {
    pub fn preview(&self) -> TextItem {
        let mut t = self.orig.clone();
        match self.mode {
            TextDragMode::MoveWhole => {
                t.translate(self.cur.0 - self.start.0, self.cur.1 - self.start.1)
            }
            TextDragMode::Handle(h) => t.apply_handle(h, [self.cur.0, self.cur.1]),
        }
        t
    }

    pub fn moved(&self) -> bool {
        (self.cur.0 - self.start.0).abs() + (self.cur.1 - self.start.1).abs() > 0.5
    }
}

/// Caret geometry for the overlay: a line segment in canvas coords plus the
/// selection highlight quads (4 canvas corners each).
pub struct TextCaretOverlay {
    pub caret: [[f32; 2]; 2],
    pub selection: Vec<[[f32; 2]; 4]>,
}

/// A Tool-Property VALUE-BAR drag on the Object-selected text item
/// (auditor round 34, MEDIUM): the bar's changed frames apply as LIVE
/// preview with no history, and the whole interaction commits as ONE undo
/// step on release — `commit_text_edit`'s rewind trick, drag-shaped. The
/// bracket is opened by the first `begin_text_bar_drag` of an interaction;
/// every bar's release arm closes it.
pub struct TextBarDrag {
    pub layer: usize,
    /// The pre-drag snapshot — the single undo step's pre-image (the whole
    /// set; the commit swaps it wholesale, no per-item index needed).
    pub before: TextSet,
}

impl App {
    // --- basics -----------------------------------------------------------

    pub fn text_editing(&self) -> bool {
        self.text_edit.is_some()
    }

    pub fn doc_dpi(&self) -> u32 {
        self.page.as_ref().map(|p| p.dpi).unwrap_or(0)
    }

    /// Remember a font as just used (CSP Font list "Recently used": newest
    /// first, no duplicates, max 10) and persist it through ui.txt.
    pub fn note_recent_font(&mut self, font: &str) {
        self.recent_fonts.retain(|f| f != font);
        self.recent_fonts.insert(0, font.to_string());
        self.recent_fonts.truncate(10);
        self.layout.note_recent_fonts(&self.recent_fonts);
    }

    /// The item under edit, straight from the document (the live copy).
    pub fn edited_item(&self) -> Option<&TextItem> {
        let ed = self.text_edit.as_ref()?;
        self.doc.layers.get(ed.layer)?.texts()?.texts.get(ed.index)
    }

    /// TX-styles: re-stamp `style` onto every current-page item carrying
    /// its name — shape each restyled item, then commit per layer through
    /// `set_texts` (raster + undo), the whole page wrapped into ONE undo
    /// press. Returns the number of items restyled.
    pub fn apply_text_style_current(&mut self, style: &mn_core::text::TextStyle) -> usize {
        let dpi = self.doc_dpi();
        let mut groups = 0usize;
        let mut items = 0usize;
        for li in 0..self.doc.layers.len() {
            let hit = |t: &mn_core::text::TextItem| t.style.as_deref() == Some(style.name.as_str());
            if !self
                .doc
                .layers
                .get(li)
                .and_then(|l| l.texts())
                .is_some_and(|ts| ts.texts.iter().any(hit))
            {
                continue;
            }
            // ORA-loaded layers may still be cache-less: warm BEFORE the
            // clone or the re-raster drops the untouched sprites too.
            self.warm_texts(li);
            let Some(ts) = self.doc.layers.get(li).and_then(|l| l.texts()) else {
                continue;
            };
            let mut ts = ts.clone();
            let Self {
                doc, text_engine, ..
            } = self;
            let Some(engine) = text_engine.as_ref() else {
                return items;
            };
            for item in ts.texts.iter_mut().filter(|t| hit(t)) {
                style.apply(item);
                item.cache = engine.render(item, dpi).ok().flatten();
                items += 1;
            }
            if doc.set_texts(li, ts) {
                groups += 1;
            }
        }
        if groups > 1 {
            self.doc.wrap_recent("Text style", groups);
        }
        items
    }

    /// TX-styles, the chapter-wide half: push the live document's style
    /// list onto every OTHER page and re-style their items. Same round
    /// trip (and the same honesty) as batch: other pages are saved
    /// directly, undo covers the open page only. Returns (pages, items).
    pub fn apply_text_styles_other_pages(&mut self) -> (usize, usize) {
        if self.stash_current_page().is_err() {
            return (0, 0);
        }
        let dpi = self.doc_dpi();
        let styles = self.doc.text_styles.clone();
        let (mut pages, mut items) = (0usize, 0usize);
        for i in 0..self.pages.len() {
            if i == self.page_index {
                continue;
            }
            let Some(bytes) = self.pages[i].bytes.as_deref() else {
                continue;
            };
            let Ok(mut doc) = mn_core::project::bytes_to_doc(bytes) else {
                continue;
            };
            doc.text_styles = styles.clone();
            let mut page_items = 0usize;
            for li in 0..doc.layers.len() {
                let Some(ts) = doc.layers.get(li).and_then(|l| l.texts()) else {
                    continue;
                };
                let mut ts = ts.clone();
                let mut touched = false;
                // A decoded page has NO caches: shape every item, or the
                // re-raster would erase the sprites styles never touched.
                let Some(engine) = self.text_engine.as_ref() else {
                    return (pages, items);
                };
                for item in ts.texts.iter_mut() {
                    if let Some(s) = item
                        .style
                        .as_deref()
                        .and_then(|n| styles.iter().find(|s| s.name == n))
                    {
                        s.apply(item);
                        touched = true;
                        page_items += 1;
                    }
                    item.cache = engine.render(item, dpi).ok().flatten();
                }
                if touched {
                    doc.set_texts(li, ts);
                }
            }
            if page_items == 0 {
                continue;
            }
            let Ok(nb) = mn_core::project::doc_to_bytes(&doc) else {
                continue;
            };
            let rev = self.page_rev_next();
            let e = &mut self.pages[i];
            e.bytes = Some(nb);
            e.rev = rev;
            e.doc_rev = 0;
            e.thumb = None;
            pages += 1;
            items += page_items;
        }
        self.mark_pages_dirty();
        (pages, items)
    }

    /// Fill missing sprite caches on a text layer (ORA-loaded layers have
    /// none). Must run before the first edit — see `Document::warm_text_caches`.
    pub fn warm_texts(&mut self, layer: usize) {
        let dpi = self.doc_dpi();
        let Self {
            doc, text_engine, ..
        } = self;
        let Some(engine) = text_engine.as_ref() else {
            return;
        };
        doc.warm_text_caches(layer, |item| engine.render(item, dpi).ok().flatten());
    }

    /// Remember the item before a mutation, for the IN-EDITOR undo stack.
    ///
    /// `typing` marks a plain character insert: a run of them coalesces into
    /// one step, so Ctrl+Z steps back by word rather than by letter. Every
    /// other edit — a space, a newline, a deletion, a style, a reading —
    /// ends the run and gets its own step.
    fn snapshot_edit(&mut self, typing: bool) {
        let Some(item) = self.edited_item().cloned() else {
            return;
        };
        let Some(ed) = self.text_edit.as_mut() else {
            return;
        };
        if typing && ed.typing_run {
            return;
        }
        ed.undo.push((item, ed.caret));
        // A session is not a place to keep an unbounded history; the
        // document's own stack is behind this one.
        if ed.undo.len() > 200 {
            ed.undo.remove(0);
        }
        ed.typing_run = typing;
        ed.bar_run = false;
    }

    /// Take back the pre-image a mutation turned out not to need.
    ///
    /// `set_ruby`/`set_font_range` report whether they changed anything, and
    /// when they did not, the entry `snapshot_edit` just pushed would cost the
    /// reader a Ctrl+Z that restores what is already on screen — a press that
    /// visibly does nothing, which reads as a broken undo.
    fn drop_last_snapshot(&mut self) {
        if let Some(ed) = self.text_edit.as_mut() {
            ed.undo.pop();
        }
    }

    /// Step back one in-editor edit. False when the stack is empty, which is
    /// the caller's signal to fall through to the document's undo (removing
    /// the whole text box — the right LAST step, and it used to be the only
    /// one).
    pub fn text_undo_step(&mut self) -> bool {
        let Some(ed) = self.text_edit.as_mut() else {
            return false;
        };
        let Some((item, caret)) = ed.undo.pop() else {
            return false;
        };
        ed.typing_run = false;
        ed.caret = caret;
        ed.anchor = caret;
        ed.goal = None;
        self.with_edited_item(move |it| *it = item);
        true
    }

    /// Mutate the edited item, re-shape its sprite, re-rasterize the layer.
    /// No document history — commit() turns the whole session into one undo
    /// step; `snapshot_edit` feeds the in-session stack.
    fn with_edited_item(&mut self, f: impl FnOnce(&mut TextItem)) {
        let Some(ed) = self.text_edit.as_ref() else {
            return;
        };
        let (li, ix) = (ed.layer, ed.index);
        let dpi = self.doc_dpi();
        let Self {
            doc, text_engine, ..
        } = self;
        let Some(engine) = text_engine.as_ref() else {
            return;
        };
        let size = doc.size;
        let Some(layer) = doc.layers.get_mut(li) else {
            return;
        };
        let LayerKind::Text(ts) = &mut layer.kind else {
            return;
        };
        let Some(item) = ts.texts.get_mut(ix) else {
            return;
        };

        f(item);

        if item.auto_size {
            if let Ok(natural) = engine.natural_size(item, dpi) {
                // Vertical text grows leftward (columns are right-to-left):
                // keep the top-RIGHT corner planted; horizontal keeps top-left.
                if item.vertical {
                    item.pos[0] += item.size[0] - natural[0];
                }
                item.size = natural;
            }
        }
        item.cache = engine.render(item, dpi).ok().flatten();
        let raster = ts.rasterize(size);
        layer.replace_tiles(raster);
        doc.touch();
        self.mark_dirty();
    }

    // --- session lifecycle -------------------------------------------------

    /// Begin editing an existing item, caret at the clicked canvas point.
    pub fn start_text_edit(&mut self, layer: usize, index: usize, at: Option<[f32; 2]>) {
        self.commit_text_edit();
        self.warm_texts(layer);
        let Some(ts) = self.doc.layers.get(layer).and_then(|l| l.texts()) else {
            return;
        };
        let before = ts.clone();
        let Some(item) = ts.texts.get(index) else {
            return;
        };
        let dpi = self.doc_dpi();
        let caret = match (at, self.text_engine.as_ref()) {
            (Some(p), Some(e)) => {
                let l = item.to_local(p);
                e.hit_test_point(item, dpi, l).unwrap_or(0)
            }
            _ => item.utf16_len(),
        };
        self.text_edit = Some(TextEditState {
            layer,
            index,
            before,
            caret,
            anchor: caret,
            goal: None,
            new_layer: false,
            pending_surrogate: None,
            undo: Vec::new(),
            typing_run: false,
            bar_run: false,
        });
        self.text_sel = None;
        self.mark_dirty();
    }

    /// Create a new item (auto box at a click point, or a dragged fixed box)
    /// and enter editing. Appends to the active layer when it is a text
    /// layer, else creates "Text N" at the top (clears history, like the
    /// other structural layer ops).
    pub fn start_new_text(&mut self, origin: [f32; 2], fixed_box: Option<[f32; 2]>) {
        self.commit_text_edit();
        let Some(engine) = self.text_engine.as_ref() else {
            self.set_status("text engine unavailable (DirectWrite init failed)");
            return;
        };
        let font = if self.text_font.is_empty() {
            engine.default_family()
        } else {
            self.text_font.clone()
        };
        let color = {
            let c = self.active_color();
            [
                (c[0] * 255.0).round() as u8,
                (c[1] * 255.0).round() as u8,
                (c[2] * 255.0).round() as u8,
            ]
        };
        let mut item = TextItem::new(origin, font, self.text_size_pt, color, self.text_vertical);
        item.outline_px = self.mm_to_px(self.text_outline_mm).max(0.0);
        item.outline_color = self.text_outline_color;
        // TX-styles: new text carries the picked work style (the palette
        // already synced the defaults when it was picked).
        item.style = self
            .text_style_new
            .clone()
            .filter(|n| self.doc.text_styles.iter().any(|s| &s.name == n));
        // Round-34 typography defaults ride along (CSP: palette values apply
        // to new text).
        item.align = self.text_align;
        item.frame_align = self.text_frame_align;
        item.letter_spacing_pt = self.text_letter_pt;
        item.line_spacing = self.text_line;
        // Auto 縦中横 (TX-062) rides along the same way: it is a property of
        // how this box is set, not of the characters, so it belongs on the
        // item and is seeded from Tool Property like the rest.
        item.auto_tcy = self.text_auto_tcy;
        let dpi = self.doc_dpi();
        if let Some(size) = fixed_box {
            item.size = size;
            item.auto_size = false;
        } else {
            item.size = engine.natural_size(&item, dpi).unwrap_or([8.0, 8.0]);
            if item.vertical {
                // Click point = top-right of the (leftward-growing) column.
                item.pos[0] -= item.size[0];
            }
        }

        let (layer, new_layer, before) = match self.doc.layers.get(self.doc.active) {
            Some(l) if l.is_text() => (
                self.doc.active,
                false,
                l.texts().cloned().unwrap_or_default(),
            ),
            _ => {
                let n = 1 + self.doc.layers.iter().filter(|l| l.is_text()).count();
                let li = self
                    .doc
                    .add_text_layer(format!("Text {n}"), TextSet::default());
                (li, true, TextSet::default())
            }
        };
        self.warm_texts(layer);

        let index = {
            let Some(l) = self.doc.layers.get_mut(layer) else {
                return;
            };
            let LayerKind::Text(ts) = &mut l.kind else {
                return;
            };
            ts.texts.push(item);
            ts.texts.len() - 1
        };
        self.text_edit = Some(TextEditState {
            layer,
            index,
            before,
            caret: 0,
            anchor: 0,
            goal: None,
            new_layer,
            pending_surrogate: None,
            undo: Vec::new(),
            typing_run: false,
            bar_run: false,
        });
        self.text_sel = None;
        // Rasterize once so a fixed box shows immediately (empty = no ink).
        self.with_edited_item(|_| {});
        self.set_status("type text — Esc commits");
    }

    /// End the session: one undo step for everything it changed. Empty new
    /// items are removed instead (and their layer, when this session made it).
    ///
    /// **Returns whether it pushed that step.** A session whose net effect is
    /// nothing pushes none, and a caller that queues an `AppCmd::Undo` on the
    /// assumption that it did would pop a step belonging to somebody else —
    /// see the Ctrl+Z arm in `text_key`.
    pub fn commit_text_edit(&mut self) -> bool {
        let Some(ed) = self.text_edit.take() else {
            return false;
        };
        let li = ed.layer;
        let Some(layer) = self.doc.layers.get_mut(li) else {
            return false;
        };
        let LayerKind::Text(ts) = &mut layer.kind else {
            return false;
        };
        let mut working = ts.clone();
        if let Some(item) = working.texts.get(ed.index) {
            if item.text.is_empty() {
                working.texts.remove(ed.index);
            }
        }
        if working == ed.before {
            // Nothing changed (or an empty new item evaporated): restore the
            // starting state without touching history.
            *ts = ed.before.clone();
            let raster = ts.rasterize(self.doc.size);
            layer.replace_tiles(raster);
            if ed.new_layer && working.texts.is_empty() {
                self.doc.remove_layer(li);
                self.renderer.invalidate();
            }
            self.mark_dirty();
            return false;
        }
        // Quietly rewind to the pre-session state, then apply the result as
        // ONE undoable step.
        *ts = ed.before.clone();
        self.doc.set_texts(li, working);
        self.mark_dirty();
        true
    }

    /// Drop the session and restore the pre-session state (no undo step).
    pub fn cancel_text_edit(&mut self) {
        let Some(ed) = self.text_edit.take() else {
            return;
        };
        let li = ed.layer;
        if let Some(layer) = self.doc.layers.get_mut(li) {
            if let LayerKind::Text(ts) = &mut layer.kind {
                *ts = ed.before.clone();
                let raster = ts.rasterize(self.doc.size);
                layer.replace_tiles(raster);
            }
        }
        if ed.new_layer {
            self.doc.remove_layer(li);
            self.renderer.invalidate();
        }
        self.doc.touch();
        self.mark_dirty();
    }

    // --- T-tool pointer gestures -------------------------------------------

    pub fn text_tool_down(&mut self, cx: f32, cy: f32, shift: bool) {
        // Click inside the edited box: move the caret (Shift extends).
        if let Some(item) = self.edited_item() {
            if item.contains([cx, cy], 2.0) {
                let dpi = self.doc_dpi();
                let l = item.to_local([cx, cy]);
                if let Some(e) = self.text_engine.as_ref() {
                    if let Ok(pos) = e.hit_test_point(item, dpi, l) {
                        let ed = self.text_edit.as_mut().unwrap();
                        ed.caret = pos;
                        if !shift {
                            ed.anchor = pos;
                        }
                        ed.goal = None;
                    }
                }
                self.text_gesture = Some(TextGesture::Select);
                self.mark_dirty();
                return;
            }
        }
        self.commit_text_edit();
        // Click on another text item (topmost text layer wins): edit it.
        for li in (0..self.doc.layers.len()).rev() {
            let l = &self.doc.layers[li];
            if !l.visible {
                continue;
            }
            if let Some(ts) = l.texts() {
                if let Some(ti) = ts.text_at([cx, cy], 2.0) {
                    self.start_text_edit(li, ti, Some([cx, cy]));
                    self.text_gesture = Some(TextGesture::Select);
                    return;
                }
            }
        }
        // Empty canvas: maybe a box drag, decided on release.
        self.text_gesture = Some(TextGesture::Box {
            start: (cx, cy),
            cur: (cx, cy),
        });
        self.mark_dirty();
    }

    pub fn text_tool_move(&mut self, cx: f32, cy: f32) {
        match &mut self.text_gesture {
            Some(TextGesture::Box { cur, .. }) => {
                *cur = (cx, cy);
                self.mark_dirty();
            }
            Some(TextGesture::Select) => {
                let Some(item) = self.edited_item() else {
                    return;
                };
                let dpi = self.doc_dpi();
                let l = item.to_local([cx, cy]);
                if let Some(e) = self.text_engine.as_ref() {
                    if let Ok(pos) = e.hit_test_point(item, dpi, l) {
                        if let Some(ed) = self.text_edit.as_mut() {
                            ed.caret = pos;
                            ed.goal = None;
                        }
                    }
                }
                self.mark_dirty();
            }
            None => {}
        }
    }

    pub fn text_tool_up(&mut self, cx: f32, cy: f32) {
        match self.text_gesture.take() {
            Some(TextGesture::Box { start, .. }) => {
                let (w, h) = ((cx - start.0).abs(), (cy - start.1).abs());
                if w > 12.0 && h > 12.0 {
                    self.start_new_text(
                        [start.0.min(cx), start.1.min(cy)],
                        Some([w.max(ct::MIN_TEXT_EXTENT), h.max(ct::MIN_TEXT_EXTENT)]),
                    );
                } else {
                    self.start_new_text([start.0, start.1], None);
                }
            }
            Some(TextGesture::Select) | None => {}
        }
    }

    // --- keyboard ----------------------------------------------------------

    /// One UTF-16 unit from WM_CHAR while editing. Joins surrogate pairs,
    /// drops control characters (Enter arrives as VK_RETURN in `text_key`).
    pub fn text_char(&mut self, unit: u16) {
        let Some(ed) = self.text_edit.as_mut() else {
            return;
        };
        let scalar = match (ed.pending_surrogate.take(), unit) {
            (_, 0xD800..=0xDBFF) => {
                ed.pending_surrogate = Some(unit);
                return;
            }
            (Some(hi), 0xDC00..=0xDFFF) => {
                char::from_u32(0x1_0000 + (((hi as u32 - 0xD800) << 10) | (unit as u32 - 0xDC00)))
            }
            (_, u) => char::from_u32(u as u32),
        };
        let Some(c) = scalar else { return };
        if c.is_control() {
            return;
        }
        self.insert_at_caret(&c.to_string());
    }

    fn insert_at_caret(&mut self, s: &str) {
        let Some(ed) = self.text_edit.as_ref() else {
            return;
        };
        // A single character with a selection to replace, or whitespace, is
        // a boundary — everything else typed run-on coalesces into one undo
        // step. (Pasting is never "typing", however short the paste.)
        let typing = !ed.has_selection()
            && s.chars().count() == 1
            && !s.chars().any(|c| c.is_whitespace());
        self.snapshot_edit(typing);
        let Some(ed) = self.text_edit.as_ref() else {
            return;
        };
        let (a, b) = ed.selection();
        let add = ct::utf16_len(s);
        self.with_edited_item(|item| {
            if a != b {
                item.delete_range(a, b);
            }
            item.insert(a, s);
        });
        if let Some(ed) = self.text_edit.as_mut() {
            ed.caret = a + add;
            ed.anchor = ed.caret;
            ed.goal = None;
        }
    }

    fn delete_selection_or(&mut self, fallback: impl FnOnce(&TextItem, u32) -> (u32, u32)) {
        let Some(ed) = self.text_edit.as_ref() else {
            return;
        };
        let (a, b) = if ed.has_selection() {
            ed.selection()
        } else {
            let caret = ed.caret;
            match self.edited_item() {
                Some(item) => fallback(item, caret),
                None => return,
            }
        };
        // Backspace at the start of the box, Delete at the end: there is
        // nothing to remove, so there is nothing to remember either. The
        // snapshot goes AFTER this, or the next Ctrl+Z spends itself
        // restoring the text it is already looking at.
        if a == b {
            return;
        }
        self.snapshot_edit(false);
        self.with_edited_item(|item| item.delete_range(a, b));
        if let Some(ed) = self.text_edit.as_mut() {
            ed.caret = a.min(b);
            ed.anchor = ed.caret;
            ed.goal = None;
        }
    }

    fn move_caret(&mut self, pos: u32, shift: bool) {
        if let Some(ed) = self.text_edit.as_mut() {
            ed.caret = pos;
            if !shift {
                ed.anchor = pos;
            }
        }
        self.mark_dirty();
    }

    /// Caret motion along the reading axis: −1 back, +1 forward. Collapses an
    /// active selection to its edge on unshifted plain moves (standard).
    fn caret_step(&mut self, dir: i32, word: bool, shift: bool) {
        let Some(ed) = self.text_edit.as_ref() else {
            return;
        };
        let Some(item) = self.edited_item() else {
            return;
        };
        let (a, b) = ed.selection();
        let pos = if !shift && ed.has_selection() && !word {
            if dir < 0 { a } else { b }
        } else {
            let c = ed.caret;
            match (dir < 0, word) {
                (true, false) => ct::prev_boundary(&item.text, c),
                (false, false) => ct::next_boundary(&item.text, c),
                (true, true) => ct::prev_word_boundary(&item.text, c),
                (false, true) => ct::next_word_boundary(&item.text, c),
            }
        };
        self.move_caret(pos, shift);
        if let Some(ed) = self.text_edit.as_mut() {
            ed.goal = None;
        }
    }

    /// Caret motion across lines/columns: −1 = previous line, +1 = next.
    fn caret_line(&mut self, dir: i32, shift: bool) {
        let Some(ed) = self.text_edit.as_ref() else {
            return;
        };
        let goal = ed.goal;
        let caret = ed.caret;
        let dpi = self.doc_dpi();
        let Some(item) = self.edited_item() else {
            return;
        };
        let Some(e) = self.text_engine.as_ref() else {
            return;
        };
        if let Ok((pos, g)) = e.line_move(item, dpi, caret, dir, goal) {
            self.move_caret(pos, shift);
            if let Some(ed) = self.text_edit.as_mut() {
                ed.goal = Some(g);
            }
        }
    }

    /// Tool Property B/I/U buttons — same behaviour as Ctrl+B/I/U.
    pub fn text_style_button(&mut self, flag: StyleFlag) {
        self.toggle_style(flag);
    }

    /// Apply the Tool Property furigana field to the selected characters
    /// (TX-062). An empty field clears whatever reading they carry.
    pub fn text_ruby_button(&mut self) {
        let Some(ed) = self.text_edit.as_ref() else {
            self.set_status("double-click the text first — furigana applies to selected characters");
            return;
        };
        let (a, b) = ed.selection();
        if a == b {
            self.set_status("select the kanji first, then set its reading");
            return;
        }
        let reading = self.text_ruby.trim().to_owned();
        self.snapshot_edit(false);
        let mut changed = false;
        self.with_edited_item(|item| {
            changed = item.set_ruby(a, b, &reading);
        });
        if !changed {
            self.drop_last_snapshot();
        }
        self.set_status(match (changed, reading.is_empty()) {
            (false, _) => "no change".to_owned(),
            (true, true) => "furigana removed".to_owned(),
            (true, false) => format!("furigana: {reading}"),
        });
    }

    /// Set the selected characters in `family` (TX-064). Picking the item's
    /// own family clears the override rather than storing a redundant one.
    pub fn text_font_range_button(&mut self, family: String) {
        let Some(ed) = self.text_edit.as_ref() else {
            return;
        };
        let (a, b) = ed.selection();
        if a == b {
            return;
        }
        self.snapshot_edit(false);
        let mut changed = false;
        self.with_edited_item(|item| {
            changed = item.set_font_range(a, b, &family);
        });
        if !changed {
            self.drop_last_snapshot();
        }
        self.set_status(if changed {
            format!("selection set in {family}")
        } else {
            format!("selection already in {family}")
        });
    }

    /// Toggle 縦中横 over the selected characters (TX-063): digits and short
    /// Latin runs stand UPRIGHT inside a vertical column instead of lying on
    /// their side, which is how every number in vertical text is set.
    ///
    /// A toggle rather than two buttons, and it reads the selection to
    /// decide: if the whole selection is already upright, the press turns it
    /// off. That is what a user expects from a thing that looks like B/I/U.
    pub fn text_tcy_button(&mut self) {
        let Some(ed) = self.text_edit.as_ref() else {
            self.set_status("double-click the text first — 縦中横 applies to selected characters");
            return;
        };
        let (a, b) = ed.selection();
        if a == b {
            self.set_status("select the number first, then stand it upright");
            return;
        }
        let on = !self.selection_is_tcy();
        self.snapshot_edit(false);
        let mut changed = false;
        self.with_edited_item(|item| {
            changed = item.set_tcy(a, b, on);
        });
        if !changed {
            self.drop_last_snapshot();
        }
        self.set_status(if on {
            "縦中横: upright in the column"
        } else {
            "縦中横 removed"
        });
    }

    /// Is every selected character already inside a 縦中横 run? Drives the
    /// toggle's pressed state and its meaning.
    pub fn selection_is_tcy(&self) -> bool {
        let Some(ed) = self.text_edit.as_ref() else {
            return false;
        };
        let (a, b) = ed.selection();
        if a == b {
            return false;
        }
        let Some(item) = self.edited_item() else {
            return false;
        };
        (a..b).all(|p| item.tcy.iter().any(|t| p >= t.start && p < t.end()))
    }

    /// The reading under the caret — the Tool Property field shows it as a
    /// hint so an existing annotation is legible without hunting on canvas.
    pub fn ruby_at_caret(&self) -> Option<String> {
        let ed = self.text_edit.as_ref()?;
        let item = self.edited_item()?;
        item.ruby_at(ed.selection().0).map(|r| r.text.clone())
    }

    fn toggle_style(&mut self, flag: StyleFlag) {
        let Some(ed) = self.text_edit.as_ref() else {
            return;
        };
        let (a, b) = ed.selection();
        if a == b {
            self.set_status("select some text first (B/I/U applies to the selection)");
            return;
        }
        self.snapshot_edit(false);
        self.with_edited_item(|item| {
            let on = !item.range_has_all(a, b, flag);
            item.set_style(a, b, flag, on);
        });
    }

    fn selected_text(&self) -> Option<String> {
        let ed = self.text_edit.as_ref()?;
        let (a, b) = ed.selection();
        if a == b {
            return None;
        }
        let item = self.edited_item()?;
        let (ba, bb) = (
            ct::utf16_to_byte(&item.text, a),
            ct::utf16_to_byte(&item.text, b),
        );
        Some(item.text[ba..bb].to_string())
    }

    /// Every key the editor understands. Returns true when consumed (the
    /// caller must then skip app shortcuts AND egui). Arrow roles swap with
    /// orientation: in vertical text up/down walk the column and left/right
    /// change columns.
    pub fn text_key(&mut self, vk: u16, ctrl: bool, shift: bool) -> bool {
        let Some(ed) = self.text_edit.as_ref() else {
            return false;
        };
        let vertical = self.edited_item().map(|i| i.vertical).unwrap_or(false);
        let len = self.edited_item().map(|i| i.utf16_len()).unwrap_or(0);
        let caret = ed.caret;
        match vk {
            0x1B => {
                self.commit_text_edit(); // Esc
            }
            0x0D => self.insert_at_caret("\n"), // Enter
            0x08 => {
                // Backspace (+Ctrl = word)
                self.delete_selection_or(|item, c| {
                    let a = if ctrl {
                        ct::prev_word_boundary(&item.text, c)
                    } else {
                        ct::prev_boundary(&item.text, c)
                    };
                    (a, c)
                });
            }
            0x2E => {
                // Delete (+Ctrl = word)
                self.delete_selection_or(|item, c| {
                    let b = if ctrl {
                        ct::next_word_boundary(&item.text, c)
                    } else {
                        ct::next_boundary(&item.text, c)
                    };
                    (c, b)
                });
            }
            0x25 | 0x26 | 0x27 | 0x28 => {
                // Arrows: map to (reading axis, line axis) per orientation.
                let (back, fwd, prev_line, next_line) = if vertical {
                    (0x26, 0x28, 0x27, 0x25) // up, down; right = prev column
                } else {
                    (0x25, 0x27, 0x26, 0x28)
                };
                if vk == back {
                    self.caret_step(-1, ctrl, shift);
                } else if vk == fwd {
                    self.caret_step(1, ctrl, shift);
                } else if vk == prev_line {
                    self.caret_line(-1, shift);
                } else if vk == next_line {
                    self.caret_line(1, shift);
                }
            }
            0x24 => {
                // Home (Ctrl = start of text)
                let pos = if ctrl {
                    0
                } else {
                    let dpi = self.doc_dpi();
                    match (self.edited_item(), self.text_engine.as_ref()) {
                        (Some(item), Some(e)) => {
                            e.line_bounds(item, dpi, caret).map(|(s, _)| s).unwrap_or(0)
                        }
                        _ => 0,
                    }
                };
                self.move_caret(pos, shift);
            }
            0x23 => {
                // End (Ctrl = end of text)
                let pos = if ctrl {
                    len
                } else {
                    let dpi = self.doc_dpi();
                    match (self.edited_item(), self.text_engine.as_ref()) {
                        (Some(item), Some(e)) => e
                            .line_bounds(item, dpi, caret)
                            .map(|(_, e)| e)
                            .unwrap_or(len),
                        _ => len,
                    }
                };
                self.move_caret(pos, shift);
            }
            0x41 if ctrl => {
                // Ctrl+A
                if let Some(ed) = self.text_edit.as_mut() {
                    ed.anchor = 0;
                    ed.caret = len;
                }
                self.mark_dirty();
            }
            0x43 if ctrl => {
                // Ctrl+C
                if let Some(s) = self.selected_text() {
                    crate::win32::clipboard_set_text(&s);
                }
            }
            0x58 if ctrl => {
                // Ctrl+X
                if let Some(s) = self.selected_text() {
                    crate::win32::clipboard_set_text(&s);
                    self.delete_selection_or(|_, c| (c, c));
                }
            }
            0x56 if ctrl => {
                // Ctrl+V
                if let Some(s) = crate::win32::clipboard_get_text() {
                    let s = s.replace("\r\n", "\n").replace('\r', "\n");
                    if !s.is_empty() {
                        self.insert_at_caret(&s);
                    }
                }
            }
            0x42 if ctrl => self.toggle_style(StyleFlag::Bold), // Ctrl+B
            0x49 if ctrl => self.toggle_style(StyleFlag::Italic), // Ctrl+I
            0x55 if ctrl => self.toggle_style(StyleFlag::Underline), // Ctrl+U
            0x5A if ctrl => {
                // Ctrl+Z steps back through THIS session's edits first
                // (owner: "Ctrl+Z seems to remove an entire text rather than
                // edit something in it"). Only once there is nothing left to
                // undo inside the box does it end the session and undo the
                // box itself — which is the right last step, and used to be
                // the only one.
                //
                // ONLY if the session left a step to undo, though: a session
                // that ends where it started pushes nothing, and the queued
                // Undo would then pop whatever the document did BEFORE it —
                // the stroke you drew a minute ago, deleted from under a
                // blinking caret. Nothing to undo means nothing to undo.
                if !self.text_undo_step() && self.commit_text_edit() {
                    self.push_cmd(AppCmd::Undo);
                }
            }
            // Swallow anything else that would trigger a tool shortcut while
            // typing; real characters arrive via WM_CHAR.
            _ => return true,
        }
        true
    }

    // --- Object tool -------------------------------------------------------

    /// Object-tool press: text boxes sit above balloons, so try them first.
    /// Returns true when a text item claimed the press.
    pub fn text_object_hit(&mut self, cx: f32, cy: f32) -> bool {
        let tol = (10.0 / self.viewport.zoom.max(0.01)).max(2.0);
        let rot = ROTATE_OFFSET_SCREEN / self.viewport.zoom.max(0.01);
        for li in (0..self.doc.layers.len()).rev() {
            let l = &self.doc.layers[li];
            if !l.visible {
                continue;
            }
            let Some(ts) = l.texts() else { continue };
            for ti in (0..ts.texts.len()).rev() {
                let item = &ts.texts[ti];
                // Handles are only live on the already-selected item, so a
                // stray rotate lollipop can't steal clicks from other objects.
                let selected = self.text_sel == Some((li, ti));
                let mode = if selected {
                    item.handle_near([cx, cy], tol * 1.4, rot)
                        .map(TextDragMode::Handle)
                        .or_else(|| {
                            item.contains([cx, cy], 0.0)
                                .then_some(TextDragMode::MoveWhole)
                        })
                } else {
                    item.contains([cx, cy], 0.0)
                        .then_some(TextDragMode::MoveWhole)
                };
                if let Some(mode) = mode {
                    self.warm_texts(li);
                    let Some(orig) = self.doc.layers[li]
                        .texts()
                        .and_then(|ts| ts.texts.get(ti))
                        .cloned()
                    else {
                        return false;
                    };
                    self.text_sel = Some((li, ti));
                    self.object_pick = Some((cx, cy));
                    self.text_obj_drag = Some(TextObjDrag {
                        layer: li,
                        index: ti,
                        mode,
                        start: (cx, cy),
                        cur: (cx, cy),
                        orig,
                    });
                    self.balloon_sel = None;
                    self.object_sel = None;
                    return true;
                }
            }
        }
        self.text_sel = None;
        false
    }

    /// Object-tool release on a text drag: commit as one undo step.
    pub fn finish_text_obj_drag(&mut self, cx: f32, cy: f32) {
        let Some(mut d) = self.text_obj_drag.take() else {
            return;
        };
        d.cur = (cx, cy);
        if !d.moved() {
            return;
        }
        let mut item = d.preview();
        // A move keeps the sprite (translate shifted its origin); any reshape
        // or rotation needs a fresh shape.
        if !matches!(d.mode, TextDragMode::MoveWhole) {
            let dpi = self.doc_dpi();
            item.cache = self
                .text_engine
                .as_ref()
                .and_then(|e| e.render(&item, dpi).ok().flatten());
        }
        let Some(ts) = self.doc.layers.get(d.layer).and_then(|l| l.texts()) else {
            return;
        };
        let mut ts = ts.clone();
        if d.index < ts.texts.len() {
            ts.texts[d.index] = item;
            self.push_cmd(AppCmd::TextCommit {
                layer: d.layer,
                texts: ts,
            });
        }
        self.mark_dirty();
    }

    // --- overlay geometry --------------------------------------------------

    /// Caret + selection quads for the canvas overlay, canvas coords.
    pub fn text_caret_overlay(&self) -> Option<TextCaretOverlay> {
        let ed = self.text_edit.as_ref()?;
        let item = self.edited_item()?;
        let e = self.text_engine.as_ref()?;
        let dpi = self.doc_dpi();
        let c = e.caret(item, dpi, ed.caret).ok()?;
        // The caret crosses the character cell: vertical bar for horizontal
        // text, horizontal bar (across the column) for vertical text.
        let caret = if item.vertical {
            [
                item.to_canvas([c.cell[0], c.point[1]]),
                item.to_canvas([c.cell[0] + c.cell[2], c.point[1]]),
            ]
        } else {
            [
                item.to_canvas([c.point[0], c.cell[1]]),
                item.to_canvas([c.point[0], c.cell[1] + c.cell[3]]),
            ]
        };
        let (a, b) = ed.selection();
        let selection = if a == b {
            Vec::new()
        } else {
            e.selection_rects(item, dpi, a, b)
                .unwrap_or_default()
                .iter()
                .map(|r| {
                    [
                        item.to_canvas([r[0], r[1]]),
                        item.to_canvas([r[0] + r[2], r[1]]),
                        item.to_canvas([r[0] + r[2], r[1] + r[3]]),
                        item.to_canvas([r[0], r[1] + r[3]]),
                    ]
                })
                .collect()
        };
        Some(TextCaretOverlay { caret, selection })
    }

    /// Screen position (client px) for the IME composition window: just under
    /// the caret.
    pub fn ime_caret_client_px(&self) -> Option<(i32, i32)> {
        let ov = self.text_caret_overlay()?;
        let p = ov.caret[1];
        let (sx, sy) = self.viewport.to_screen(p[0], p[1]);
        Some((sx.round() as i32, sy.round() as i32 + 4))
    }

    /// Apply a Tool Property change (font/size/orientation/outline) to the
    /// item being edited, or to the Object-tool selection. Commits as one
    /// undo step per call — the shape discrete controls (buttons, pickers)
    /// want. The VALUE-BAR rows use `begin_text_bar_drag` + `preview_text_prop`
    /// + `commit_text_bar_drag` instead (one undo step per DRAG, not per
    /// frame; auditor round 34).
    pub fn apply_text_prop(&mut self, f: impl FnOnce(&mut TextItem)) {
        if self.text_edit.is_some() {
            // A property change is an edit like any other. Without its own
            // pre-image it is invisible to the in-editor stack, and the next
            // Ctrl+Z reaches straight past it to the typing underneath —
            // taking back the font AND the sentence in one press.
            self.snapshot_edit(false);
            self.with_edited_item(|item| f(item));
            return;
        }
        if let Some((li, ts)) = self.object_text_snapshot(f) {
            self.push_cmd(AppCmd::TextCommit {
                layer: li,
                texts: ts,
            });
        }
    }

    /// Object-selected item + a mutation → the mutated TextSet clone with
    /// its sprite re-rendered (the shared prologue of the apply/preview
    /// paths). Returns None when there is no Object text selection.
    fn object_text_snapshot(&mut self, f: impl FnOnce(&mut TextItem)) -> Option<(usize, TextSet)> {
        let (li, ti) = self.text_sel?;
        self.warm_texts(li);
        let dpi = self.doc_dpi();
        let ts = self.doc.layers.get(li).and_then(|l| l.texts())?.clone();
        let mut ts = ts;
        let item = ts.texts.get_mut(ti)?;
        f(item);
        if item.auto_size {
            if let Some(e) = self.text_engine.as_ref() {
                if let Ok(natural) = e.natural_size(item, dpi) {
                    if item.vertical {
                        item.pos[0] += item.size[0] - natural[0];
                    }
                    item.size = natural;
                }
            }
        }
        item.cache = self
            .text_engine
            .as_ref()
            .and_then(|e| e.render(item, dpi).ok().flatten());
        Some((li, ts))
    }

    /// Open the value-bar drag bracket: snapshot the selected item's TextSet
    /// so the whole drag lands as ONE undo step. Idempotent — an open
    /// bracket means the same drag is continuing (every frame calls this).
    /// If a release is ever lost (the panel closing mid-drag), the stale
    /// bracket makes the next interaction's commit span both — merged undo
    /// granularity, never data loss.
    ///
    /// A live session has no document bracket (that path is history-free
    /// until the session commits), but it still needs ONE in-editor
    /// pre-image for the drag — taken here, where the drag begins, and not
    /// in `preview_text_prop`, which runs per frame and would bury the
    /// session's real edits under a hundred slider positions.
    pub fn begin_text_bar_drag(&mut self) {
        if self.text_edit.is_some() {
            if !self.text_edit.as_ref().is_some_and(|ed| ed.bar_run) {
                self.snapshot_edit(false);
                if let Some(ed) = self.text_edit.as_mut() {
                    ed.bar_run = true;
                }
            }
            return;
        }
        if self.text_bar_drag.is_some() {
            return;
        }
        let Some((li, _)) = self.text_sel else {
            return;
        };
        self.warm_texts(li);
        let Some(before) = self.doc.layers.get(li).and_then(|l| l.texts()).cloned() else {
            return;
        };
        self.text_bar_drag = Some(TextBarDrag { layer: li, before });
    }

    /// A value-bar drag frame: apply LIVE with no history — the canvas
    /// preview follows the bar, `commit_text_bar_drag` lands the undo entry.
    pub fn preview_text_prop(&mut self, f: impl FnOnce(&mut TextItem)) {
        if self.text_edit.is_some() {
            self.with_edited_item(|item| f(item));
            return;
        }
        if let Some((li, ts)) = self.object_text_snapshot(f) {
            let size = self.doc.size;
            let raster = ts.rasterize(size);
            if let Some(layer) = self.doc.layers.get_mut(li) {
                layer.kind = LayerKind::Text(ts);
                layer.replace_tiles(raster);
            }
            self.doc.touch();
            self.mark_dirty();
        }
    }

    /// Close the value-bar drag bracket: quietly rewind to the pre-drag
    /// snapshot, then apply the dragged state as ONE undo step
    /// (`commit_text_edit`'s trick — set_texts snapshots the pre-image
    /// itself). No-op when no bracket is open.
    ///
    /// During a session there is no document bracket to close, only the
    /// in-editor run to end, so the next bar drag takes its own pre-image.
    pub fn commit_text_bar_drag(&mut self) {
        if let Some(ed) = self.text_edit.as_mut() {
            ed.bar_run = false;
        }
        let Some(d) = self.text_bar_drag.take() else {
            return;
        };
        let final_ts = {
            let Some(layer) = self.doc.layers.get_mut(d.layer) else {
                return;
            };
            let LayerKind::Text(ts) = &mut layer.kind else {
                return;
            };
            let final_ts = ts.clone();
            // Quiet rewind, no re-raster — set_texts rasterizes right after.
            *ts = d.before;
            final_ts
        };
        self.doc.set_texts(d.layer, final_ts);
        self.mark_dirty();
    }

    /// The sprite caches referenced by `RenderedText` are plain data — this
    /// exists so the module keeps compiling if nothing else names the type.
    #[allow(dead_code)]
    fn _rendered_text_marker(_: Arc<RenderedText>) {}
}

/// The item shown by Tool Property when the Text tool is up: the edited item,
/// else the Object selection, else None (panel edits the new-text defaults).
pub fn property_target(app: &App) -> Option<(usize, usize)> {
    if let Some(ed) = app.text_edit.as_ref() {
        Some((ed.layer, ed.index))
    } else if app.tool == Tool::Object {
        app.text_sel
    } else {
        None
    }
}

/// What Ctrl+Z does while the caret is in a text box. Every assertion here is
/// about the DOCUMENT or the item's text — never about a return value — because
/// all three bugs these pin were invisible from inside the function that caused
/// them and only showed up as a page that had lost something.
#[cfg(test)]
mod in_editor_undo_tests {
    use super::*;
    use crate::app::PointerKind;
    use mn_core::PenSample;

    /// `app.rs` keeps the same four lines in `new_document_tests::headless`,
    /// private to that module. A machine with no usable adapter — or no
    /// DirectWrite — skips instead of failing: without a text engine there is
    /// no session to undo inside.
    fn headless() -> Option<App> {
        let renderer = mn_gpu::Renderer::new_headless(mn_gpu::GpuConfig {
            force_fallback: std::env::var("MN_WARP").is_ok(),
            no_vsync: false,
        })
        .ok()?;
        let app = App::new(renderer, (1280, 860), 1.0);
        app.text_engine.is_some().then_some(app)
    }

    fn typed(app: &App) -> String {
        app.edited_item().map(|i| i.text.clone()).unwrap_or_default()
    }

    fn type_str(app: &mut App, s: &str) {
        for u in s.encode_utf16() {
            app.text_char(u);
        }
    }

    /// Total ink on the page — the only honest way to ask whether a stroke is
    /// still there after an undo.
    fn all_ink(app: &App) -> u64 {
        app.doc
            .layers
            .iter()
            .flat_map(|l| l.tiles())
            .map(|(_, t)| t.alpha_sum())
            .sum()
    }

    /// A brush stroke across the middle of the view. SCREEN coordinates:
    /// `push_batch` runs them through the viewport itself.
    fn scribble(app: &mut App) {
        app.begin_stroke(PointerKind::Mouse);
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

    /// `main::pump_commands`, in miniature: the Ctrl+Z fall-through QUEUES an
    /// `AppCmd::Undo`, so nothing is proved until the queue is drained.
    fn pump(app: &mut App) {
        while let Some(c) = app.cmds.pop_front() {
            crate::cmd::dispatch(app, c);
        }
    }

    fn text_layer(app: &App) -> usize {
        app.doc
            .layers
            .iter()
            .position(|l| l.is_text())
            .expect("the session left a text layer behind")
    }

    fn committed_text(app: &App) -> String {
        app.doc
            .layers
            .iter()
            .find(|l| l.is_text())
            .and_then(|l| l.texts())
            .and_then(|t| t.texts.first())
            .map(|i| i.text.clone())
            .unwrap_or_default()
    }

    /// DEFECT 1 (data loss). Ctrl+Z past the last in-editor step used to
    /// commit the session and queue a document `Undo` unconditionally. A
    /// session whose net effect is nothing pushes NO document step — so the
    /// queued undo popped whatever was underneath it, and the stroke you drew
    /// before you touched the box vanished while the caret was still blinking
    /// in it.
    #[test]
    fn ctrl_z_out_of_a_text_box_never_undoes_the_stroke_underneath_it() {
        let Some(mut app) = headless() else {
            println!("[test] SKIP: no usable adapter");
            return;
        };
        // A box that is already part of the document...
        app.start_new_text([300.0, 300.0], None);
        type_str(&mut app, "hi");
        app.commit_text_edit();
        let li = text_layer(&app);

        // ...and a stroke drawn after it, on the paint layer: the work.
        app.doc.active = 0;
        scribble(&mut app);
        let kept = all_ink(&app);
        let steps = app.doc.undo_len();
        assert!(steps >= 2, "the box and the stroke are both on the stack");

        // Double-click the EXISTING box, type one character, undo it: the
        // session is still open and its in-editor stack is now empty.
        app.start_text_edit(li, 0, None);
        type_str(&mut app, "x");
        assert_eq!(typed(&app), "hix");
        assert!(app.text_key(0x5A, true, false));
        assert_eq!(typed(&app), "hi", "the in-editor step went back");

        // The press that used to eat the stroke.
        assert!(app.text_key(0x5A, true, false));
        pump(&mut app);

        assert_eq!(all_ink(&app), kept, "the stroke is still on the page");
        assert_eq!(
            app.doc.undo_len(),
            steps,
            "nothing was popped off the document stack"
        );
        assert_eq!(committed_text(&app), "hi", "and the box still reads what it did");
    }

    /// DEFECT 2 (discrete controls). A Tool Property change during a session
    /// took a fast path with no `snapshot_edit`, so it was invisible to the
    /// in-editor stack: one Ctrl+Z reverted the property AND emptied the box,
    /// because the only entry on the stack was the typing.
    #[test]
    fn a_tool_property_change_mid_session_undoes_on_its_own() {
        let Some(mut app) = headless() else {
            println!("[test] SKIP: no usable adapter");
            return;
        };
        app.start_new_text([300.0, 300.0], None);
        type_str(&mut app, "abc");
        let before_pt = app.edited_item().expect("the edited item").size_pt;

        app.apply_text_prop(|i| i.size_pt = 33.0);
        assert_eq!(app.edited_item().unwrap().size_pt, 33.0, "the size applied");

        assert!(app.text_key(0x5A, true, false));
        assert_eq!(typed(&app), "abc", "the press took back the size, not the text");
        assert_eq!(app.edited_item().unwrap().size_pt, before_pt);

        assert!(app.text_key(0x5A, true, false));
        assert_eq!(typed(&app), "", "the typing is the step behind it");
    }

    /// DEFECT 2 (value bars). The furigana/size bars call
    /// `begin_text_bar_drag` + `preview_text_prop` every changed frame. One
    /// snapshot for the whole drag, taken when the bracket opens — not one per
    /// frame, and not none at all.
    #[test]
    fn a_value_bar_drag_mid_session_is_one_in_editor_step() {
        let Some(mut app) = headless() else {
            println!("[test] SKIP: no usable adapter");
            return;
        };
        app.start_new_text([300.0, 300.0], None);
        type_str(&mut app, "abc");
        let before_pct = app.edited_item().expect("the edited item").ruby_style.size_pct;
        let steps = app.doc.undo_len();

        for k in 0..10u16 {
            let pct = 40.0 + k as f32;
            app.begin_text_bar_drag();
            app.preview_text_prop(move |i| i.ruby_style.size_pct = pct);
        }
        app.commit_text_bar_drag();
        assert_eq!(
            app.edited_item().unwrap().ruby_style.size_pct,
            49.0,
            "the live preview tracked the drag"
        );
        assert_eq!(
            app.doc.undo_len(),
            steps,
            "a bar drag inside a session never touches document history"
        );

        assert!(app.text_key(0x5A, true, false));
        assert_eq!(typed(&app), "abc", "one press took back the whole drag, not the text");
        assert_eq!(app.edited_item().unwrap().ruby_style.size_pct, before_pct);

        assert!(app.text_key(0x5A, true, false));
        assert_eq!(typed(&app), "", "and the typing is one step behind");
    }

    /// DEFECT 3. The pre-image was taken before anyone knew the edit would
    /// change something, so a Backspace with nothing behind the caret still
    /// pushed an entry — and the next Ctrl+Z spent itself restoring what was
    /// already on screen.
    #[test]
    fn a_backspace_at_the_start_does_not_cost_a_ctrl_z() {
        let Some(mut app) = headless() else {
            println!("[test] SKIP: no usable adapter");
            return;
        };
        app.start_new_text([300.0, 300.0], None);
        type_str(&mut app, "ab");
        app.text_key(0x24, true, false); // Ctrl+Home: caret to the start
        assert!(app.text_key(0x08, false, false)); // Backspace, nothing behind it
        assert_eq!(typed(&app), "ab", "the key changed nothing, as expected");

        assert!(app.text_key(0x5A, true, false));
        assert_eq!(typed(&app), "", "the very first press undid the typing");
    }

    /// DEFECT 3, the other half: `set_ruby` reports "nothing changed" and the
    /// entry it was given has to go back.
    #[test]
    fn a_furigana_press_that_changes_nothing_does_not_cost_a_ctrl_z() {
        let Some(mut app) = headless() else {
            println!("[test] SKIP: no usable adapter");
            return;
        };
        app.start_new_text([300.0, 300.0], None);
        type_str(&mut app, "ab");
        app.text_key(0x41, true, false); // Ctrl+A
        app.text_ruby.clear();
        app.text_ruby_button(); // no reading, and none to clear
        assert_eq!(typed(&app), "ab");

        assert!(app.text_key(0x5A, true, false));
        assert_eq!(typed(&app), "", "the very first press undid the typing");
    }
}
