//! Story Editor (TRIAGE 144, PM-040/045/046/047): every text field in the
//! chapter as an editable script. The active page edits the LIVE document
//! (one `set_texts` undo step per field); every other page edits a decoded
//! copy that re-encodes into its ORA bytes — same write-on-switch currency
//! the Pages panel uses. Hidden text layers are invisible to the editor
//! AND to its bulk operations (PM-047). Deferred: PM-041 (by design — it
//! erases per-page undo), PM-042..044 (script-side field create/split/
//! merge/move).

use super::App;
use mn_core::text::TextItem;

impl App {
    /// Decode every non-active page for script editing. The active page
    /// stays `None` — its edits go through the live doc.
    pub fn story_refresh(&mut self) {
        let live = self.page_index;
        self.story_docs = Vec::with_capacity(self.pages.len());
        for i in 0..self.pages.len() {
            if i == live {
                self.story_docs.push(None);
                continue;
            }
            let decoded = self.pages[i]
                .bytes
                .as_ref()
                .and_then(|b| mn_core::project::bytes_to_doc(b).ok());
            self.story_docs.push(decoded);
        }
    }

    /// All (page, layer, item) fields the editor shows — hidden layers
    /// excluded (PM-047). Iterates the live doc for the active page, the
    /// decoded copies for the rest.
    pub fn story_fields(&self) -> Vec<(usize, usize, usize)> {
        let mut out = Vec::new();
        for p in 0..self.pages.len() {
            let doc = match p {
                k if k == self.page_index => Some(&self.doc),
                _ => self.story_docs.get(p).and_then(|d| d.as_ref()),
            };
            let Some(doc) = doc else {
                continue;
            };
            for (l, layer) in doc.layers.iter().enumerate() {
                if !layer.visible {
                    continue;
                }
                if let Some(ts) = layer.texts() {
                    for i in 0..ts.texts.len() {
                        out.push((p, l, i));
                    }
                }
            }
        }
        out
    }

    /// One field's text.
    pub fn story_text(&self, p: usize, l: usize, i: usize) -> Option<String> {
        let doc = match p {
            k if k == self.page_index => Some(&self.doc),
            _ => self.story_docs.get(p).and_then(|d| d.as_ref()),
        }?;
        Some(doc.layers.get(l)?.texts()?.texts.get(i)?.text.clone())
    }

    /// Rebuild the edit buffers from the current state (after bulk ops).
    pub fn story_rebuffer(&mut self) {
        self.story_bufs = self
            .story_fields()
            .iter()
            .filter_map(|&(p, l, i)| self.story_text(p, l, i))
            .collect();
    }

    /// Open (or refresh) the editor: decode pages + rebuild buffers.
    pub fn story_open_refresh(&mut self) {
        self.story_refresh();
        self.story_rebuffer();
        self.story_open = true;
    }

    /// Write one field (the script window's edit path). Style runs are
    /// reset when the text LENGTH changes — the on-canvas editor maintains
    /// spans through cursor edits; a script edit does not know where the
    /// caret was, and the 198-vote use is uniform-style dialogue. Same
    /// length = pure restyle/whitespace edit, runs kept.
    pub fn story_set_text(&mut self, p: usize, l: usize, i: usize, text: &str) -> bool {
        let keep_runs;
        let new_set = {
            let doc = match p {
                k if k == self.page_index => &self.doc,
                _ => match self.story_docs.get(p).and_then(|d| d.as_ref()) {
                    Some(d) => d,
                    None => return false,
                },
            };
            let Some(ts) = doc.layers.get(l).and_then(|x| x.texts()) else {
                return false;
            };
            let Some(item) = ts.texts.get(i) else {
                return false;
            };
            let mut set = ts.clone();
            keep_runs = item.text.len() == text.len();
            let it = &mut set.texts[i];
            if !keep_runs {
                it.runs.clear();
            }
            it.text = text.to_string();
            set
        };
        if p == self.page_index {
            self.warm_texts(l);
            if self.doc.set_texts(l, new_set) {
                self.mark_dirty();
                return true;
            }
            false
        } else {
            let bytes = {
                let Some(Some(doc)) = self.story_docs.get_mut(p).map(|d| d.as_mut()) else {
                    return false;
                };
                if !doc.set_texts_raw(l, new_set) {
                    return false;
                }
                mn_core::project::doc_to_bytes(doc).ok()
            };
            let Some(b) = bytes else {
                return false;
            };
            let rev = self.page_rev_next();
            if let Some(e) = self.pages.get_mut(p) {
                e.bytes = Some(b);
                e.rev = rev;
                e.doc_rev = 0; // unknown decode state — force re-stash honesty
            }
            self.mark_pages_dirty();
            true
        }
    }

