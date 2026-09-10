use super::*;

impl Document {
    // ---------------------------------------------------------------- undo --

    /// Open an undo op on the **active** layer.
    ///
    /// Every `Layer::tile_mut` between here and `end_op` snapshots its
    /// pre-image, so a `StrokeSink` becomes undoable without knowing undo
    /// exists. Calling `begin_op` while an op is already open does nothing (the
    /// existing recording keeps accumulating) — a stroke is never split in two.
    ///
    /// Note the recording is armed on the layer that is active *now*; switching
    /// layers mid-op keeps recording into the original one.
    pub fn begin_op(&mut self) {
        let li = self.active.min(self.layers.len().saturating_sub(1));
        self.begin_op_on(li);
    }

    /// Open an undo op on a SPECIFIC layer. The Object tool's folder move
    /// records each child in turn — children are almost never the active
    /// layer, and `begin_op` recording "whichever layer happened to be
    /// active" is exactly how a folder drag became un-undoable art loss.
    /// Same no-nesting rule as `begin_op`.
    pub fn begin_op_on(&mut self, li: usize) {
        if self.op_layer.is_some() || li >= self.layers.len() {
            return;
        }
        self.layers[li].arm_recording();
        self.op_layer = Some(li);
    }

    /// Close the open op and push it onto the undo stack.
    ///
    /// Returns `true` when a group was actually pushed — an op that touched no
    /// tiles (pen-down/pen-up with no movement inside the canvas) pushes
    /// nothing, so undo never eats a no-op.
    pub fn end_op(&mut self) -> bool {
        let Some((li, label, tiles)) = self.take_op() else {
            return false;
        };
        self.history
            .push_labeled(&label, UndoGroup::Tiles { layer: li, tiles });
        self.touch();
        true
    }

    /// Close the open op and hand back its `Tiles` group WITHOUT pushing:
    /// a multi-layer loop (`apply_adjust_many`) collects one per layer and
    /// pushes them as ONE step via [`Self::push_compound`]. `None` when no
    /// op was open or nothing was touched.
    pub fn end_op_take(&mut self) -> Option<UndoGroup> {
        let (li, _label, tiles) = self.take_op()?;
        Some(UndoGroup::Tiles { layer: li, tiles })
    }

    /// Push already-collected groups as ONE labelled undo step. A single
    /// member skips the `Compound` wrapper (identical history to the op
    /// having pushed itself). Index drift cannot bite for the same reason
    /// `set_tone_many` is safe: every index-shifting op clears the history.
    pub fn push_compound(&mut self, label: &str, mut members: Vec<UndoGroup>) -> bool {
        match members.len() {
            0 => return false,
            1 => self.history.push_labeled(label, members.pop().unwrap()),
            _ => self
                .history
                .push_labeled(label, UndoGroup::Compound(members)),
        }
        self.touch();
        true
    }

    /// Bundle the newest `n` history steps into ONE labelled step
    /// (recordable action runs, whole-work reflows). Members are collected
    /// newest-first, which IS the swap order `Compound` needs (undo unwinds
    /// the run backwards; the reversed inverse replays it forward on redo).
    /// Structural members are fine: each is a Structure swap, and the LIFO
    /// argument on `UndoGroup`'s doc comment holds inside a Compound too.
    pub fn wrap_recent(&mut self, label: &str, n: usize) -> bool {
        if n == 0 {
            return false;
        }
        let mut members = Vec::with_capacity(n);
        for _ in 0..n {
            match self.history.pop_undo() {
                Some(g) => members.push(g),
                None => break,
            }
        }
        if members.is_empty() {
            return false;
        }
        // `push_compound` unwraps a single member — a one-step run reads
        // in the History palette exactly like the step itself would.
        self.push_compound(label, members)
    }

    /// The layer stack as an undo pre-image: `Arc`-cheap clones with any
    /// open op's recording scrubbed (a snapshot must never inherit one —
    /// the same rule `duplicate_layer` follows). Taken at the TOP of every
    /// structural op, before the first mutation.
    ///
    /// Public because the app records structure too: a live-layer parameter
    /// edit and the correction dialog both snapshot the stack before they
    /// touch it, and both need the same scrub.
    pub fn stack_snapshot(&self) -> Vec<Layer> {
        let mut v = self.layers.clone();
        for l in &mut v {
            l.recording = None;
        }
        v
    }

