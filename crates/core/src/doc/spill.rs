use super::*;

impl Document {
    /// The nearest frame folder ABOVE `index` (never `index` itself).
    pub fn enclosing_frame_folder(&self, index: usize) -> Option<usize> {
        let mut i = index;
        while let Some(f) = self.enclosing_folder(i) {
            if self.layers[f].is_frame() {
                return Some(f);
            }
            i = f;
        }
        None
    }

    /// The breakout layer at `index` re-seated: the composite step it is
    /// emitted immediately AFTER. `None` = not a breakout layer (the walk is
    /// the identity for it).
    ///
    /// The default seat is the layer's own frame folder header — today's
    /// shipped behaviour, and the floor: `draws_over` can only push the seat
    /// UP. Every covered layer is lifted out of any sealed folder the
    /// escapee's own seat is not already inside (see [`Self::lift_seat`]),
    /// then the highest of them wins.
    pub fn spill_anchor(&self, index: usize) -> Option<usize> {
        let l = self.layers.get(index)?;
        if l.folder || !l.escape_frame {
            return None;
        }
        let ff = self.enclosing_frame_folder(index)?;
        if self.layers[ff].through {
            return None; // a through frame folder never clipped it anyway
        }
        let mut seat = ff;
        if !l.draws_over.is_empty() {
            for (j, other) in self.layers.iter().enumerate().skip(ff + 1) {
                // At or below the header is already covered by the default
                // seat, so only the layers above it can move anything.
                if l.draws_over.contains(&other.id) {
                    seat = seat.max(self.lift_seat(j, ff));
                }
            }
        }
        Some(seat)
    }

    /// Lift a covered layer out of every SEALED folder that does not also
    /// enclose the escapee's own seat: inside one, the escapee would join
    /// that group's isolation — and, for a frame folder, be clipped by the
    /// very panel mask it is trying to spill over. Hopping to the folder's
    /// header instead draws it over the whole finished group, which is what
    /// "draws over the art in that panel" has to mean. A Through folder has
    /// no seal to escape, so it is walked past without moving the seat.
    fn lift_seat(&self, target: usize, ff: usize) -> usize {
        // The folders enclosing the escapee's own seat: the escapee already
        // composites inside these, so they are not walls.
        let mut open: Vec<usize> = Vec::new();
        let mut p = ff;
        while let Some(f) = self.enclosing_folder(p) {
            open.push(f);
            p = f;
        }
        let mut seat = target;
        let mut probe = target;
        while let Some(f) = self.enclosing_folder(probe) {
            if open.contains(&f) {
                break;
            }
            if !self.layers[f].through {
                seat = f;
            }
            probe = f;
        }
        seat
    }

    /// The order both compositors walk the stack in, each step with its
    /// EFFECTIVE depth and which half of a mask-capped spill it draws.
    /// Identity except FB-overflow: a non-folder layer with `escape_frame`
    /// inside a SEALED frame folder re-seats immediately after its
    /// [`Self::spill_anchor`], at that anchor's own depth — so its ink lands
    /// in the accumulator the walk has open right there, above the panel
    /// mask and the border ink. Escapees keep their stack order. A through
    /// frame folder never clips, so its escapees stay in place.
    ///
    /// **The mask cap** (part 2, item 1): when a breakout layer carries an
    /// ENABLED layer mask, the mask stops being an alpha mask and becomes
    /// the allowed breakout REGION, so the layer composites TWICE — the
    /// masked-in part ([`SpillPart::Out`]) at the escaped seat, the rest
    /// ([`SpillPart::In`]) at the layer's own seat where the panel still
    /// clips it. The two halves are exact complements (`m` and `1 − m`), so
    /// a half-opacity layer does not double-blend anywhere.
    ///
    /// Both compositors (export.rs and gpu) MUST walk this, not
    /// `self.layers` — a disagreement here is a CPU/GPU parity break.
    pub fn composite_order(&self) -> Vec<CompositeStep> {
        let mut order: Vec<CompositeStep> = Vec::with_capacity(self.layers.len());
        // (anchor, escapee, mask-capped)
        let mut pending: Vec<(usize, usize, bool)> = Vec::new();
        for (li, l) in self.layers.iter().enumerate() {
            match self.spill_anchor(li) {
                Some(anchor) => {
                    let capped = l.breakout_mask().is_some();
                    if capped {
                        // The half the mask holds IN stays exactly where it
                        // always was, panel clip and all.
                        order.push(CompositeStep::new(li, l.depth, SpillPart::In));
                    }
                    pending.push((anchor, li, capped));
                    // No release here: anything anchored to THIS layer waits
                    // for its escaped seat below.
                    continue;
                }
                None => order.push(CompositeStep::new(li, l.depth, SpillPart::All)),
            }
            // Children walk before their header, so everything stashed for
            // this anchor bursts out right after it — folder header or
            // ordinary layer alike.
            Self::release_spills(&mut order, &mut pending, li, l.depth);
        }
        // Unreachable orphans (a stale anchor outrunning a structure edit, or
        // two escapees anchored to each other): walk them in place rather
        // than dropping their art.
        for (_, e, capped) in pending {
            let part = if capped { SpillPart::Out } else { SpillPart::All };
            order.push(CompositeStep::new(e, self.layers[e].depth, part));
        }
        order
    }

