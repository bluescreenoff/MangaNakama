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
    /// The caret trails the character before it — set at a SOFT wrap, where
    /// one UTF-16 position is both the end of the line that wrapped and the
    /// start of the next. End, a click past the last glyph, and an up/down
    /// landing that clamped past a shorter line's end all mean "the end of
    /// the line I can see"; everything else clears it. See
    /// `TextEngine::caret`.
    pub affinity: bool,
    /// This session created the layer itself (an empty commit removes it).
    pub new_layer: bool,
    /// UTF-16 high surrogate waiting for its pair (WM_CHAR arrives in halves).
    pub pending_surrogate: Option<u16>,
    /// IN-EDITOR undo: the item as it stood BEFORE each mutating burst, with
    /// the caret to put back. Oldest first; capped, because a session is not
    /// a place to keep an unbounded history.
    pub undo: Vec<(TextItem, u32)>,
    /// The other half of it: what Ctrl+Z took back, waiting for Ctrl+Shift+Z
    /// (or Ctrl+Y). Any fresh edit throws it away, as everywhere else.
    pub redo: Vec<(TextItem, u32)>,
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

/// Whether `want`ed trailing affinity is worth keeping at `pos`.
///
/// It only means anything at a SOFT wrap. At a hard break the position is
/// unambiguous, and asking DirectWrite for the trailing edge of the newline
/// draws the caret at the end of the line ABOVE the one it belongs to — the
/// same bug affinity exists to fix, pointed the other way. So: no newline
/// either side of the caret.
fn wrap_affinity(text: &str, pos: u32, want: bool) -> bool {
    if !want || pos == 0 {
        return false;
    }
    let unit = |at: u32| text.encode_utf16().nth(at as usize);
    unit(pos - 1) != Some(b'\n' as u16) && unit(pos) != Some(b'\n' as u16)
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
    /// `clicks` is the click-run this drag started from (1 = plain, 2 =
    /// double, 3+ = triple), and `base` what that press selected — a
    /// double-click drag extends by whole words, a triple-click drag by whole
    /// lines, and neither collapses when the mouse twitches.
    Select {
        clicks: u8,
        base: (u32, u32),
    },
    Box {
        start: (f32, f32),
        cur: (f32, f32),
    },
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
        // Restore the active-page invariant (bytes live in `doc`).
        self.pages[self.page_index].bytes = None;
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
        // A fresh edit is a new future: whatever Ctrl+Z had set aside for
        // Ctrl+Shift+Z is unreachable now.
        ed.redo.clear();
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
        self.text_history_step(false)
    }

    /// The other direction (Ctrl+Shift+Z / Ctrl+Y). False when there is
    /// nothing to redo — and unlike undo there is no fall-through, because
    /// committing the session would have cleared the document's redo stack
    /// anyway: there is nothing behind this one.
    pub fn text_redo_step(&mut self) -> bool {
        self.text_history_step(true)
    }

    /// One step in either direction: pop the far stack, push what is on
    /// screen onto the near one.
    fn text_history_step(&mut self, redo: bool) -> bool {
        let Some(cur) = self.edited_item().cloned() else {
            return false;
        };
        let Some(ed) = self.text_edit.as_mut() else {
            return false;
        };
        let from = if redo { &mut ed.redo } else { &mut ed.undo };
        let Some((item, caret)) = from.pop() else {
            return false;
        };
        let here = ed.caret;
        if redo {
            ed.undo.push((cur, here));
        } else {
            ed.redo.push((cur, here));
        }
        ed.typing_run = false;
        ed.caret = caret;
        ed.anchor = caret;
        ed.goal = None;
        ed.affinity = false;
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
        let (caret, trailing) = match (at, self.text_engine.as_ref()) {
            (Some(p), Some(e)) => {
                let l = item.to_local(p);
                e.hit_test_point(item, dpi, l).unwrap_or((0, false))
            }
            _ => (item.utf16_len(), false),
        };
        let affinity = wrap_affinity(&item.text, caret, trailing);
        self.text_edit = Some(TextEditState {
            layer,
            index,
            before,
            caret,
            anchor: caret,
            goal: None,
            affinity,
            new_layer: false,
            pending_surrogate: None,
            undo: Vec::new(),
            redo: Vec::new(),
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
            affinity: false,
            new_layer,
            pending_surrogate: None,
            undo: Vec::new(),
            redo: Vec::new(),
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

    /// `clicks` is the click-run count from `App::click_run` — 2 selects the
    /// word under the press, 3 the visual line (the Tool Property panel has
    /// been telling people to "double-click the text to set furigana" since
    /// the furigana field existed, and until now nothing happened).
    pub fn text_tool_down(&mut self, cx: f32, cy: f32, shift: bool, clicks: u8) {
        // Click inside the edited box: move the caret (Shift extends). A
        // NEAR-miss (a few px outside, plans/05 item 5) still aims the
        // caret — clamped to the box edge, so a click 3 px outside lands
        // at the nearest end of the line instead of dropping the session.
        if let Some(item) = self.edited_item() {
            if item.contains([cx, cy], 8.0) {
                let dpi = self.doc_dpi();
                let l = item.to_local([cx, cy]);
                let l = [
                    l[0].clamp(0.0, item.size[0].max(0.0)),
                    l[1].clamp(0.0, item.size[1].max(0.0)),
                ];
                // Everything the engine has to answer while `item` is still
                // borrowed, in one go.
                let hit = self.text_engine.as_ref().and_then(|e| {
                    let (pos, trailing) = e.hit_test_point(item, dpi, l).ok()?;
                    // A trailing hit means the cursor was on the RIGHT half of
                    // the character before `pos` — that character is the one
                    // the user pointed at, and the one a word or line select
                    // has to be asked about. (At the end of a wrapped line the
                    // difference is a whole line.)
                    let ix = if trailing { pos.saturating_sub(1) } else { pos };
                    let run = match clicks {
                        2 => Some(ct::word_range(&item.text, ix)),
                        // Visual line, so a wrapped line selects what the eye
                        // sees — and a vertical column selects the column.
                        n if n >= 3 => e.line_bounds(item, dpi, ix).ok(),
                        _ => None,
                    };
                    Some((pos, wrap_affinity(&item.text, pos, trailing), run))
                });
                let mut base = (0, 0);
                if let Some((pos, affinity, run)) = hit {
                    let ed = self.text_edit.as_mut().unwrap();
                    match run {
                        Some((a, b)) => {
                            ed.anchor = a;
                            ed.caret = b;
                            ed.affinity = false;
                            base = (a, b);
                        }
                        None => {
                            ed.caret = pos;
                            if !shift {
                                ed.anchor = pos;
                            }
                            ed.affinity = affinity;
                            base = (ed.anchor, pos);
                        }
                    }
                    ed.goal = None;
                }
                self.text_gesture = Some(TextGesture::Select { clicks, base });
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
                    let c = self.text_edit.as_ref().map(|ed| ed.caret).unwrap_or(0);
                    self.text_gesture = Some(TextGesture::Select {
                        clicks: 1,
                        base: (c, c),
                    });
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
            Some(TextGesture::Select { clicks, base }) => {
                let (clicks, base) = (*clicks, *base);
                let Some(item) = self.edited_item() else {
                    return;
                };
                let dpi = self.doc_dpi();
                let l = item.to_local([cx, cy]);
                // Same near-miss clamp as the press: a drag past the box
                // edge keeps selecting at the edge (every editor does).
                let l = [
                    l[0].clamp(0.0, item.size[0].max(0.0)),
                    l[1].clamp(0.0, item.size[1].max(0.0)),
                ];
                let hit = self.text_engine.as_ref().and_then(|e| {
                    let (pos, trailing) = e.hit_test_point(item, dpi, l).ok()?;
                    // A drag that began as a double/triple click keeps
                    // selecting in that unit, the way it does everywhere else.
                    // Same trailing-half rule as the press.
                    let ix = if trailing { pos.saturating_sub(1) } else { pos };
                    let run = match clicks {
                        2 => Some(ct::word_range(&item.text, ix)),
                        n if n >= 3 => e.line_bounds(item, dpi, ix).ok(),
                        _ => None,
                    };
                    Some((pos, wrap_affinity(&item.text, pos, trailing), run))
                });
                if let Some((pos, affinity, run)) = hit {
                    if let Some(ed) = self.text_edit.as_mut() {
                        match run {
                            Some((a, b)) => {
                                // The caret leads the drag; the anchor sits at
                                // the far edge of everything covered so far.
                                let (lo, hi) = (base.0.min(a), base.1.max(b));
                                let forward = pos >= base.1;
                                ed.anchor = if forward { lo } else { hi };
                                ed.caret = if forward { hi } else { lo };
                                ed.affinity = false;
                            }
                            None => {
                                ed.caret = pos;
                                ed.affinity = affinity;
                            }
                        }
                        ed.goal = None;
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
            Some(TextGesture::Select { .. }) | None => {}
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
        let typing =
            !ed.has_selection() && s.chars().count() == 1 && !s.chars().any(|c| c.is_whitespace());
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
            ed.affinity = false;
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
            ed.affinity = false;
        }
    }

    /// Put the caret at `pos` and forget where it came from. Home/End/Ctrl+A
    /// used to leave the old column goal standing, so the next Up/Down jumped
    /// the caret back to the column it had been in three keys ago; the same
    /// went for affinity. Motions that DO mean something by them —
    /// `caret_line`, End — set them again after calling this.
    fn move_caret(&mut self, pos: u32, shift: bool) {
        if let Some(ed) = self.text_edit.as_mut() {
            ed.caret = pos;
            if !shift {
                ed.anchor = pos;
            }
            ed.goal = None;
            ed.affinity = false;
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
    }

    /// Caret motion across lines/columns: −1 = previous line, +1 = next.
    fn caret_line(&mut self, dir: i32, shift: bool) {
        let Some(ed) = self.text_edit.as_ref() else {
            return;
        };
        let goal = ed.goal;
        let caret = ed.caret;
        let affinity = ed.affinity;
        let dpi = self.doc_dpi();
        let Some(item) = self.edited_item() else {
            return;
        };
        let Some(e) = self.text_engine.as_ref() else {
            return;
        };
        let Ok((pos, g, trailing)) = e.line_move(item, dpi, caret, affinity, dir, goal) else {
            return;
        };
        // Up on the FIRST line goes to the start of the text, Down on the last
        // to its end — every editor does this, and doing nothing instead reads
        // as a dead key.
        if pos == caret {
            let end = if dir < 0 { 0 } else { item.utf16_len() };
            self.move_caret(end, shift);
            return;
        }
        let land = wrap_affinity(&item.text, pos, trailing);
        self.move_caret(pos, shift);
        if let Some(ed) = self.text_edit.as_mut() {
            ed.goal = Some(g);
            ed.affinity = land;
        }
    }

    /// PageUp/PageDown: to the first/last VISUAL line, keeping the goal
    /// column (the plan's "loop line_move to the boundary"). Unlike
    /// [`Self::caret_line`] there is NO jump past the boundary — the first
    /// line's column IS the destination; PageUp on the first line does
    /// nothing.
    fn caret_page(&mut self, dir: i32, shift: bool) {
        let Some(ed) = self.text_edit.as_ref() else {
            return;
        };
        let mut goal = ed.goal;
        let mut caret = ed.caret;
        let affinity = ed.affinity;
        let dpi = self.doc_dpi();
        let Some(item) = self.edited_item() else {
            return;
        };
        let Some(e) = self.text_engine.as_ref() else {
            return;
        };
        let mut moved = false;
        let mut land = false;
        // The cap is pathological-layout insurance; a real box stops in a
        // handful of hops.
        for _ in 0..4096 {
            let Ok((pos, g, trailing)) = e.line_move(item, dpi, caret, affinity, dir, goal) else {
                break;
            };
            if pos == caret {
                break;
            }
            moved = true;
            goal = Some(g);
            land = wrap_affinity(&item.text, pos, trailing);
            caret = pos;
        }
        if moved {
            self.move_caret(caret, shift);
            if let Some(ed) = self.text_edit.as_mut() {
                ed.goal = goal;
                ed.affinity = land;
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
            self.set_status(
                "double-click the text first — furigana applies to selected characters",
            );
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
                // End means the end of the line you are LOOKING at. Where the
                // line wrapped, that position is also the start of the next
                // one, and without the affinity the caret drew itself down
                // there (owner-visible: press End, caret jumps a line).
                let land = self
                    .edited_item()
                    .map(|i| !ctrl && wrap_affinity(&i.text, pos, true))
                    .unwrap_or(false);
                self.move_caret(pos, shift);
                if let Some(ed) = self.text_edit.as_mut() {
                    ed.affinity = land;
                }
            }
            0x41 if ctrl => {
                // Ctrl+A
                if let Some(ed) = self.text_edit.as_mut() {
                    ed.anchor = 0;
                    ed.caret = len;
                    ed.goal = None;
                    ed.affinity = false;
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
            0x5A if ctrl && shift => {
                // Ctrl+Shift+Z REDOES. It used to fall into the arm below and
                // undo — the one thing it must never do. Nothing to redo is a
                // no-op, not a fall-through: committing the session to reach
                // the document's redo stack would have cleared that stack.
                self.text_redo_step();
            }
            0x59 if ctrl => {
                // Ctrl+Y, the same thing under the other habit. It used to be
                // swallowed silently.
                self.text_redo_step();
            }
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
            0x21 | 0x22 => {
                // PageUp/PageDown (plans/05 item 5): caret to the FIRST/LAST
                // visual line, keeping the cross-axis position — loop the
                // same line_move the line arrows use until it stops moving.
                // Balloon-sized boxes: viewport paging is not a thing here.
                let dir = if vk == 0x21 { -1 } else { 1 };
                self.caret_page(dir, shift);
            }
            // Swallow anything else that would trigger a tool shortcut while
            // typing; real characters arrive via WM_CHAR.
            _ => return true,
        }
        true
    }

    // --- Object tool -------------------------------------------------------

    /// Object-tool press with the click-run count: a DOUBLE-click on a text
    /// box opens it for editing (CSP, and the only way most people ever get
    /// into a text box), a single one selects it for dragging. Returns true
    /// when a text item claimed the press.
    pub fn text_object_press(&mut self, cx: f32, cy: f32, clicks: u8) -> bool {
        if !self.text_object_hit(cx, cy) {
            return false;
        }
        if clicks >= 2
            && let Some((li, ti)) = self.text_sel
        {
            // The press had already armed a move-drag; editing takes it back.
            self.text_obj_drag = None;
            self.object_pick = None;
            self.start_text_edit(li, ti, Some([cx, cy]));
            // Land in the Text tool, so the next click is a caret and not a
            // drag on the box you are typing in.
            self.push_cmd(AppCmd::SetTool(Tool::Text));
        }
        true
    }

    /// Object-tool press: text boxes sit above balloons, so try them first.
    /// Returns true when a text item claimed the press.
    pub fn text_object_hit(&mut self, cx: f32, cy: f32) -> bool {
        let tol = (10.0 / self.viewport.zoom.max(0.01)).max(2.0);
        let rot = crate::app::ROTATE_STALK_SCREEN / self.viewport.zoom.max(0.01);
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
        let c = e.caret(item, dpi, ed.caret, ed.affinity).ok()?;
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
        app.edited_item()
            .map(|i| i.text.clone())
            .unwrap_or_default()
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

    /// plans/05 item 5: PageUp/PageDown go to the first/last VISUAL line
    /// keeping the column; on the boundary line they do nothing (no jump
    /// past it, unlike the line arrows' end-of-text hop).
    #[test]
    fn page_keys_reach_the_first_and_last_line() {
        let Some(mut app) = headless() else {
            println!("[test] SKIP: no usable adapter");
            return;
        };
        // Pin orientation + size — App::new reads machine prefs (the
        // recorded gotcha); a test must not move with the owner's prefs.
        app.start_new_text([300.0, 300.0], None);
        app.apply_text_prop(|i| {
            i.vertical = false;
            i.size_pt = 24.0;
        });
        type_str(&mut app, "aaa");
        assert!(app.text_key(0x0D, false, false), "Enter breaks the line");
        type_str(&mut app, "bbb");
        assert!(app.text_key(0x0D, false, false));
        type_str(&mut app, "ccc");
        let line_start = |app: &App| -> u32 {
            let ed = app.text_edit.as_ref().unwrap();
            let item = app.edited_item().unwrap();
            app.text_engine
                .as_ref()
                .unwrap()
                .line_bounds(item, app.doc_dpi(), ed.caret)
                .map(|(s, _)| s)
                .unwrap_or(u32::MAX)
        };
        // Caret starts at the very end (line 3).
        assert!(app.text_key(0x21, false, false), "PageUp handled");
        assert_eq!(line_start(&app), 0, "reached the first line");
        let first = app.text_edit.as_ref().unwrap().caret;
        // Already there: pressing it again does nothing (no end-of-text hop).
        assert!(app.text_key(0x21, false, false));
        assert_eq!(app.text_edit.as_ref().unwrap().caret, first);
        assert!(app.text_key(0x22, false, false), "PageDown handled");
        assert_eq!(line_start(&app), 8, "reached the last line");
    }

    /// plans/05 item 5: a click a few px OUTSIDE the box still aims the
    /// caret, clamped to the box edge — it must not drop the edit session.
    #[test]
    fn a_near_miss_click_lands_the_caret_at_the_edge() {
        let Some(mut app) = headless() else {
            println!("[test] SKIP: no usable adapter");
            return;
        };
        app.start_new_text([300.0, 300.0], None);
        app.apply_text_prop(|i| {
            i.vertical = false;
            i.size_pt = 24.0;
        });
        type_str(&mut app, "hello");
        let (x, y) = {
            let it = app.edited_item().unwrap();
            (it.pos[0] - 4.0, it.pos[1] + it.size[1] * 0.5)
        };
        app.text_tool_down(x, y, false, 1);
        assert!(
            app.text_edit.is_some(),
            "the session survives the near-miss"
        );
        assert_eq!(
            app.text_edit.as_ref().unwrap().caret,
            0,
            "clamped to the line start, not a miss"
        );
    }

    /// plans/05 item 5 (IME v1): the IME windows ride the caret — the
    /// positioning INPUT (`ime_caret_client_px`) must track caret movement
    /// during a session, or the composition/candidate windows strand where
    /// the session began.
    #[test]
    fn the_ime_anchor_follows_the_caret() {
        let Some(mut app) = headless() else {
            println!("[test] SKIP: no usable adapter");
            return;
        };
        app.start_new_text([300.0, 300.0], None);
        app.apply_text_prop(|i| {
            i.vertical = false;
            i.size_pt = 24.0;
        });
        assert!(
            app.ime_caret_client_px().is_some(),
            "an open session always has an anchor"
        );
        type_str(&mut app, "hello");
        let after_typing = app.ime_caret_client_px().unwrap();
        // Back to the start: a different anchor (left of the typing one —
        // exact layout varies by font, direction is the contract).
        assert!(app.text_key(0x24, true, false), "Ctrl+Home handled");
        let at_start = app.ime_caret_client_px().unwrap();
        assert!(
            at_start.0 < after_typing.0,
            "the anchor followed the caret to the start ({at_start:?} vs {after_typing:?})"
        );
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
        assert_eq!(
            committed_text(&app),
            "hi",
            "and the box still reads what it did"
        );
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
        assert_eq!(
            typed(&app),
            "abc",
            "the press took back the size, not the text"
        );
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
        let before_pct = app
            .edited_item()
            .expect("the edited item")
            .ruby_style
            .size_pct;
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
        assert_eq!(
            typed(&app),
            "abc",
            "one press took back the whole drag, not the text"
        );
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

    // --- caret round (2026-08-23): selection, affinity, redo ---------------

    /// Canvas point in the middle of the character at UTF-16 index `at` — a
    /// click aimed at a letter rather than at a coordinate somebody guessed.
    /// The CELL centre, so it reads the same in a vertical column.
    fn point_at(app: &App, at: u32) -> (f32, f32) {
        let item = app.edited_item().expect("an edited item");
        let e = app.text_engine.as_ref().expect("the engine");
        let dpi = app.doc_dpi();
        let a = e.caret(item, dpi, at, false).unwrap();
        let b = e.caret(item, dpi, at + 1, false).unwrap();
        // Halfway between this caret and the next one is the middle of the
        // glyph, along whichever axis the text reads on.
        let l = if item.vertical {
            [a.cell[0] + a.cell[2] * 0.5, (a.point[1] + b.point[1]) * 0.5]
        } else {
            [(a.point[0] + b.point[0]) * 0.5, a.cell[1] + a.cell[3] * 0.5]
        };
        let p = item.to_canvas(l);
        (p[0], p[1])
    }

    /// `text_char` drops control characters (Enter is a VK, not a WM_CHAR),
    /// so a test string with line breaks in it has to press the key.
    fn type_lines(app: &mut App, s: &str) {
        for (i, line) in s.split('\n').enumerate() {
            if i > 0 {
                app.text_key(0x0D, false, false);
            }
            type_str(app, line);
        }
    }

    fn sel(app: &App) -> (u32, u32) {
        app.text_edit.as_ref().expect("a session").selection()
    }

    /// The app boots into VERTICAL text at whatever size the prefs file last
    /// held — it is a manga app, and `text_size_pt` comes off disk. A test
    /// about lines and columns pins both or it reads the machine it ran on.
    fn horizontal(app: &mut App) {
        app.text_vertical = false;
        app.text_size_pt = 24.0;
    }

    /// A narrow fixed box, so the text WRAPS and every soft-wrap question has
    /// something to be asked about.
    fn wrapped(app: &mut App, text: &str) -> (u32, u32) {
        horizontal(app);
        app.start_new_text([200.0, 200.0], Some([120.0, 300.0]));
        type_str(app, text);
        let item = app.edited_item().expect("an edited item");
        let bounds = app
            .text_engine
            .as_ref()
            .unwrap()
            .line_bounds(item, app.doc_dpi(), 0)
            .unwrap();
        assert!(
            bounds.1 > 0 && bounds.1 < item.utf16_len(),
            "the string wrapped: {bounds:?} of {}",
            item.utf16_len()
        );
        bounds
    }

    /// The Tool Property panel has been saying "double-click the text" since
    /// furigana existed; the press did nothing, because nothing counted
    /// clicks. Two selects the word under the cursor, three the visual line.
    #[test]
    fn double_click_selects_a_word_and_triple_click_the_visual_line() {
        let Some(mut app) = headless() else {
            println!("[test] SKIP: no usable adapter");
            return;
        };
        horizontal(&mut app);
        app.start_new_text([300.0, 300.0], None);
        type_str(&mut app, "hello world");
        let (x, y) = point_at(&app, 8); // inside "world"
        app.text_tool_down(x, y, false, 2);
        assert_eq!(sel(&app), (6, 11), "the word, and not the space before it");

        // Dragging on from a double-click keeps taking WHOLE words — and a
        // twitch of the mouse no longer collapses the selection to a caret.
        let (bx, by) = point_at(&app, 1); // back into "hello"
        app.text_tool_move(bx, by);
        assert_eq!(sel(&app), (0, 11), "both words, whole");
        app.text_tool_up(bx, by);

        // Japanese: a script change is a word edge, so 漢字 / かな / ABC each
        // come out on their own.
        horizontal(&mut app);
        app.start_new_text([300.0, 500.0], None);
        type_str(&mut app, "漢字かなABC");
        let (x, y) = point_at(&app, 0);
        app.text_tool_down(x, y, false, 2);
        assert_eq!(sel(&app), (0, 2), "漢字");
        let (x, y) = point_at(&app, 3);
        app.text_tool_down(x, y, false, 2);
        assert_eq!(sel(&app), (2, 4), "かな");
        let (x, y) = point_at(&app, 5);
        app.text_tool_down(x, y, false, 2);
        assert_eq!(sel(&app), (4, 7), "ABC");

        // Three clicks take the VISUAL line — the wrapped part, not the whole
        // string typed without a newline in it.
        let (a, b) = wrapped(&mut app, "aaaa bbbb cccc dddd");
        let (x, y) = point_at(&app, 1);
        app.text_tool_down(x, y, false, 3);
        assert_eq!(sel(&app), (a, b), "the first visual line");
        assert!(
            b < app.edited_item().unwrap().utf16_len(),
            "and there is more text under it"
        );
    }

    /// Vertical text lays lines out as columns; the same two presses have to
    /// mean the same two things.
    #[test]
    fn word_and_line_select_work_in_a_vertical_column() {
        let Some(mut app) = headless() else {
            println!("[test] SKIP: no usable adapter");
            return;
        };
        app.text_vertical = true;
        app.start_new_text([400.0, 200.0], Some([300.0, 120.0]));
        type_str(&mut app, "漢字かなカナあいうえお");
        let (x, y) = point_at(&app, 0);
        app.text_tool_down(x, y, false, 2);
        assert_eq!(sel(&app), (0, 2), "漢字, read down the column");

        let item = app.edited_item().expect("an edited item");
        let (a, b) = app
            .text_engine
            .as_ref()
            .unwrap()
            .line_bounds(item, app.doc_dpi(), 0)
            .unwrap();
        assert!(b < item.utf16_len(), "the column wrapped: {b}");
        let (x, y) = point_at(&app, 0);
        app.text_tool_down(x, y, false, 3);
        assert_eq!(sel(&app), (a, b), "the whole first column");
    }

    /// P0. End on a wrapped line put the caret at the START OF THE NEXT LINE
    /// — the same UTF-16 position, the wrong place on the page. Asserted on
    /// the overlay geometry, because that is what the owner sees.
    #[test]
    fn end_on_a_wrapped_line_leaves_the_caret_on_that_line() {
        let Some(mut app) = headless() else {
            println!("[test] SKIP: no usable adapter");
            return;
        };
        let (_, end) = wrapped(&mut app, "aaaa bbbb cccc dddd");
        app.text_key(0x24, true, false); // Ctrl+Home
        let home = app.text_caret_overlay().expect("a caret").caret[0];

        app.text_key(0x23, false, false); // End
        assert_eq!(
            app.text_edit.as_ref().unwrap().caret,
            end,
            "End went to the end of the VISUAL line"
        );
        let at_end = app.text_caret_overlay().expect("a caret").caret[0];
        assert!(
            (at_end[1] - home[1]).abs() < 1.0,
            "the caret is drawn on line 1 ({} vs {})",
            at_end[1],
            home[1]
        );
        assert!(
            at_end[0] > home[0] + 10.0,
            "at the end of it ({} vs {})",
            at_end[0],
            home[0]
        );

        // Any other motion drops the affinity again: Left from there is an
        // ordinary caret one character back.
        app.text_key(0x25, false, false);
        assert!(
            !app.text_edit.as_ref().unwrap().affinity,
            "a plain step clears the trailing bit"
        );
    }

    /// P1. Home/End/Ctrl+A left the old column goal standing, so the next
    /// Up/Down snapped the caret back to a column it had left two keys ago.
    #[test]
    fn home_clears_the_column_goal() {
        let Some(mut app) = headless() else {
            println!("[test] SKIP: no usable adapter");
            return;
        };
        horizontal(&mut app);
        app.start_new_text([300.0, 300.0], None);
        type_lines(&mut app, "aaaaaaaa\nbb\ncccccccc");
        app.text_key(0x24, false, false); // Home — on line 3
        assert!(
            app.text_edit.as_ref().unwrap().goal.is_none(),
            "the goal went with it"
        );
        app.text_key(0x26, false, false); // Up: line 2, column 0
        assert_eq!(
            app.text_edit.as_ref().unwrap().caret,
            9,
            "up from the start of line 3 is the start of line 2"
        );
    }

    /// P1. Up on the first line and Down on the last used to be dead keys.
    #[test]
    fn up_on_the_first_line_goes_to_the_start_and_down_on_the_last_to_the_end() {
        let Some(mut app) = headless() else {
            println!("[test] SKIP: no usable adapter");
            return;
        };
        horizontal(&mut app);
        app.start_new_text([300.0, 300.0], None);
        type_lines(&mut app, "abc\ndef");
        app.text_key(0x24, true, false); // Ctrl+Home
        app.text_key(0x27, false, false); // Right, so the caret is at 1
        assert_eq!(app.text_edit.as_ref().unwrap().caret, 1);
        app.text_key(0x26, false, false); // Up on line 1
        assert_eq!(
            app.text_edit.as_ref().unwrap().caret,
            0,
            "to the very start"
        );

        app.text_key(0x23, true, false); // Ctrl+End
        let len = app.edited_item().unwrap().utf16_len();
        app.text_key(0x25, false, false); // Left: still on the last line
        app.text_key(0x28, false, false); // Down on the last line
        assert_eq!(
            app.text_edit.as_ref().unwrap().caret,
            len,
            "to the very end"
        );
    }

    /// P1. Ctrl+Shift+Z fell into the plain Ctrl+Z arm and UNDID; Ctrl+Y was
    /// swallowed. Both redo now, and a fresh edit throws the redo away.
    #[test]
    fn ctrl_shift_z_and_ctrl_y_redo_inside_the_session() {
        let Some(mut app) = headless() else {
            println!("[test] SKIP: no usable adapter");
            return;
        };
        app.start_new_text([300.0, 300.0], None);
        type_str(&mut app, "ab cd");
        assert!(app.text_key(0x5A, true, false)); // Ctrl+Z
        assert_eq!(typed(&app), "ab ");

        assert!(app.text_key(0x5A, true, true)); // Ctrl+Shift+Z
        assert_eq!(typed(&app), "ab cd", "the redo put the word back");
        assert!(app.text_editing(), "and it did NOT end the session");

        assert!(app.text_key(0x5A, true, false));
        assert!(app.text_key(0x5A, true, false));
        assert_eq!(typed(&app), "ab");
        assert!(app.text_key(0x59, true, false)); // Ctrl+Y
        assert_eq!(typed(&app), "ab ", "Ctrl+Y is the same key by another name");
        assert!(app.text_key(0x59, true, false));
        assert_eq!(typed(&app), "ab cd");
        assert!(
            app.text_key(0x59, true, false),
            "an empty redo is swallowed, not passed on"
        );
        assert_eq!(typed(&app), "ab cd");

        // Undo, then type: the future is gone, and the redo key cannot bring
        // back text that never followed this edit.
        assert!(app.text_key(0x5A, true, false));
        assert_eq!(typed(&app), "ab ");
        type_str(&mut app, "x");
        assert_eq!(typed(&app), "ab x");
        assert!(app.text_key(0x5A, true, true));
        assert_eq!(typed(&app), "ab x", "the old 'cd' is not coming back");
    }

    /// P1. Object tool: a double-click on a text box opens it for editing
    /// (CSP). A single click still just selects it for dragging.
    #[test]
    fn object_tool_double_click_opens_the_text_for_editing() {
        let Some(mut app) = headless() else {
            println!("[test] SKIP: no usable adapter");
            return;
        };
        horizontal(&mut app);
        app.start_new_text([300.0, 300.0], None);
        type_str(&mut app, "hello");
        let (x, y) = point_at(&app, 2);
        app.commit_text_edit();
        app.tool = Tool::Object;

        assert!(app.text_object_press(x, y, 1), "the box took the press");
        assert!(!app.text_editing(), "one click selects, it does not edit");
        assert!(app.text_obj_drag.is_some(), "and arms a drag");

        assert!(app.text_object_press(x, y, 2));
        assert!(app.text_editing(), "two clicks are the way in");
        assert!(app.text_obj_drag.is_none(), "the drag stood down");
        pump(&mut app);
        assert_eq!(app.tool, Tool::Text, "and the Text tool came up with it");
        assert_eq!(typed(&app), "hello", "editing the box that was clicked");
    }
}
