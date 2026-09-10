use super::*;

impl Document {
    /// CSP-style frame border folder: a folder header carrying the frame
    /// vectors (its derived raster — white gutter + borders — masks the
    /// children), with a shared-white "White" layer at the bottom so art below
    /// the folder never shows through the panels, and an empty draw layer
    /// which becomes active. Pushed at the top of the stack. Clears the undo
    /// history. Returns the header's index.
    pub fn add_frame_folder(&mut self, name: impl Into<String>, frames: FrameSet) -> usize {
        self.add_frame_folder_with(name, frames, true)
    }

    /// Same, with CSP's "Fill inside the frame" choice: `fill_white = false`
    /// skips the shared-white base layer (art below shows through the panel).
    pub fn add_frame_folder_with(
        &mut self,
        name: impl Into<String>,
        frames: FrameSet,
        fill_white: bool,
    ) -> usize {
        let (before, active_before) = (self.stack_snapshot(), self.active);
        let mut draw = Layer::new("Layer 1");
        draw.depth = 1;
        let mut header = Layer::new(name);
        header.kind = LayerKind::Frame(frames);
        header.folder = true;
        Self::derive_frame_raster(&mut header, self.size);
        if fill_white {
            let mut white = Layer::new("White");
            white.depth = 1;
            white.fill_white(self.size);
            self.layers.push(white);
        }
        self.layers.push(draw);
        self.layers.push(header);
        // Draw layer active: the next pen stroke lands inside the folder.
        self.active = self.layers.len() - 2;
        self.record_structure("New frame folder", before, active_before);
        self.touch();
        self.layers.len() - 1
    }

    /// CSP "Divide frame folder": the folder at `index` keeps `keep` as its
    /// vectors, and a **new sibling frame folder** (with its own White + draw
    /// layer, like [`Self::add_frame_folder`]) is inserted beside the
    /// original's block carrying `split_off`. Both rasters re-derive. Clears
    /// the undo history (structural). Returns the new header's index, with the
    /// new folder's draw layer active.
    ///
    /// `above` puts the new block ABOVE the original's instead of below it.
    /// Owner 2026-09-05: "Frame 2 (the bottom one) is on the layer list higher
    /// than frame 1, which is counterintuitive" — the block always landed on
    /// one fixed side while the badges are numbered by READING order, so the
    /// two disagreed whenever the split-off half read first. Reading order is
    /// the app's (it needs the binding side), so the caller decides and this
    /// only places the block.
    pub fn divide_frame_folder(
        &mut self,
        index: usize,
        keep: FrameSet,
        split_off: FrameSet,
        above: bool,
    ) -> Option<usize> {
        let size = self.size;
        let (before, active_before) = (self.stack_snapshot(), self.active);
        let l = self.layers.get_mut(index)?;
        if !(l.folder && l.is_frame()) {
            return None;
        }
        let depth = l.depth;
        let LayerKind::Frame(cur) = &mut l.kind else {
            return None;
        };
        *cur = keep;
        Self::derive_frame_raster(l, size);

        let n = self.layers.iter().filter(|x| x.is_frame()).count() + 1;
        // Children sit BELOW their header, so the original's block is
        // `children_range(index).start ..= index`: below it is that start,
        // above it is one past the header.
        let at = if above {
            index + 1
        } else {
            self.children_range(index).start
        };
        let mut white = Layer::new("White");
        white.depth = depth + 1;
        white.fill_white(size);
        let mut draw = Layer::new("Layer 1");
        draw.depth = depth + 1;
        let mut header = Layer::new(format!("Frame {n}"));
        header.kind = LayerKind::Frame(split_off);
        header.folder = true;
        header.depth = depth;
        Self::derive_frame_raster(&mut header, size);
        // Children sit below their header in the flat encoding.
        self.layers.insert(at, header);
        self.layers.insert(at, draw);
        self.layers.insert(at, white);
        self.active = at + 1; // the new draw layer
        self.normalize_depths();
        self.record_structure("Divide frame folder", before, active_before);
        self.touch();
        Some(at + 2)
    }

