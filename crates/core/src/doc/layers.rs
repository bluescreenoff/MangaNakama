use super::*;

impl Document {
    // ---------------------------------------------------------- layer ops --
    //
    // Order convention: `layers[0]` is the **bottom** layer, composited first;
    // the last element is the top. (ORA's stack.xml is the other way round —
    // `core::ora` reverses on the way in and out.)
    //
    // Any op that shifts layer indices records a `UndoGroup::Structure`
    // snapshot through `record_structure` (pattern: take `stack_snapshot()`
    // + `active` BEFORE the first mutation, record on the success path).
    // Index-carrying groups deeper in the stack stay valid because undo is
    // LIFO — see the enum's doc comment. The one exception is `resize_to`,
    // which still clears (canvas size is outside the snapshot).

    // ------------------------------------------------------------ folders --
    //
    // Folders are encoded *flat*: a folder header at depth d owns the
    // contiguous run of layers directly below it (lower indices) whose depth
    // is > d. Undo indices, the GPU tile cache and every existing iteration
    // keep working; only presentation (visibility/opacity) cascades, via
    // `effective_presentation`.

    /// The indices of `index`'s children (empty when it is not a folder or has
    /// none). Includes nested descendants — a folder inside a folder counts
    /// with everything in it.
    pub fn children_range(&self, index: usize) -> std::ops::Range<usize> {
        if index >= self.layers.len() || !self.layers[index].folder {
            return index..index;
        }
        let d = self.layers[index].depth;
        let mut s = index;
        while s > 0 && self.layers[s - 1].depth > d {
            s -= 1;
        }
        s..index
    }

    /// The block a structural op moves as one unit: the layer itself, plus its
    /// children when it is a folder. Always non-empty, ends at `index`
    /// inclusive.
    pub fn block_range(&self, index: usize) -> std::ops::Range<usize> {
        let c = self.children_range(index);
        c.start..index + 1
    }

    /// The innermost folder ENCLOSING `index` (None at the top level).
    /// Equal depth is not parenthood: two folders at the same depth may
    /// live in different parents, and combining them would land the
    /// result in one parent while silently emptying the other (audit H,
    /// 2026-08-19) — the combine paths compare these instead.
    pub fn enclosing_folder(&self, index: usize) -> Option<usize> {
        let d = self.layers.get(index)?.depth;
        ((index + 1)..self.layers.len()).find(|&i| {
            let l = &self.layers[i];
            l.folder && l.depth < d
        })
    }

    /// Per-layer visibility with every ancestor folder's eye folded in.
    /// Opacity does NOT cascade here: with true group isolation a folder's
    /// opacity is applied once, when its composited group blends onto the
    /// backdrop — the compositors read each layer's own opacity.
    pub fn effective_visibility(&self) -> Vec<bool> {
        let mut out: Vec<bool> = self.layers.iter().map(|l| l.visible).collect();
        for i in 0..self.layers.len() {
            if self.layers[i].folder && !self.layers[i].visible {
                for j in self.children_range(i) {
                    out[j] = false;
                }
            }
        }
        out
    }

    /// For each layer, the index of the layer its `clip` flag clips it to:
    /// the nearest layer below at the same depth that is not itself clipped —
    /// a plain layer, or a FOLDER header (clip-to-folder, scenario 2a: the
    /// group's combined ink is the base). A THROUGH folder has no isolated
    /// composite to clip to, so it breaks the chain like no base at all.
    /// `None` = not clipped (or no valid base — the flag is then ignored,
    /// CSP-style).
    pub fn clip_bases(&self) -> Vec<Option<usize>> {
        let n = self.layers.len();
        let mut out = vec![None; n];
        for i in 0..n {
            let l = &self.layers[i];
            if !l.clip || l.folder {
                continue;
            }
            let mut j = i;
            while j > 0 {
                j -= 1;
                let b = &self.layers[j];
                if b.depth != l.depth {
                    break;
                }
                if b.folder {
                    if !b.through {
                        out[i] = Some(j);
                    }
                    break;
                }
                if !b.clip {
                    out[i] = Some(j);
                    break;
                }
            }
        }
        out
    }