    /// Record a structural op (add/remove/duplicate/move/merge/divide/
    /// combine) as ONE undoable step: the caller took [`Self::stack_snapshot`]
    /// and noted `active` BEFORE mutating, and calls this on the success
    /// path. This replaced the old clear-the-history model (2026-08-21):
    /// undo is LIFO, so an index-carrying group deeper in the stack is only
    /// ever swapped once the Structure swaps above it have restored the
    /// exact stack it was recorded against — indices cannot go stale as
    /// long as every structural change records one of these.
    pub fn record_structure(&mut self, label: &str, before: Vec<Layer>, active_before: usize) {
        self.cancel_op();
        // The palette multi-selection is index-keyed; a structural shift is
        // exactly what it must not survive. Cheap to rebuild, so clearing
        // stays the safe move even though the history now survives.
        self.layer_multi.clear();
        self.history.push_labeled(
            label,
            UndoGroup::Structure {
                layers: before,
                active: active_before,
            },
        );
    }

    /// Recordable actions: make a whole replayed run ONE undo press. The
    /// caller cloned `layers` and noted `active` BEFORE replaying; whatever
    /// the run pushed or cleared since is superseded by the snapshot pair
    /// (pre-run stack in the group, post-run stack live), so the history is
    /// cleared first and this group lands alone.
    pub fn push_structure(&mut self, label: &str, before: Vec<Layer>, active_before: usize) {
        self.clear_history();
        self.history.push_labeled(
            label,
            UndoGroup::Structure {
                layers: before,
                active: active_before,
            },
        );
        self.touch();
    }

    /// True when `g` is, or contains, a [`UndoGroup::Structure`] — a
    /// Compound from a recorded action run can carry one in its belly, and
    /// the cache-invalidation door below must see through the wrapper.
    fn group_is_structural(g: &UndoGroup) -> bool {
        match g {
            UndoGroup::Structure { .. } => true,
            UndoGroup::Compound(members) => members.iter().any(Self::group_is_structural),
            _ => false,
        }
    }

    /// True when the next undo would move a [`UndoGroup::Structure`]
    /// (possibly inside a Compound) — the app must fully invalidate the GPU
    /// tile cache around that swap (restored tiles keep their old, lower
    /// revisions; the cache uploads only on newer).
    pub fn next_undo_is_structure(&self) -> bool {
        self.history
            .peek_undo()
            .is_some_and(Self::group_is_structural)
    }

    /// Same door, redo side.
    pub fn next_redo_is_structure(&self) -> bool {
        self.history
            .peek_redo()
            .is_some_and(Self::group_is_structural)
    }

    /// Vector inking (docs/VECTOR-INKING.md): close the open op as ONE
    /// stroke group — the tile pre-images AND the recorded geometry, so a
    /// single undo takes back both. An empty gesture spends nothing; a
    /// layer that turns out not to record degrades to plain [`Self::end_op`]
    /// semantics.
    pub fn end_op_vector_stroke(&mut self, stroke: crate::stroke_set::VectorStroke) -> bool {
        let Some((li, label, tiles)) = self.take_op() else {
            return false;
        };
        match self.layers.get_mut(li).and_then(|l| l.strokes.as_mut()) {
            Some(set) => set.strokes.push(stroke.clone()),
            None => {
                self.history
                    .push_labeled(&label, UndoGroup::Tiles { layer: li, tiles });
                self.touch();
                return true;
            }
        }
        self.history.push_labeled(
            &label,
            UndoGroup::VectorStroke {
                layer: li,
                tiles,
                stroke: Box::new(stroke),
                present: true,
            },
        );
        self.touch();
        true
    }

    /// Vector inking phase 3: close the open op as ONE set-restructuring
    /// group (trim eraser, stroke delete) — the re-derived tiles' pre-images
    /// plus the whole set as it was BEFORE.
    pub fn end_op_vector_set(&mut self, before: crate::stroke_set::StrokeSet, label: &str) -> bool {
        let Some((li, _label, tiles)) = self.take_op() else {
            return false;
        };
        self.history.push_labeled(
            label,
            UndoGroup::VectorSet {
                layer: li,
                tiles,
                strokes: before,
            },
        );
        self.touch();
        true
    }