    /// PM-045: apply the Text tool's current settings (font, size,
    /// vertical, outline, alignment, spacing) to every visible field in
    /// the chapter. Returns the field count.
    ///
    /// `story_fields` lists one entry per ITEM but a restyle rewrites a
    /// whole text LAYER, so the targets collapse to distinct layers first
    /// (the list is already page- then layer-ordered, so `dedup` is
    /// enough). One write per layer = one raster, one re-encode and one
    /// undo step each: one Ctrl+Z takes the whole button press back.
    pub fn story_apply_tool_style(&mut self) -> usize {
        let mut n = 0;
        let mut targets: Vec<(usize, usize)> = self
            .story_fields()
            .into_iter()
            .map(|(p, l, _i)| (p, l))
            .collect();
        targets.dedup();
        for (p, l) in targets {
            let styled = {
                let doc = match p {
                    k if k == self.page_index => &self.doc,
                    _ => match self.story_docs.get(p).and_then(|d| d.as_ref()) {
                        Some(d) => d,
                        None => continue,
                    },
                };
                let Some(ts) = doc.layers.get(l).and_then(|x| x.texts()) else {
                    continue;
                };
                let mut set = ts.clone();
                for it in &mut set.texts {
                    apply_tool_style(it, self);
                }
                set
            };
            // One write, but the caller counts FIELDS, not layers.
            let fields = styled.texts.len();
            if p == self.page_index {
                self.warm_texts(l);
                if self.doc.set_texts(l, styled) {
                    n += fields;
                }
            } else {
                let bytes = {
                    let Some(Some(doc)) = self.story_docs.get_mut(p).map(|d| d.as_mut()) else {
                        continue;
                    };
                    if !doc.set_texts_raw(l, styled) {
                        continue;
                    }
                    mn_core::project::doc_to_bytes(doc).ok()
                };
                if let Some(b) = bytes {
                    let rev = self.page_rev_next();
                    if let Some(e) = self.pages.get_mut(p) {
                        e.bytes = Some(b);
                        e.rev = rev;
                        e.doc_rev = 0;
                    }
                    self.mark_pages_dirty();
                    n += fields;
                }
            }
        }
        n
    }

    /// PM-046: chapter-wide find-and-replace. Returns (fields hit,
    /// occurrences).
    pub fn story_replace_all(
        &mut self,
        find: &str,
        repl: &str,
        ignore_case: bool,
    ) -> (usize, usize) {
        if find.is_empty() {
            return (0, 0);
        }
        let mut fields = 0;
        let mut occ = 0;
        for (p, l, i) in self.story_fields() {
            let Some(cur) = self.story_text(p, l, i) else {
                continue;
            };
            let (hay, needle) = if ignore_case {
                (cur.to_lowercase(), find.to_lowercase())
            } else {
                (cur.clone(), find.to_string())
            };
            if !hay.contains(&needle) {
                continue;
            }
            let new = if ignore_case {
                replace_ignore_case(&cur, find, repl)
            } else {
                cur.replace(find, repl)
            };
            occ += if ignore_case {
                hay.matches(&needle).count()
            } else {
                cur.matches(find).count()
            };
            if self.story_set_text(p, l, i, &new) {
                fields += 1;
            }
        }
        (fields, occ)
    }
}