    /// Repair the depth invariant after a structural edit: a layer can never
    /// be deeper than the layer above it allows (folder above → its depth + 1,
    /// plain layer above → its depth). `pub(crate)` for the ORA loader.
    pub(crate) fn normalize_depths(&mut self) {
        let mut allowed: u8 = 0;
        for i in (0..self.layers.len()).rev() {
            let l = &mut self.layers[i];
            if l.depth > allowed {
                l.depth = allowed;
            }
            allowed = l.depth + u8::from(l.folder);
        }
    }

    /// Re-derive a frame layer's raster from its vectors. A frame FOLDER gets
    /// border ink in `tiles` + the panel coverage in `mask_tiles` (true
    /// isolation); a flat frame layer keeps the round-7 white-gutter raster.
    pub(crate) fn derive_frame_raster(l: &mut Layer, size: (u32, u32)) {
        let LayerKind::Frame(fs) = &l.kind else {
            return;
        };
        let fs = fs.clone();
        if l.folder {
            l.replace_tiles(fs.rasterize_border(size));
            l.replace_mask_tiles(Some(fs.rasterize_mask(size)));
        } else {
            l.replace_tiles(fs.rasterize(size));
            l.replace_mask_tiles(None);
        }
    }

    /// Toggle a folder's expand state (presentation only, like rename).
    pub fn set_folder_open(&mut self, index: usize, open: bool) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        if !l.folder {
            return false;
        }
        l.open = open;
        self.touch();
        true
    }

    /// New empty folder directly above `index`'s block, same depth, active.
    /// Records one structural undo step. Returns the new index.
    pub fn add_folder_above(&mut self, index: usize, name: impl Into<String>) -> usize {
        let (before, active_before) = (self.stack_snapshot(), self.active);
        let depth = self.layers.get(index).map(|l| l.depth).unwrap_or(0);
        let at = (index + 1).min(self.layers.len());
        let mut f = Layer::new(name);
        f.folder = true;
        f.depth = depth;
        self.layers.insert(at, f);
        self.active = at;
        self.normalize_depths();
        self.record_structure("New folder", before, active_before);
        self.touch();
        at
    }

    /// New empty layer as the **topmost child** of the folder at `index`, and
    /// make it active. Records one structural undo step. Returns the new index.
    pub fn add_layer_in_folder(&mut self, index: usize, name: impl Into<String>) -> Option<usize> {
        if !self.layers.get(index)?.folder {
            return None;
        }
        let (before, active_before) = (self.stack_snapshot(), self.active);
        let mut l = Layer::new(name);
        l.depth = self.layers[index].depth + 1;
        self.layers.insert(index, l);
        self.active = index;
        self.record_structure("New layer", before, active_before);
        self.touch();
        Some(index)
    }

    /// Move a whole block (a layer, or a folder with everything in it) so its
    /// bottom lands at gap `slot` (an insertion point in the **current**
    /// stack, 0..=len), and give the moved layer depth `depth` (children keep
    /// their relative depths; the result is normalized). Refuses to drop a
    /// folder into itself. Records one structural undo step.
    pub fn move_block_to_slot(&mut self, from: usize, slot: usize, depth: u8) -> bool {
        let n = self.layers.len();
        if from >= n || slot > n {
            return false;
        }
        let r = self.block_range(from);
        if slot > r.start && slot <= from {
            return false; // inside the moving block
        }
        let base = self.layers[from].depth;
        if (slot == r.start || slot == from + 1) && depth == base {
            return false; // dropped where it already sits
        }
        let (before, active_before) = (self.stack_snapshot(), self.active);
        let active_offset = if self.active >= r.start && self.active <= from {
            Some(self.active - r.start)
        } else {
            None
        };
        let block: Vec<Layer> = self.layers.drain(r.clone()).collect();
        let k = block.len();
        let at = if slot > from { slot - k } else { slot };
        for (i, mut l) in block.into_iter().enumerate() {
            // Children shift with the header; saturate rather than wrap.
            let rel = l.depth.saturating_sub(base);
            l.depth = depth.saturating_add(rel);
            self.layers.insert(at + i, l);
        }
        self.active = match active_offset {
            Some(off) => at + off,
            None => {
                let a = self.active;
                if slot > from && a > from && a < slot {
                    a - k // was between the block and the gap; block hopped over it
                } else if slot <= r.start && a >= slot && a < r.start {
                    a + k // block landed under it
                } else {
                    a
                }
            }
        };
        self.normalize_depths();
        self.record_structure("Move layer", before, active_before);
        self.touch();
        true
    }

    /// Insert a new empty layer directly above `index` (same depth — a
    /// sibling) and make it active. Returns the new layer's index. Records
    /// one structural undo step.
    /// The top of the clip run riding `index`: the last CONSECUTIVE clipped
    /// (same depth, non-folder) layer above it — `index` itself when nothing
    /// rides. From a mid-run member it finds the same top, so an insert
    /// relative to any member of the run lands outside it.
    pub fn clip_run_top(&self, index: usize) -> usize {
        let Some(l) = self.layers.get(index) else {
            return index;
        };
        let mut j = index;
        while let Some(n) = self.layers.get(j + 1) {
            if n.depth != l.depth || n.folder || !n.clip {
                break;
            }
            j += 1;
        }
        j
    }

    pub fn add_layer_above(&mut self, index: usize, name: impl Into<String>) -> usize {
        let (before, active_before) = (self.stack_snapshot(), self.active);
        // docs/CLIPPING-SCENARIOS.md: a plain insert INSIDE a clip run would
        // silently re-base the members above it onto the new empty layer —
        // everything clipped goes invisible. Hop above the run instead; a
        // layer meant to join the run still can (clip resolves through the
        // members to the same base wherever it sits in the run).
        let index = self.clip_run_top(index);
        let at = (index + 1).min(self.layers.len());
        let mut l = Layer::new(name);
        l.depth = self.layers.get(index).map(|x| x.depth).unwrap_or(0);
        self.layers.insert(at, l);
        self.active = at;
        self.normalize_depths();
        self.record_structure("New layer", before, active_before);
        self.touch();
        at
    }

    /// Insert a new empty layer above the active one and make it active.
    pub fn add_layer(&mut self, name: impl Into<String>) -> usize {
        self.add_layer_above(self.active, name)
    }

    /// Current index of the layer with stable id `id`. THE door for anything
    /// holding an id across edits (automation, future cross-references) —
    /// linear, stacks are small.
    pub fn layer_index_of(&self, id: u64) -> Option<usize> {
        self.layers.iter().position(|l| l.id == id)
    }

    /// Make every stable id in the document real and unique: layers, text
    /// items and balloons. `0` (a file from before ids existed, or a fresh
    /// item awaiting its commit) and duplicates (a hand-edited file; first
    /// occurrence keeps the id) are reminted. Lifts the mint past the largest
    /// id seen FIRST, so a heal can never hand out an id the file also holds.
    /// Called by the ORA loader; harmless anywhere else.
    pub fn ensure_ids(&mut self) {
        let mut max = 0u64;
        for l in &self.layers {
            max = max.max(l.id);
            if let LayerKind::Speech(sp) = &l.kind {
                for t in &sp.texts.texts {
                    max = max.max(t.id);
                }
                for b in &sp.balloons.balloons {
                    max = max.max(b.id);
                }
            }
        }
        bump_ids_past(max);
        let mut seen = std::collections::HashSet::new();
        for l in &mut self.layers {
            if l.id == 0 || !seen.insert(l.id) {
                l.id = mint_id();
                seen.insert(l.id);
            }
            if let LayerKind::Speech(sp) = &mut l.kind {
                sp.mint_ids();
            }
        }
    }

    /// Remove a layer — a folder goes with everything inside it. Refuses to
    /// empty the document and refuses an out-of-range index; both return
    /// `false`. Records one structural undo step.
    pub fn remove_layer(&mut self, index: usize) -> bool {
        if index >= self.layers.len() {
            return false;
        }
        let r = self.block_range(index);
        if r.len() >= self.layers.len() {
            return false;
        }
        let (before, active_before) = (self.stack_snapshot(), self.active);
        self.layers.drain(r);
        if self.active >= self.layers.len() {
            self.active = self.layers.len() - 1;
        }
        self.normalize_depths();
        self.record_structure("Delete layer", before, active_before);
        self.touch();
        true
    }

    /// Copy a layer (pixels included — `Arc` clones, so it is cheap until one
    /// of the two is painted on) and insert the copy above it. A folder is
    /// copied with its children. Returns the new index of the copied layer.
    /// Records one structural undo step.
    pub fn duplicate_layer(&mut self, index: usize) -> Option<usize> {
        if index >= self.layers.len() {
            return None;
        }
        let (before, active_before) = (self.stack_snapshot(), self.active);
        let r = self.block_range(index);
        let mut block: Vec<Layer> = self.layers[r.clone()].to_vec();
        for l in &mut block {
            // A clone must never inherit an open op's recording.
            l.recording = None;
            // Both copies live from here on — the copy is a NEW identity.
            l.id = mint_id();
        }
        if let Some(top) = block.last_mut() {
            top.name = format!("{} copy", top.name);
        }
        let k = block.len();
        for (i, l) in block.into_iter().enumerate() {
            self.layers.insert(index + 1 + i, l);
        }
        let at = index + k;
        self.active = at;
        self.record_structure("Duplicate layer", before, active_before);
        self.touch();
        Some(at)
    }

    /// Move a layer to a new index (reorder), keeping its depth. `to` is the
    /// index it should end up at in the resulting stack. A folder moves with
    /// its children. Returns `false` on a bad index. Records one structural undo step;
    /// keeps the moved layer active if it already was.
    pub fn move_layer(&mut self, from: usize, to: usize) -> bool {
        let n = self.layers.len();
        if from >= n || to >= n {
            return false;
        }
        if from == to {
            return true;
        }
        let depth = self.layers[from].depth;
        // Final-index semantics -> insertion gap in the current stack.
        let slot = if to > from { to + 1 } else { to };
        self.move_block_to_slot(from, slot, depth)
    }

    /// Move a layer one step up (towards the top of the stack).
    pub fn raise_layer(&mut self, index: usize) -> bool {
        index + 1 < self.layers.len() && self.move_layer(index, index + 1)
    }

    /// Move a layer one step down (towards the bottom).
    pub fn lower_layer(&mut self, index: usize) -> bool {
        index > 0 && self.move_layer(index, index - 1)
    }

    /// New layer above the active one, filled from an image, centred on the
    /// canvas (oversized images are clipped). Records one structural undo step like any
    /// structural layer op. Returns the new layer's index.
    pub fn add_layer_from_image(
        &mut self,
        name: impl Into<String>,
        img: &image::RgbaImage,
    ) -> usize {
        let size = self.size;
        let ox = (size.0 as i64 - img.width() as i64) / 2;
        let oy = (size.1 as i64 - img.height() as i64) / 2;
        self.add_layer_from_image_at(name, img, ox, oy)
    }

    /// The same, with the image's top-left corner named in canvas pixels
    /// instead of centred. I03's batch-placement replay needs it: a
    /// placement the artist made by hand on one page is a RECTANGLE, and
    /// centring is only the special case where that rectangle happens to
    /// sit in the middle.
    pub fn add_layer_from_image_at(
        &mut self,
        name: impl Into<String>,
        img: &image::RgbaImage,
        ox: i64,
        oy: i64,
    ) -> usize {
        let at = self.add_layer(name);
        let size = self.size;
        fill_layer_from_image_at(&mut self.layers[at], size, img, ox, oy);
        self.touch();
        at
    }

    /// IO-043: import with a selection active. The same layer
    /// [`Document::add_layer_from_image`] makes, plus the layer mask CSP
    /// builds for you — everything outside the selection is hidden, and
    /// nothing is destroyed doing it (delete the mask and the whole image
    /// is back). Returns the new layer's index and whether a mask was
    /// actually built, so the caller can say which of the two things it
    /// just did.
    ///
    /// **Why a mask and not a crop.** The gesture this serves is "select
    /// the panel, drop the photo in" — and the user is guessing at the
    /// crop. A mask is the guess he can take back; a crop is not, and the
    /// pixels outside a panel are exactly the ones he reaches for when the
    /// panel turns out to be the wrong size. This is also what CSP does,
    /// for the same reason.
    ///
    /// No selection ⇒ no mask, and emphatically not an all-hidden one:
    /// [`Document::mask_outside_selection`] masks EVERYTHING when
    /// `selection` is `None`, which as an import result would look exactly
    /// like a failed import.
    pub fn add_layer_from_image_masked(
        &mut self,
        name: impl Into<String>,
        img: &image::RgbaImage,
    ) -> (usize, bool) {
        let at = self.add_layer_from_image(name, img);
        let masked = self.selection.is_some() && self.mask_outside_selection(at);
        (at, masked)
    }

}