    /// Vector inking phase 2: close the open op as ONE stroke-edit group —
    /// the re-derived tiles' pre-images plus the stroke as it was BEFORE
    /// the edit (`strokes[index]` must already hold the edited version).
    pub fn end_op_vector_edit(
        &mut self,
        index: usize,
        before: crate::stroke_set::VectorStroke,
        label: &str,
    ) -> bool {
        let Some((li, _label, tiles)) = self.take_op() else {
            return false;
        };
        self.history.push_labeled(
            label,
            UndoGroup::VectorEdit {
                layer: li,
                tiles,
                index,
                stroke: Box::new(before),
            },
        );
        self.touch();
        true
    }

    /// Close the open op and hand back its layer, label and sorted
    /// pre-images WITHOUT pushing a group — the shared half of `end_op`,
    /// for the ops that wrap the same recording in a richer group (the
    /// effect-line regen, whose spec rides the pixels). `None` when no op
    /// was open or nothing was touched.
    #[allow(clippy::type_complexity)]
    pub(super) fn take_op(&mut self) -> Option<(usize, String, Vec<(TileIdx, Option<Arc<Tile>>)>)> {
        let li = self.op_layer.take()?;
        let rec = self.layers.get_mut(li).and_then(Layer::take_recording)?;
        if rec.is_empty() {
            return None;
        }
        let mut tiles: Vec<(TileIdx, Option<Arc<Tile>>)> = rec.into_iter().collect();
        // HashMap order is not deterministic; groups are compared in tests and
        // replayed in order, so sort them.
        tiles.sort_by_key(|(idx, _)| (idx.y, idx.x));
        let label = self
            .pending_op_label
            .take()
            .unwrap_or_else(|| "Edit".into());
        Some((li, label, tiles))
    }

    /// Close the open op by RESTORING every pre-image — as if the op never
    /// happened, and no step is spent. (The vector eraser that touched no
    /// stroke reverts its live raster erase this way.)
    pub fn abort_op_restore(&mut self) {
        if let Some((li, _label, tiles)) = self.take_op()
            && let Some(l) = self.layers.get_mut(li)
        {
            for (idx, snap) in tiles {
                l.set_tile(idx, snap);
            }
        }
    }

    /// Drop the open op's recording **without** restoring anything. The pixels
    /// stay; they just stop being undoable. Used when the history is discarded.
    pub fn cancel_op(&mut self) {
        if let Some(li) = self.op_layer.take() {
            if let Some(l) = self.layers.get_mut(li) {
                l.take_recording();
            }
        }
    }

    pub fn is_op_open(&self) -> bool {
        self.op_layer.is_some()
    }

    /// Restore the newest undo group. Returns `false` when there is nothing to
    /// undo. An op left open is closed first, so ctrl-Z mid-stroke is safe.
    pub fn undo(&mut self) -> bool {
        if self.op_layer.is_some() {
            self.end_op();
        }
        let Some((label, group)) = self.history.pop_undo_labeled() else {
            return false;
        };
        match self.swap_group(group) {
            Some(inverse) => {
                self.history.push_redo_labeled(&label, inverse);
                self.touch();
                true
            }
            None => false,
        }
    }

    /// Re-apply the newest undone group. Returns `false` when there is nothing
    /// to redo.
    pub fn redo(&mut self) -> bool {
        if self.op_layer.is_some() {
            self.end_op();
        }
        let Some((label, group)) = self.history.pop_redo_labeled() else {
            return false;
        };
        match self.swap_group(group) {
            Some(inverse) => {
                self.history.push_undo_keep_redo_labeled(&label, inverse);
                self.touch();
                true
            }
            None => false,
        }
    }