    /// `FB-026` "Duplicate layer" — the other answer to *what happens to the
    /// art* when a panel with drawing in it is cut. Same structural move as
    /// [`Self::divide_frame_folder`], except the new folder gets a **copy of
    /// the original's contents** (the pixels ride along as `Arc` clones, so
    /// it is cheap until one of the two is painted on) instead of a fresh
    /// White + empty draw layer.
    ///
    /// Both halves then mask the same art to their own shape, which is the
    /// point: the artist cuts a drawn panel and keeps the drawing in both.
    /// Returns the new header's index; the copy's topmost child is active.
    /// `above` places the new block exactly as in [`Self::divide_frame_folder`].
    pub fn divide_frame_folder_dup(
        &mut self,
        index: usize,
        keep: FrameSet,
        split_off: FrameSet,
        above: bool,
    ) -> Option<usize> {
        let size = self.size;
        let l = self.layers.get(index)?;
        if !(l.folder && l.is_frame()) {
            return None;
        }
        let depth = l.depth;
        // Snapshot the contents BEFORE the header is rewritten.
        let mut block: Vec<Layer> = self.layers[self.children_range(index)].to_vec();
        if block.is_empty() {
            // Nothing to duplicate — the empty-folder answer IS the answer
            // (and it records its own structural undo step).
            return self.divide_frame_folder(index, keep, split_off, above);
        }
        let (before, active_before) = (self.stack_snapshot(), self.active);
        for c in &mut block {
            // A clone must never inherit an open op's recording.
            c.recording = None;
            // The originals stay in the source folder — new identities here.
            c.id = mint_id();
        }
        let l = self.layers.get_mut(index)?;
        let LayerKind::Frame(cur) = &mut l.kind else {
            return None;
        };
        *cur = keep;
        Self::derive_frame_raster(l, size);

        let n = self.layers.iter().filter(|x| x.is_frame()).count() + 1;
        let at = if above {
            index + 1
        } else {
            self.children_range(index).start
        };
        let mut header = Layer::new(format!("Frame {n}"));
        header.kind = LayerKind::Frame(split_off);
        header.folder = true;
        header.depth = depth;
        Self::derive_frame_raster(&mut header, size);
        let k = block.len();
        self.layers.insert(at, header);
        // Children sit below their header, in their original order.
        for (i, c) in block.into_iter().enumerate() {
            self.layers.insert(at + i, c);
        }
        self.active = at + k - 1; // the copy's topmost child
        self.normalize_depths();
        self.record_structure("Divide frame folder", before, active_before);
        self.touch();
        Some(at + k)
    }

    /// New frame (koma) layer at the **top** of the stack — frames sit above
    /// the art they mask — rasterized from `frames` and made active. Records
    /// one structural undo step. Returns the new index.
    pub fn add_frame_layer(&mut self, name: impl Into<String>, frames: FrameSet) -> usize {
        let (before, active_before) = (self.stack_snapshot(), self.active);
        let mut l = Layer::new(name);
        l.replace_tiles(frames.rasterize(self.size));
        l.kind = LayerKind::Frame(frames);
        self.layers.push(l);
        self.active = self.layers.len() - 1;
        self.record_structure("New frame layer", before, active_before);
        self.touch();
        self.active
    }