/// PM-045's source of truth: the Text tool's settings, applied to one item.
fn apply_tool_style(it: &mut TextItem, app: &App) {
    it.font = app.text_font.clone();
    it.size_pt = app.text_size_pt;
    it.vertical = app.text_vertical;
    it.outline_px = app.text_outline_mm / 25.4 * app.doc_dpi().max(1) as f32;
    it.align = app.text_align;
    it.frame_align = app.text_frame_align;
    it.letter_spacing_pt = app.text_letter_pt;
    it.line_spacing = app.text_line;
}

/// Case-insensitive replace; the replacement keeps its own case.
fn replace_ignore_case(text: &str, find: &str, repl: &str) -> String {
    let f: Vec<char> = find.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < t.len() {
        if i + f.len() <= t.len()
            && t[i..i + f.len()]
                .iter()
                .zip(&f)
                .all(|(a, b)| a.to_lowercase().eq(b.to_lowercase()))
        {
            out.push_str(repl);
            i += f.len();
        } else {
            out.push(t[i]);
            i += 1;
        }
    }
    out
}

impl App {
    // --- PM-042/043 (TRIAGE 144 remainder): script-side field
    // create/split/merge — the script drives the canvas.

    /// A style template for a NEW field: the page's last visible text
    /// item, else any in the chapter, else tool defaults.
    pub(crate) fn story_item_template(&self, p: usize) -> mn_core::text::TextItem {
        let doc_of = |p: usize| -> Option<&mn_core::Document> {
            if p == self.page_index {
                Some(&self.doc)
            } else {
                self.story_docs.get(p).and_then(|d| d.as_ref())
            }
        };
        // The FIELD'S OWN PAGE first (audit H, 2026-08-19: this scanned
        // from page 0 ignoring `p`, so a new field was styled from
        // whatever page 1 held — and hidden layers counted too).
        let mut order: Vec<usize> = vec![p];
        order.extend((0..self.pages.len()).filter(|&q| q != p));
        for q in order {
            let Some(doc) = doc_of(q) else { continue };
            for layer in doc.layers.iter().rev() {
                if !layer.visible {
                    continue;
                }
                if let Some(ts) = layer.texts() {
                    if let Some(it) = ts.texts.last() {
                        let mut t = it.clone();
                        // A template styles a NEW field — it must not carry
                        // the source item's identity (the commit re-mints).
                        t.id = 0;
                        return t;
                    }
                }
            }
        }
        mn_core::text::TextItem {
            id: 0,
            text: String::new(),
            runs: Vec::new(),
            pos: [64.0, 64.0],
            size: [200.0, 40.0],
            auto_size: true,
            rotation: 0.0,
            font: "serif".into(),
            size_pt: 12.0,
            color: [0, 0, 0],
            outline_px: 0.0,
            outline_color: [255, 255, 255],
            vertical: true,
            align: Default::default(),
            frame_align: Default::default(),
            letter_spacing_pt: 0.0,
            line_spacing: Default::default(),
            ruby: Vec::new(),
            ruby_style: mn_core::text::RubyStyle::default(),
            tcy: Vec::new(),
            auto_tcy: 0,
            fonts: Vec::new(),
            style: None,
            cache: None,
        }
    }