    /// Swap a group's state into the document, returning the group that undoes
    /// the swap (i.e. the state that was there a moment ago).
    ///
    /// `Layer::set_tile` stamps a fresh revision on every restored tile, so the
    /// GPU cache re-uploads them; that is why the revision counter is global.
    /// Frame groups swap the vector state and re-rasterize — the fresh tiles
    /// carry fresh revisions for the same reason.
    fn swap_group(&mut self, group: UndoGroup) -> Option<UndoGroup> {
        match group {
            UndoGroup::Tiles { layer, tiles } => {
                let l = self.layers.get_mut(layer)?;
                let mut inverse = Vec::with_capacity(tiles.len());
                for (idx, snapshot) in tiles {
                    inverse.push((idx, l.tile_arc(idx).cloned()));
                    l.set_tile(idx, snapshot);
                }
                Some(UndoGroup::Tiles {
                    layer,
                    tiles: inverse,
                })
            }
            UndoGroup::GenLines { layer, spec, tiles } => {
                let l = self.layers.get_mut(layer)?;
                // Parameters and pixels swap together — see the group's
                // doc comment. Same tile door as `Tiles`, so restored tiles
                // get fresh revisions and the compositor re-uploads them.
                // The layer always has one here (only generated layers
                // regenerate); the fallback keeps the group's own spec
                // rather than inventing state if that ever changes.
                let spec_before = l.genlines.unwrap_or(spec);
                l.genlines = Some(spec);
                let mut inverse = Vec::with_capacity(tiles.len());
                for (idx, snapshot) in tiles {
                    inverse.push((idx, l.tile_arc(idx).cloned()));
                    l.set_tile(idx, snapshot);
                }
                Some(UndoGroup::GenLines {
                    layer,
                    spec: spec_before,
                    tiles: inverse,
                })
            }
            UndoGroup::VectorStroke {
                layer,
                tiles,
                stroke,
                present,
            } => {
                let l = self.layers.get_mut(layer)?;
                // Pixels and record swap together — the group's doc comment.
                // Same tile door as `Tiles` (fresh revisions, compositor
                // re-uploads).
                let mut inverse = Vec::with_capacity(tiles.len());
                for (idx, snapshot) in tiles {
                    inverse.push((idx, l.tile_arc(idx).cloned()));
                    l.set_tile(idx, snapshot);
                }
                if let Some(set) = &mut l.strokes {
                    if present {
                        // The recorded stroke is the set's newest; take it
                        // back with the ink.
                        set.strokes.pop();
                    } else {
                        set.strokes.push((*stroke).clone());
                    }
                }
                Some(UndoGroup::VectorStroke {
                    layer,
                    tiles: inverse,
                    stroke,
                    present: !present,
                })
            }
            UndoGroup::VectorEdit {
                layer,
                tiles,
                index,
                mut stroke,
            } => {
                let l = self.layers.get_mut(layer)?;
                let mut inverse = Vec::with_capacity(tiles.len());
                for (idx, snapshot) in tiles {
                    inverse.push((idx, l.tile_arc(idx).cloned()));
                    l.set_tile(idx, snapshot);
                }
                if let Some(s) = l
                    .strokes
                    .as_mut()
                    .and_then(|set| set.strokes.get_mut(index))
                {
                    std::mem::swap(s, &mut stroke);
                }
                Some(UndoGroup::VectorEdit {
                    layer,
                    tiles: inverse,
                    index,
                    stroke,
                })
            }
            UndoGroup::VectorSet {
                layer,
                tiles,
                strokes,
            } => {
                let l = self.layers.get_mut(layer)?;
                let mut inverse = Vec::with_capacity(tiles.len());
                for (idx, snapshot) in tiles {
                    inverse.push((idx, l.tile_arc(idx).cloned()));
                    l.set_tile(idx, snapshot);
                }
                let strokes_before = match &mut l.strokes {
                    Some(set) => std::mem::replace(set, strokes),
                    None => strokes,
                };
                Some(UndoGroup::VectorSet {
                    layer,
                    tiles: inverse,
                    strokes: strokes_before,
                })
            }
            UndoGroup::Frames { layer, frames } => {
                let size = self.size;
                let l = self.layers.get_mut(layer)?;
                let LayerKind::Frame(cur) = &mut l.kind else {
                    return None;
                };
                let inverse = UndoGroup::Frames {
                    layer,
                    frames: cur.clone(),
                };
                *cur = frames;
                Self::derive_frame_raster(l, size);
                Some(inverse)
            }
            // Item P: the two undo groups still carry HALF a speech layer
            // each, so undoing a balloon edit leaves the words on the layer
            // untouched — the raster is re-derived from both halves.
            UndoGroup::Balloons { layer, balloons } => {
                let size = self.size;
                let l = self.layers.get_mut(layer)?;
                let LayerKind::Speech(sp) = &mut l.kind else {
                    return None;
                };
                let inverse = UndoGroup::Balloons {
                    layer,
                    balloons: std::mem::replace(&mut sp.balloons, balloons),
                };
                let raster = sp.rasterize(size);
                l.replace_tiles(raster);
                Some(inverse)
            }
            UndoGroup::Texts { layer, texts } => {
                let size = self.size;
                let l = self.layers.get_mut(layer)?;
                let LayerKind::Speech(sp) = &mut l.kind else {
                    return None;
                };
                let inverse = UndoGroup::Texts {
                    layer,
                    texts: std::mem::replace(&mut sp.texts, texts),
                };
                let raster = sp.rasterize(size);
                l.replace_tiles(raster);
                Some(inverse)
            }
            UndoGroup::Mask { layer, mask } => {
                let l = self.layers.get_mut(layer)?;
                let inverse = UndoGroup::Mask {
                    layer,
                    mask: l.mask.clone(),
                };
                l.mask = mask;
                if let Some(m) = l.mask.as_mut() {
                    m.revision = crate::tile::next_revision();
                }
                Some(inverse)
            }
            UndoGroup::Tones { layer, tone } => {
                let l = self.layers.get_mut(layer)?;
                let inverse = UndoGroup::Tones {
                    layer,
                    tone: l.tone,
                };
                l.tone = tone;
                l.tone_tiles = None;
                // The border effect grows around the tone raster, so a tone
                // undo invalidates it too.
                l.edge_tiles = None;
                l.edge_stamp = None;
                Some(inverse)
            }
            UndoGroup::Edges { layer, edge } => {
                let l = self.layers.get_mut(layer)?;
                let inverse = UndoGroup::Edges {
                    layer,
                    edge: l.edge,
                };
                l.edge = edge;
                l.edge_tiles = None;
                l.edge_stamp = None;
                Some(inverse)
            }
            UndoGroup::Paper { colour } => {
                let inverse = UndoGroup::Paper {
                    colour: self.paper.colour,
                };
                self.paper.colour = colour;
                Some(inverse)
            }
            UndoGroup::Rulers { rulers } => {
                let inverse = UndoGroup::Rulers {
                    rulers: self.rulers.clone(),
                };
                self.rulers = rulers;
                Some(inverse)
            }
            UndoGroup::Compound(groups) => {
                // Members swap in order; the inverse carries them REVERSED
                // so redo replays forward. Members cannot fail here for the
                // same reason any group's layer lookup cannot: a structure
                // change would have cleared the history.
                let mut inverses = Vec::with_capacity(groups.len());
                for g in groups {
                    inverses.push(self.swap_group(g)?);
                }
                inverses.reverse();
                Some(UndoGroup::Compound(inverses))
            }
            UndoGroup::Structure { mut layers, active } => {
                // Wholesale stack swap. NO tile revisions are stamped (see
                // the variant's doc comment — the app invalidates the GPU
                // cache when this group moves).
                std::mem::swap(&mut self.layers, &mut layers);
                let active_before = self.active;
                self.active = active.min(self.layers.len().saturating_sub(1));
                // The multi-selection is index-keyed; a stack swap is
                // exactly the shift it must not survive.
                self.layer_multi.clear();
                Some(UndoGroup::Structure {
                    layers,
                    active: active_before,
                })
            }
        }
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// The label of the step the next undo would take (`None` when there
    /// is nothing to undo) — the leak-repair arm's "is the fill still the
    /// newest step?" check.
    pub fn peek_undo_label(&self) -> Option<&str> {
        self.history.peek_undo_label()
    }

    pub fn undo_len(&self) -> usize {
        self.history.undo_len()
    }

    pub fn redo_len(&self) -> usize {
        self.history.redo_len()
    }

    /// PR-041: undoable operations performed on this document, ever —
    /// monotonic, and unaffected by the depth cap or by `clear_history`.
    /// The edge the "save recovery data for every operation" preference
    /// fires on; see `undo::History`'s `ops` field for why neither
    /// `undo_len` nor `revision` would do.
    pub fn op_count(&self) -> u64 {
        self.history.ops()
    }

    /// How many undo groups this document keeps (the `undo_depth`
    /// preference; [`crate::undo::UNDO_LIMIT`] until something sets it).
    pub fn undo_limit(&self) -> usize {
        self.history.limit()
    }

    /// Apply the `undo_depth` preference. Trims immediately when lowered.
    pub fn set_undo_limit(&mut self, limit: usize) {
        self.history.set_limit(limit);
    }

    /// Throw the history away (file load, or a change undo cannot express —
    /// today that means `resize_to`; the other structural ops record a
    /// [`UndoGroup::Structure`] instead, see `record_structure`).
    pub fn clear_history(&mut self) {
        self.cancel_op();
        // The palette multi-selection is index-keyed; everything that
        // shifts indices clears it (see `layer_multi`'s note).
        self.layer_multi.clear();
        self.history.clear();
        // PR-041: a change that comes through here pushes no group, so
        // counting only pushes would leave exactly the unrecoverable
        // changes uncounted. `clear` deliberately does not reset the tally.
        self.history.note_op();
    }

}