    /// Replace a frame layer's vector state, re-rasterize, and push a normal
    /// undo step (frame edits are undoable — they shift no layer indices).
    /// Returns `false` when `index` is not a frame layer.
    /// FB-035/036/038 (TRIAGE 141): combine two sibling frame folders.
    /// The UPPER folder's header survives; both folders' children pool
    /// under it; frames CONCAT (FB-036 "keep shapes", non-adjacent fine
    /// per FB-038) — or, with `merge_borders` and exactly one frame each
    /// sharing an edge, the two become ONE rect frame at the union bbox
    /// (FB-035 "combine border"; exact for the rect frames divides
    /// produce — a non-rect shape would need a real polygon union,
    /// recorded). Both frames' `slot` fields drop to None: the merged
    /// folder's reading order is decided by geometry alone (8.54).
    /// Returns the surviving header's index. Structural (clears history).
    pub fn combine_frame_folders(
        &mut self,
        a: usize,
        b: usize,
        merge_borders: bool,
    ) -> Option<usize> {
        let (ia, ib) = (a, b);
        if ia >= self.layers.len() || ib >= self.layers.len() || ia == ib {
            return None;
        }
        let (ha, hb) = (&self.layers[ia], &self.layers[ib]);
        if !(ha.folder && ha.is_frame() && hb.folder && hb.is_frame()) {
            return None;
        }
        if ha.depth != hb.depth {
            return None; // not siblings — nesting needs FB-037's own round
        }
        if self.enclosing_folder(ia) != self.enclosing_folder(ib) {
            return None; // same depth, DIFFERENT parents — not siblings
        }
        let (before, active_before) = (self.stack_snapshot(), self.active);
        // Audit 2026-08-21: the combine DESTROYS B's header, and everything
        // that renders at FOLDER level goes with it — the compositor reads
        // visibility/opacity/blend/through/draft off the header, and border
        // width, the ruler flag and the reading pin live on the `FrameSet`,
        // not on a `Frame`. None of it has a per-child home to move to (a
        // group blend is not a per-child blend, a hidden group is not a
        // hidden child, and a `Frame` carries no border of its own), so a
        // pair that disagrees refuses instead of silently repainting one
        // side's panels in the other's style. `slot` is exempt: it is
        // divide provenance, and clearing it is this op's documented job.
        let look = |l: &Layer| {
            (
                l.visible,
                l.through,
                l.draft,
                l.opacity.to_bits(),
                l.blend,
                l.frames()
                    .map(|f| (f.border_px.to_bits(), f.border_ruler, f.reading_pin)),
            )
        };
        // A layer mask is a canvas-space raster: it cannot be split between
        // the two, and the survivor's would start clipping the partner's ink.
        if look(ha) != look(hb) || ha.mask.is_some() || hb.mask.is_some() {
            return None;
        }
        let mut set_a = ha.frames()?.clone();
        let set_b = hb.frames()?.clone();
        if merge_borders && set_a.frames.len() == 1 && set_b.frames.len() == 1 {
            let (ra, rb) = (set_a.frames[0].bbox(), set_b.frames[0].bbox());
            let tol = 2.0;
            let share_x = (ra[2] - rb[0]).abs() <= tol || (rb[2] - ra[0]).abs() <= tol;
            let share_y = (ra[3] - rb[1]).abs() <= tol || (rb[3] - ra[1]).abs() <= tol;
            let overlap_x = ra[0] < rb[2] - tol && rb[0] < ra[2] - tol;
            let overlap_y = ra[1] < rb[3] - tol && rb[1] < ra[3] - tol;
            if (share_x && overlap_y) || (share_y && overlap_x) {
                let u = [
                    ra[0].min(rb[0]),
                    ra[1].min(rb[1]),
                    ra[2].max(rb[2]),
                    ra[3].max(rb[3]),
                ];
                set_a.frames = vec![crate::frame::Frame::rect(u[0], u[1], u[2], u[3])];
            } else {
                set_a.frames.extend(set_b.frames);
            }
        } else {
            set_a.frames.extend(set_b.frames);
        }
        set_a.slot = None;
        let ba = self.block_range(ia);
        let bb = self.block_range(ib);
        // Children of both blocks, then the surviving header.
        let n = (ia - ba.start) + (ib - bb.start) + 1;
        let mut block: Vec<Layer> = self.layers[ba.start..ia].to_vec(); // A's children
        block.extend(self.layers[bb.start..ib].iter().cloned()); // B's children
        let mut header = self.layers[ia].clone();
        header.kind = LayerKind::Frame(set_a);
        Self::derive_frame_raster(&mut header, self.size);
        block.push(header);
        // Remove the HIGHER block first so the lower indices stay valid.
        let (lo, hi) = if ba.start < bb.start {
            (ba, bb)
        } else {
            (bb, ba)
        };
        self.layers.drain(hi.clone());
        self.layers.splice(lo.start..lo.end, block);
        self.normalize_depths();
        // Agree with group_frame_folders_common_parent: the new folder's
        // header is the selection after a combine (both used to differ —
        // audit H, 2026-08-19).
        self.active = lo.start + n - 1;
        self.record_structure("Combine frame folders", before, active_before);
        self.touch();
        Some(lo.start + n - 1)
    }