    /// PM-042: a new text field on page `p` — appended under the page's
    /// last visible text layer's last item when one exists, else a NEW
    /// text layer at the top of the stack. Returns the (layer, item)
    /// the editor should focus. Enter twice / the + button land here.
    pub fn story_new_field(&mut self, p: usize) -> Option<(usize, usize)> {
        let mut tpl = self.story_item_template(p);
        // The write paths below need the doc; find the last text layer
        // first (read-only pass).
        let last = {
            let doc = if p == self.page_index {
                &self.doc
            } else {
                self.story_docs.get(p).and_then(|d| d.as_ref())?
            };
            doc.layers
                .iter()
                .rposition(|l| l.visible && l.texts().is_some_and(|t| !t.texts.is_empty()))
        };
        match last {
            Some(l) => {
                let mut set = {
                    let doc = if p == self.page_index {
                        &self.doc
                    } else {
                        self.story_docs.get(p).and_then(|d| d.as_ref())?
                    };
                    doc.layers[l].texts()?.clone()
                };
                let tail = set.texts.last().cloned().unwrap_or_else(|| tpl.clone());
                tpl.pos = [tail.pos[0], tail.pos[1] + tail.size[1] + 16.0];
                set.texts.push(tpl);
                let i = set.texts.len() - 1;
                if !self.story_write_set(p, l, set) {
                    return None;
                }
                Some((l, i))
            }
            None => {
                // Position by the TARGET page's size, not the active
                // page's (audit H — they differ in a mixed-size project).
                let (w, h) = {
                    let doc = if p == self.page_index {
                        &self.doc
                    } else {
                        self.story_docs.get(p).and_then(|d| d.as_ref())?
                    };
                    doc.size
                };
                tpl.pos = [w as f32 * 0.5 - tpl.size[0] * 0.5, h as f32 * 0.3];
                let set = mn_core::TextSet { texts: vec![tpl] };
                let l = if p == self.page_index {
                    self.doc.add_text_layer("Text", set.clone());
                    self.mark_dirty();
                    self.doc.layers.len() - 1
                } else {
                    let Some(Some(doc)) = self.story_docs.get_mut(p).map(|d| d.as_mut()) else {
                        return None;
                    };
                    let l = doc.add_text_layer("Text", set);
                    let bytes = mn_core::project::doc_to_bytes(doc).ok()?;
                    let rev = self.page_rev_next();
                    let e = self.pages.get_mut(p)?;
                    e.bytes = Some(bytes);
                    e.rev = rev;
                    e.doc_rev = 0;
                    l
                };
                Some((l, 0))
            }
        }
    }

    /// PM-043: split field (p,l,i) at BYTE offset `at` — the tail
    /// becomes a new field directly below (runs drop: both halves render
    /// plain; the script is the source of truth).
    pub fn story_split_field(&mut self, p: usize, l: usize, i: usize, at: usize) -> bool {
        let mut set = {
            let doc = if p == self.page_index {
                &self.doc
            } else {
                match self.story_docs.get(p).and_then(|d| d.as_ref()) {
                    Some(d) => d,
                    None => return false,
                }
            };
            let Some(ts) = doc.layers.get(l).and_then(|x| x.texts()) else {
                return false;
            };
            ts.clone()
        };
        let Some(item) = set.texts.get(i) else {
            return false;
        };
        let text = item.text.clone();
        let at = at.min(text.len());
        if !text.is_char_boundary(at) {
            return false;
        }
        let (head, tail) = text.split_at(at);
        let mut below = item.clone();
        below.text = tail.to_string();
        below.runs.clear();
        below.pos = [item.pos[0], item.pos[1] + item.size[1] + 16.0];
        let mut head_item = item.clone();
        head_item.text = head.to_string();
        head_item.runs.clear();
        set.texts[i] = head_item;
        set.texts.insert(i + 1, below);
        self.story_write_set(p, l, set)
    }

    /// PM-043: merge field (p,l,i) with its PREVIOUS field on the same
    /// layer (Backspace at the start). Returns false at the layer's
    /// first field — nothing to merge with.
    pub fn story_merge_field(&mut self, p: usize, l: usize, i: usize) -> bool {
        if i == 0 {
            return false;
        }
        let mut set = {
            let doc = if p == self.page_index {
                &self.doc
            } else {
                match self.story_docs.get(p).and_then(|d| d.as_ref()) {
                    Some(d) => d,
                    None => return false,
                }
            };
            let Some(ts) = doc.layers.get(l).and_then(|x| x.texts()) else {
                return false;
            };
            ts.clone()
        };
        let Some(cur) = set.texts.get(i) else {
            return false;
        };
        let tail = cur.text.clone();
        set.texts.remove(i);
        let Some(prev) = set.texts.get_mut(i - 1) else {
            return false;
        };
        prev.text.push_str(&tail);
        prev.runs.clear();
        self.story_write_set(p, l, set)
    }

