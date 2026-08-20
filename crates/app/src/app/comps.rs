//! Layer comps (TRIAGE 139, LC-001..009): named snapshots of the whole
//! layer-visibility state, one tap to apply. The comps live ON THE
//! DOCUMENT (`doc.comps`, persisted as `mnc-comps`) — the App keeps only
//! selection UX state — so LC-008 (apply across every page, strict
//! structure match) and LC-009 (batch export, one image set per comp)
//! ride the same store. LC-003's "Last document state" is the visibility
//! snapshot taken just before the most recent comp APPLICATION.

use super::App;

impl App {
    /// LC-001: snapshot the current visibility under a name.
    /// `doc.comps` is persisted document state (`mnc-comps`), so every
    /// mutation here touches the doc — without it, four comps set up and
    /// the app closed met no unsaved-changes prompt and were gone.
    pub fn comp_add(&mut self, name: &str) {
        let vis: Vec<bool> = self.doc.layers.iter().map(|l| l.visible).collect();
        self.doc.comps.push(mn_core::doc::LayerComp {
            name: name.to_string(),
            vis,
        });
        self.comp_selected = Some(self.doc.comps.len() - 1);
        self.doc.touch();
    }

    /// LC-005: overwrite comp `i` — the ROW whose 💾 was clicked, not the
    /// selected comp (the old argument-less form silently replaced
    /// whichever comp happened to be selected). Returns false on a bad
    /// index so the status line stays honest.
    pub fn comp_save(&mut self, i: usize) -> bool {
        let vis: Vec<bool> = self.doc.layers.iter().map(|l| l.visible).collect();
        let Some(c) = self.doc.comps.get_mut(i) else {
            return false;
        };
        c.vis = vis;
        self.doc.touch();
        true
    }

    /// LC-002: apply a comp. The pre-application state is stashed for
    /// LC-003's pinned row. Returns false on a bad index.
    pub fn comp_apply(&mut self, i: usize) -> bool {
        let Some(c) = self.doc.comps.get(i) else {
            return false;
        };
        let snap = c.vis.clone();
        self.comp_last_state = self.doc.layers.iter().map(|l| l.visible).collect();
        for (li, l) in self.doc.layers.iter_mut().enumerate() {
            l.visible = snap.get(li).copied().unwrap_or(self.comp_added_visible);
        }
        self.comp_selected = Some(i);
        self.comp_multi.clear();
        self.doc.touch();
        self.mark_dirty();
        true
    }

    /// LC-003: return to the state before the last comp application.
    pub fn comp_restore_last(&mut self) {
        let snap = std::mem::take(&mut self.comp_last_state);
        if snap.is_empty() {
            return;
        }
        for (li, l) in self.doc.layers.iter_mut().enumerate() {
            if let Some(v) = snap.get(li) {
                l.visible = *v;
            }
        }
        self.comp_selected = None;
        self.comp_multi.clear();
        self.doc.touch();
        self.mark_dirty();
    }

    /// LC-004: step to the previous/next comp in list order (wraps).
    pub fn comp_step(&mut self, forward: bool) {
        if self.doc.comps.is_empty() {
            return;
        }
        let n = self.doc.comps.len();
        let cur = self.comp_selected.map(|i| i as i64).unwrap_or(-1);
        let next = if forward {
            (cur + 1).rem_euclid(n as i64) as usize
        } else {
            (cur - 1 + n as i64).rem_euclid(n as i64) as usize
        };
        self.comp_apply(next);
    }

    /// LC-007: drag-reorder — move comp `src` to insertion slot `dst`
    /// (0..=len, the boundary the red insertion line sat on, counted on
    /// the ORIGINAL order). Both selections remap by identity. No-ops on
    /// a redundant move (dst == src or src+1 — "drop where it was").
    pub fn comp_move(&mut self, src: usize, dst: usize) {
        let n = self.doc.comps.len();
        if src >= n || dst > n || dst == src || dst == src + 1 {
            return;
        }
        let ins = if dst > src { dst - 1 } else { dst };
        let c = self.doc.comps.remove(src);
        self.doc.comps.insert(ins, c);
        let remap = |i: usize| -> usize {
            if i == src {
                ins
            } else if src < ins && i > src && i <= ins {
                i - 1
            } else if ins < src && i >= ins && i < src {
                i + 1
            } else {
                i
            }
        };
        self.comp_selected = self.comp_selected.map(remap);
        let multi = std::mem::take(&mut self.comp_multi);
        self.comp_multi = multi.iter().map(|&i| remap(i)).collect();
        // The multi-selection is an index SET — the remap permutes its
        // members, so it re-sorts.
        self.comp_multi.sort_unstable();
        self.doc.touch();
        self.mark_dirty();
    }

    /// LC-007: Ctrl+click toggles a comp in the multi-selection (the
    /// click anchor follows it).
    pub fn comp_toggle_multi(&mut self, i: usize) {
        if i >= self.doc.comps.len() {
            return;
        }
        if self.comp_multi.contains(&i) {
            self.comp_multi.retain(|&s| s != i);
        } else {
            self.comp_multi.push(i);
            self.comp_multi.sort_unstable();
        }
        self.comp_selected = Some(i);
    }

    /// LC-007: Shift+click selects the range from the anchor (the last
    /// single selection) through `i`.
    pub fn comp_range_select(&mut self, i: usize) {
        if i >= self.doc.comps.len() {
            return;
        }
        let a = self
            .comp_selected
            .unwrap_or(0)
            .min(self.doc.comps.len() - 1);
        let (lo, hi) = if a <= i { (a, i) } else { (i, a) };
        self.comp_multi = (lo..=hi).collect();
        self.comp_selected = Some(i);
    }

    /// Keep the selections valid after comp `i` is removed (the palette's
    /// delete): indices shift down above it, the selection itself drops.
    pub fn comp_delete_at(&mut self, i: usize) {
        if i >= self.doc.comps.len() {
            return;
        }
        self.doc.comps.remove(i);
        self.comp_selected = self
            .comp_selected
            .map(|s| {
                if s == i {
                    usize::MAX
                } else if s > i {
                    s - 1
                } else {
                    s
                }
            })
            .filter(|&s| s < self.doc.comps.len());
        let m = std::mem::take(&mut self.comp_multi);
        self.comp_multi = m
            .iter()
            .filter(|&&s| s != i)
            .map(|&s| if s > i { s - 1 } else { s })
            .collect();
        self.doc.touch();
    }
}