    /// FB-037 (TRIAGE 141): wrap two sibling frame folders in a NEW
    /// COMMON PARENT — a plain folder at their depth; both blocks move
    /// one level deeper, headers and shapes untouched ("originals
    /// survive"). The non-destructive combine. Returns the new header.
    /// Structural (clears history).
    ///
    /// Non-adjacent siblings are legal: the LOWER block is spliced up,
    /// directly under the higher block's start, before the folder is
    /// inserted — a folder's children must be contiguous above its
    /// header, and CSP's grouping does the same (the selection moves to
    /// the highest selected position; the intervening layers stay put,
    /// below both). Audit E, 2026-08-19: the old guard refused every
    /// separated pair as "not siblings" and its insert position would
    /// have left the lower block outside the parent.
    pub fn group_frame_folders_common_parent(&mut self, a: usize, b: usize) -> Option<usize> {
        let (ia, ib) = (a, b);
        if ia >= self.layers.len() || ib >= self.layers.len() || ia == ib {
            return None;
        }
        let (ha, hb) = (&self.layers[ia], &self.layers[ib]);
        if !(ha.folder && ha.is_frame() && hb.folder && hb.is_frame()) {
            return None;
        }
        if ha.depth != hb.depth {
            return None;
        }
        if self.enclosing_folder(ia) != self.enclosing_folder(ib) {
            return None; // same depth, DIFFERENT parents — not siblings
        }
        let ba = self.block_range(ia);
        let bb = self.block_range(ib);
        let (lo, hi) = if ba.start < bb.start {
            (ba, bb)
        } else {
            (bb, ba)
        };
        // Same-depth blocks cannot overlap; refuse anything that would
        // interleave rather than guess (an invariant breach, not a layout).
        if lo.end > hi.start {
            return None;
        }
        let (before, active_before) = (self.stack_snapshot(), self.active);
        let parent_depth = ha.depth;
        // Splice the lower block up, adjacent to the higher block. When
        // the pair is already adjacent this is a no-op (insert_at ==
        // lo.start); otherwise the lower block crosses the intervening
        // layers, keeping its internal order.
        let moved: Vec<Layer> = self.layers.drain(lo.clone()).collect();
        let insert_at = hi.start - lo.len();
        self.layers.splice(insert_at..insert_at, moved);
        // One contiguous run now: deepen both blocks, parent below them.
        let run = insert_at..(insert_at + lo.len() + hi.len());
        for i in run.clone() {
            self.layers[i].depth += 1;
        }
        let mut header = Layer::new("Frames");
        header.folder = true;
        header.depth = parent_depth;
        // Children sit BELOW their header in the vec: the parent goes
        // AFTER the last layer of the second block.
        self.layers.insert(run.end, header);
        self.normalize_depths();
        self.active = run.end;
        self.record_structure("Group frame folders", before, active_before);
        self.touch();
        Some(run.end)
    }

    /// SF-004/005: re-rasterize the effect-line layer at `index` from
    /// `spec` — ONE undo step covering both the pixels and the parameters.
    /// Returns false, document untouched, when the layer carries no spec of
    /// its own or the render produced nothing (the caller's status message).
    ///
    /// The spec is an argument rather than a field read (it used to be
    /// stored first, then rendered) because both halves must go into the
    /// same group: undo that restored the old pixels while leaving the new
    /// parameters on the layer would make the Edit dialog describe art that
    /// is no longer there. The tiles are written through `set_tile` inside
    /// the op bracket — the old wholesale `replace_tiles` swap bypassed the
    /// copy-on-write recording, which is why the regen was un-undoable and
    /// had to purge the layer's history to stay consistent.
    pub fn regen_genlines(&mut self, index: usize, spec: crate::genlines::GenLinesSpec) -> bool {
        // Only a layer that was generated regenerates: this is the in-place
        // door, and pointing it at an ink layer would eat the drawing.
        let Some(before) = self.layers.get(index).and_then(|l| l.genlines) else {
            return false;
        };
        let tiles = spec.render(self.size);
        if tiles.is_empty() {
            return false;
        }
        // An op left open belongs to whatever gesture came before; close it
        // rather than nest (same rule `undo` follows), or `begin_op_on`
        // would no-op and these writes would go unrecorded.
        if self.op_layer.is_some() {
            self.end_op();
        }
        self.begin_op_on(index);
        self.set_op_label("Regenerate lines");
        // Wholesale replacement, tile by tile: every index the old raster
        // covered and the new one does not has to be cleared, or the
        // previous lines survive around the new ones.
        let stale: Vec<TileIdx> = self.layers[index]
            .tiles()
            .map(|(i, _)| i)
            .filter(|i| !tiles.contains_key(i))
            .collect();
        for idx in stale {
            self.layers[index].set_tile(idx, None);
        }
        for (idx, tile) in tiles {
            self.layers[index].set_tile(idx, Some(tile));
        }
        self.layers[index].genlines = Some(spec);
        if let Some((li, label, tiles)) = self.take_op() {
            self.history.push_labeled(
                &label,
                UndoGroup::GenLines {
                    layer: li,
                    spec: before,
                    tiles,
                },
            );
        }
        self.touch();
        true
    }

    pub fn set_frames(&mut self, index: usize, frames: FrameSet) -> bool {
        let size = self.size;
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        let LayerKind::Frame(cur) = &mut l.kind else {
            return false;
        };
        let before = cur.clone();
        *cur = frames;
        Self::derive_frame_raster(l, size);
        self.history.push_labeled(
            "Frame",
            UndoGroup::Frames {
                layer: index,
                frames: before,
            },
        );
        self.touch();
        true
    }

}