    /// The dual write path shared by split/merge/new-item: live set_texts
    /// (one undo step) on the active page, set_texts_raw + re-encode on
    /// the decoded copies.
    fn story_write_set(&mut self, p: usize, l: usize, set: mn_core::TextSet) -> bool {
        if p == self.page_index {
            self.warm_texts(l);
            if self.doc.set_texts(l, set) {
                self.mark_dirty();
                return true;
            }
            false
        } else {
            let bytes = {
                let Some(Some(doc)) = self.story_docs.get_mut(p).map(|d| d.as_mut()) else {
                    return false;
                };
                if !doc.set_texts_raw(l, set) {
                    return false;
                }
                mn_core::project::doc_to_bytes(doc).ok()
            };
            let Some(b) = bytes else {
                return false;
            };
            let rev = self.page_rev_next();
            if let Some(e) = self.pages.get_mut(p) {
                e.bytes = Some(b);
                e.rev = rev;
                e.doc_rev = 0;
            }

            self.mark_pages_dirty();
            true
        }
    }
}

impl App {
    // --- PM-044 (TRIAGE 144, the last deferred piece): move / duplicate
    // a field to ANOTHER page from the script side — "without opening
    // either". The editor's toolbar carries the selected field to the
    // chosen page; drag gestures stay a later UI nicety.

    /// Move (or duplicate) field (p,l,i) to page `q`'s last text layer
    /// (creating one when the page has none — the story_new_field rules).
    /// A MOVE that empties the source layer removes that layer (an empty
    /// text layer is junk). Cross-page by construction: no single undo
    /// step spans two documents — the active page's half keeps its own
    /// undo, the other half re-encodes (the story-write convention).
    pub fn story_move_field(
        &mut self,
        p: usize,
        l: usize,
        i: usize,
        q: usize,
        duplicate: bool,
    ) -> bool {
        if q >= self.pages.len() || q == p {
            // Same-page targets are refused OUTRIGHT (audit A, 2026-08-19):
            // this control targets pages, and the two-document path run
            // against one document deletes the field — the pre-write clone
            // written back at the end overwrites the placement copy.
            return false;
        }
        // 1. Lift the item (and its set) from the source.
        let (mut set, item) = {
            let doc = if p == self.page_index {
                &self.doc
            } else {
                match self.story_docs.get(p).and_then(|d| d.as_ref()) {
                    Some(d) => d,
                    None => return false,
                }
            };
            let Some(ts) = doc.layers.get(l).and_then(|x| x.texts()) else {
                return false;
            };
            let Some(item) = ts.texts.get(i).cloned() else {
                return false;
            };
            (ts.clone(), item)
        };
        // 2. Place on the target: append under its last text layer.
        // (story_docs must hold q's decode; the editor keeps them warm.)
        let placed = 'place: {
            let last = {
                let doc = if q == self.page_index {
                    &self.doc
                } else {
                    match self.story_docs.get(q).and_then(|d| d.as_ref()) {
                        Some(d) => d,
                        None => break 'place None,
                    }
                };
                doc.layers
                    .iter()
                    .rposition(|x| x.visible && x.texts().is_some_and(|t| !t.texts.is_empty()))
            };
            match last {
                Some(tl) => {
                    let mut tset = {
                        let doc = if q == self.page_index {
                            &self.doc
                        } else {
                            self.story_docs.get(q).and_then(|d| d.as_ref()).unwrap()
                        };
                        doc.layers[tl].texts().unwrap().clone()
                    };
                    let mut below = item.clone();
                    if let Some(tail) = tset.texts.last() {
                        below.pos = [tail.pos[0], tail.pos[1] + tail.size[1] + 16.0];
                    }
                    tset.texts.push(below);
                    if !self.story_write_set(q, tl, tset) {
                        break 'place None;
                    }
                    Some(())
                }
                None => {
                    // Target page has no text layer: make one (the
                    // story_new_field path).
                    let mut fresh = item.clone();
                    fresh.pos = [64.0, 64.0];
                    let tset = mn_core::TextSet { texts: vec![fresh] };
                    if q == self.page_index {
                        self.doc.add_text_layer("Text", tset);
                        self.mark_dirty();
                    } else {
                        let Some(Some(doc)) = self.story_docs.get_mut(q).map(|d| d.as_mut()) else {
                            break 'place None;
                        };
                        doc.add_text_layer("Text", tset);
                        let Ok(b) = mn_core::project::doc_to_bytes(doc) else {
                            break 'place None;
                        };
                        let rev = self.page_rev_next();
                        let Some(e) = self.pages.get_mut(q) else {
                            break 'place None;
                        };
                        e.bytes = Some(b);
                        e.rev = rev;
                        e.doc_rev = 0;
                    }
                    Some(())
                }
            }
        };
        let Some(()) = placed else {
            return false;
        };
        // 3. Remove from the source (moves only).
        if !duplicate {
            set.texts.remove(i);
            if set.texts.is_empty() {
                if p == self.page_index {
                    let _ = self.doc.remove_layer(l);
                    self.mark_dirty();
                } else {
                    let Some(Some(doc)) = self.story_docs.get_mut(p).map(|d| d.as_mut()) else {
                        return true; // placed; source cleanup failed — honest partial
                    };
                    let _ = doc.remove_layer(l);
                    if let Ok(b) = mn_core::project::doc_to_bytes(doc) {
                        let rev = self.page_rev_next();
                        if let Some(e) = self.pages.get_mut(p) {
                            e.bytes = Some(b);
                            e.rev = rev;
                            e.doc_rev = 0;
                        }
                    }
                }
            } else if !self.story_write_set(p, l, set) {
                return true; // placed; source write failed — honest partial
            }
        }
        true
    }
}