    /// Emit every spill anchored at `anchor` (and, transitively, everything
    /// anchored at those), all at the anchor's own effective depth.
    fn release_spills(
        order: &mut Vec<CompositeStep>,
        pending: &mut Vec<(usize, usize, bool)>,
        anchor: usize,
        depth: u8,
    ) {
        let mut queue = std::collections::VecDeque::from([anchor]);
        while let Some(a) = queue.pop_front() {
            let mut k = 0;
            while k < pending.len() {
                if pending[k].0 == a {
                    let (_, e, capped) = pending.remove(k);
                    let part = if capped { SpillPart::Out } else { SpillPart::All };
                    order.push(CompositeStep::new(e, depth, part));
                    queue.push_back(e);
                } else {
                    k += 1;
                }
            }
        }
    }

    /// Every layer the breakout at `index` could be told to draw over: the
    /// stack ABOVE its frame folder header (bottom-first, like `layers`).
    /// Anything at or below that header is covered by the default seat, so
    /// offering it would be a switch that does nothing.
    pub fn spill_candidates(&self, index: usize) -> Vec<usize> {
        match self.spill_anchor(index) {
            Some(_) => {
                let ff = self.enclosing_frame_folder(index).unwrap_or(0);
                ((ff + 1)..self.layers.len()).filter(|&j| j != index).collect()
            }
            None => Vec::new(),
        }
    }

    /// The topmost layer the breakout at `index` currently draws over, i.e.
    /// where the insertion marker sits. `None` = the default seat.
    pub fn spill_seat(&self, index: usize) -> Option<usize> {
        let l = self.layers.get(index)?;
        if l.draws_over.is_empty() {
            return None;
        }
        let ff = self.enclosing_frame_folder(index)?;
        ((ff + 1)..self.layers.len())
            .filter(|&j| l.draws_over.contains(&self.layers[j].id))
            .next_back()
    }

    /// The cascade made explicit: "draws over the layer at `top`" means
    /// "draws over everything from the frame folder header up to `top`",
    /// because paint order is a stack and there is no drawing a layer twice.
    /// `None` = back to the default seat (the empty set).
    pub fn draws_over_cascade(&self, index: usize, top: Option<usize>) -> BTreeSet<u64> {
        let (Some(top), Some(ff)) = (top, self.enclosing_frame_folder(index)) else {
            return BTreeSet::new();
        };
        ((ff + 1)..=top.min(self.layers.len().saturating_sub(1)))
            .filter(|&j| j != index)
            .map(|j| self.layers[j].id)
            .collect()
    }

    /// Move the breakout layer's insertion marker (item 3's one control).
    /// Refuses anything that is not a breakout layer, so a stale set can
    /// never be written onto a layer that would silently ignore it. One
    /// undo press: the Structure snapshot carries the whole stack, and the
    /// set lives on `Layer`.
    pub fn set_layer_spill_seat(&mut self, index: usize, top: Option<usize>) -> bool {
        if self.spill_anchor(index).is_none() {
            return false;
        }
        let want = self.draws_over_cascade(index, top);
        if self.layers[index].draws_over == want {
            return false;
        }
        let before = self.stack_snapshot();
        let active_before = self.active;
        self.layers[index].draws_over = want;
        self.record_structure("Breakout draws over", before, active_before);
        self.touch();
        true
    }

    /// `draws_over` with every dead id dropped — the SAVE-TIME prune the
    /// stable-id mint's doc comment owes. A layer deleted in one session
    /// frees its id; the mint restarts next session and `ensure_ids` can
    /// hand that number to somebody else, so a cross-reference to it must
    /// never reach the file.
    pub fn live_draws_over(&self, index: usize) -> BTreeSet<u64> {
        let live: std::collections::HashSet<u64> = self.layers.iter().map(|l| l.id).collect();
        self.layers
            .get(index)
            .map(|l| l.draws_over.iter().copied().filter(|id| live.contains(id)).collect())
            .unwrap_or_default()
    }

    /// Per-layer draft state with ancestor folders folded in — a draft
    /// folder drafts everything inside it (mirrors
    /// [`Self::effective_visibility`]).
    pub fn effective_drafts(&self) -> Vec<bool> {
        let mut out: Vec<bool> = self.layers.iter().map(|l| l.draft).collect();
        for i in 0..self.layers.len() {
            if self.layers[i].folder && self.layers[i].draft {
                for j in self.children_range(i) {
                    out[j] = true;
                }
            }
        }
        out
    }

}