// --- PM-053 "Write text to file" ----------------------------------------
// The translator/letterer handoff: every text item in the chapter, in
// the order a reader meets it, as a plain `.txt`. Both halves already
// existed — the Story Editor walks text, `frame_order` numbers panels —
// so this is the seam between them, not new machinery.

/// One text item reduced to what the dump needs.
struct DumpItem {
    text: String,
    cx: f32,
    cy: f32,
}

/// One balloon is ONE line: internal line breaks are typesetting, not
/// content, so every whitespace run collapses to a single space. A
/// translator wants one segment per balloon, and the breaks are the
/// letterer's to redo anyway.
fn flatten_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The page's panels in reading order (empty when the page has no frame
/// folders). Same inputs `App::renumber_frames` feeds the core, but off
/// an arbitrary document — the dump reads pages that are not open.
fn page_panels(
    doc: &mn_core::Document,
    rtl: bool,
    spread: bool,
    tol: f32,
) -> Vec<mn_core::frame_order::PanelRef> {
    let sets: Vec<Option<&mn_core::FrameSet>> = doc.layers.iter().map(|l| l.frames()).collect();
    let folders: Vec<mn_core::frame_order::FolderInput<'_>> = doc
        .layers
        .iter()
        .enumerate()
        .filter(|(i, l)| l.folder && l.is_frame() && sets[*i].is_some())
        .map(|(i, _)| mn_core::frame_order::FolderInput {
            layer: i,
            set: sets[i].unwrap(),
            pin: sets[i].unwrap().reading_pin,
        })
        .collect();
    if folders.is_empty() {
        return Vec::new();
    }
    mn_core::frame_order::reading_order(&folders, rtl, spread, doc.size.0 as f32, tol).panels
}

impl App {
    /// The reading-order sort INSIDE one panel. Vertical Japanese reads
    /// right-to-left in columns, so a right-bound work sorts by column
    /// band first and height second; a left-bound one reads in rows and
    /// sorts the other way round. `tol` (the page's gutter) is the band
    /// width — two balloons closer than that count as one column.
    fn dump_sort(&self, items: &[DumpItem], keys: &mut [usize], tol: f32) {
        let tol = tol.max(1.0);
        let rtl = self.binding_right;
        keys.sort_by_key(|&k| {
            let it = &items[k];
            if rtl {
                (-((it.cx / tol).floor() as i64), it.cy as i64)
            } else {
                ((it.cy / tol).floor() as i64, it.cx as i64)
            }
        });
    }

    /// PM-053: the whole chapter's text as a plain script.
    ///
    /// Format, one page at a time: a `== Page N ==` marker for EVERY
    /// page (so page numbers stay countable even where nothing is
    /// written), then `-- Panel N --` for each panel that holds text,
    /// numbered by the same reading order the Layers badges show. A text
    /// item is filed by its CENTRE point, so a balloon straddling a
    /// gutter lands in the panel it mostly sits in. Items in no panel
    /// come last under `-- Outside panels --`; on a page with no panels
    /// at all they simply follow the page marker with no panel row,
    /// because "outside" means nothing there.
    ///
    /// Hidden text is skipped — the layer's own eye AND its folders'
    /// (this dump is what gets printed, and neither does). Note the
    /// Story Editor's PM-047 rule only tests the layer's own eye; this
    /// one is the stricter of the two.
    pub fn script_dump(&self) -> String {
        let tol = self
            .mm_to_px(self.gutter_folder_mm.0.max(self.gutter_border_mm.0))
            .max(2.0);
        let mut lines: Vec<String> = Vec::new();
        lines.push("MangaNakama script export".to_owned());
        if !self.story.trim().is_empty() {
            lines.push(format!("Work: {}", self.story.trim()));
        }
        lines.push(format!("Pages: {}", self.pages.len()));
        lines.push(String::new());

        for p in 0..self.pages.len() {
            lines.push(format!("== Page {} ==", p + 1));
            // The ACTIVE page reads from the live document (unsaved
            // typing included); the rest decode their stashed bytes, the
            // same way `run_preflight` reaches them.
            let decoded = if p == self.page_index {
                None
            } else {
                self.pages[p]
                    .bytes
                    .as_ref()
                    .and_then(|b| mn_core::project::bytes_to_doc(b).ok())
            };
            let doc = if p == self.page_index {
                Some(&self.doc)
            } else {
                decoded.as_ref()
            };
            let Some(doc) = doc else {
                lines.push("(page could not be read)".to_owned());
                lines.push(String::new());
                continue;
            };

            let vis = doc.effective_visibility();
            let mut items: Vec<DumpItem> = Vec::new();
            for (li, l) in doc.layers.iter().enumerate() {
                if !vis.get(li).copied().unwrap_or(false) {
                    continue;
                }
                let Some(ts) = l.texts() else { continue };
                for it in &ts.texts {
                    let text = flatten_line(&it.text);
                    if text.is_empty() {
                        continue;
                    }
                    items.push(DumpItem {
                        text,
                        cx: it.pos[0] + it.size[0] * 0.5,
                        cy: it.pos[1] + it.size[1] * 0.5,
                    });
                }
            }

            let spread = self.pages.get(p).is_some_and(|e| e.spread);
            let panels = page_panels(doc, self.binding_right, spread, tol);
            let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); panels.len()];
            let mut orphans: Vec<usize> = Vec::new();
            for (k, it) in items.iter().enumerate() {
                let hit = panels.iter().position(|pr| {
                    doc.layers
                        .get(pr.layer)
                        .and_then(|l| l.frames())
                        .and_then(|f| f.frames.get(pr.frame))
                        .is_some_and(|fr| fr.contains([it.cx, it.cy]))
                });
                match hit {
                    Some(n) => buckets[n].push(k),
                    None => orphans.push(k),
                }
            }
            for b in buckets.iter_mut() {
                self.dump_sort(&items, b, tol);
            }
            self.dump_sort(&items, &mut orphans, tol);

            for (n, b) in buckets.iter().enumerate() {
                if b.is_empty() {
                    continue;
                }
                lines.push(format!("-- Panel {} --", n + 1));
                lines.extend(b.iter().map(|&k| items[k].text.clone()));
            }
            if !orphans.is_empty() {
                if !panels.is_empty() {
                    lines.push("-- Outside panels --".to_owned());
                }
                lines.extend(orphans.iter().map(|&k| items[k].text.clone()));
            }
            lines.push(String::new());
        }

        // UTF-8 with a BOM and CRLF ends: this file leaves the app for
        // Notepad, Word and Excel on a Japanese Windows box, and those
        // two details are what stop it arriving as mojibake or as one
        // endless line. Our OWN sidecars (ui.txt and friends) stay bare
        // LF — different audience, no precedent broken.
        let mut out = String::from("\u{feff}");
        for l in lines {
            out.push_str(&l);
            out.push_str("\r\n");
        }
        out
    }
}
