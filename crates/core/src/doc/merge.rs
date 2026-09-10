use super::*;

impl Document {
    /// Merge layer `index` down into `index - 1`, honouring the upper layer's
    /// blend mode and opacity (CSP "Merge with layer below"). A hidden upper
    /// layer contributes nothing and is simply removed. Clears the undo
    /// history like other structural ops. Returns `false` on a bad index.
    /// Vector layers (frames, balloons) refuse to merge — baking the derived
    /// raster into art would destroy both the vectors and the pixels under it.
    ///
    /// A FOLDER merges down too (CSP: the group flattens onto the layer
    /// under it — the everyday "collapse this shading folder into the
    /// flats"): its isolated composite is baked with the header's own
    /// opacity and blend, then the whole block leaves. A CLIPPED layer
    /// bakes what it SHOWS — its ink cut to the base's alpha — when the
    /// layer under it is that base (CSP's merge); when the layer under it
    /// is another member of the same clip run the ink lands as-is and
    /// stays clipped through it.
    ///
    /// The "layer below" is the row under the whole BLOCK (a folder's
    /// children sit below its header in `layers`), see
    /// [`Self::merge_down_refusal`] for every rule.
    pub fn merge_down(&mut self, index: usize) -> bool {
        if self.merge_down_refusal(index).is_some() {
            return false;
        }
        let block = self.block_range(index);
        let below = block.start - 1;
        let (before, active_before) = (self.stack_snapshot(), self.active);
        let mut upper = if self.layers[index].folder {
            self.flatten_block(block.clone())
        } else {
            self.layers[index].clone()
        };
        if upper.clip && self.clip_bases()[index] == Some(below) {
            let base = &self.layers[below];
            let keys: Vec<TileIdx> = upper.tiles.keys().copied().collect();
            for idx in keys {
                let Some(b) = base.display_tile(idx) else {
                    upper.tiles.remove(&idx);
                    continue;
                };
                let bd = b.data();
                let ud = Arc::make_mut(upper.tiles.get_mut(&idx).expect("own key")).data_mut();
                for p in 0..crate::tile::TILE_PIXELS {
                    let a = bd[p * 4 + 3] as u32;
                    for c in 0..4 {
                        ud[p * 4 + c] =
                            ((ud[p * 4 + c] as u32 * a) / crate::tile::FIX15_ONE as u32) as u16;
                    }
                }
            }
        }
        bake_layer_into(&mut self.layers[below], &upper);
        self.layers.drain(block);
        self.active = below;
        self.normalize_depths();
        self.record_structure("Merge down", before, active_before);
        self.touch();
        true
    }

    /// Why [`Self::merge_down`] would refuse `index`, in the words the
    /// status line says — `None` = it will merge. One list so the palette
    /// never refuses silently: Ctrl+E doing nothing with no message was
    /// the 2026-09-02 surface pass's first finding in this family.
    pub fn merge_down_refusal(&self, index: usize) -> Option<&'static str> {
        let l = self.layers.get(index)?;
        let block = self.block_range(index);
        let Some(below) = block.start.checked_sub(1) else {
            return Some("nothing below this layer to merge into");
        };
        let b = &self.layers[below];
        if l.is_frame() {
            return Some("frame folders keep their vectors — they never merge");
        }
        if l.is_vector() {
            return Some("balloon and text layers keep their vectors — rasterize first");
        }
        if b.folder {
            return Some("the row below is a folder — open it and merge onto a layer inside, or merge the folder itself");
        }
        if b.is_vector() {
            return Some("the row below keeps its vectors (frame/balloon/text) — nothing can merge onto it");
        }
        // The DESTINATION may not be a stroke-recording layer: the merged
        // pixels would land in tiles the next replay zeroes. (The source
        // may be one — that is CSP's rasterize-and-merge, and its record
        // leaves with the layer.)
        if b.records_strokes() {
            return Some("the row below is a vector layer — its next edit would replay over the merged ink");
        }
        if l.tone.is_some() || b.tone.is_some() {
            return Some("merge refuses tone layers — remove the tone first (it is non-destructive)");
        }
        if l.lock {
            return Some("this layer is locked — unlock it to merge");
        }
        if b.lock {
            return Some("the layer below is locked — unlock it to merge into it");
        }
        // Merging across a folder boundary would smuggle pixels in or out
        // of a mask.
        if l.depth != b.depth {
            return Some("the row below is outside this folder — move the layer out first");
        }
        None
    }

    /// The isolated composite of the block `r` (a folder header and its
    /// children) as ONE raster layer, with the header's own opacity and
    /// blend carried on the result so the caller bakes it exactly as the
    /// page showed the group. Children are composited through the header
    /// at full opacity/Normal inside a scratch document, which is the
    /// group's isolated buffer by construction.
    fn flatten_block(&self, r: std::ops::Range<usize>) -> Layer {
        let header = &self.layers[r.end - 1];
        let mut scratch = Document::new(self.size.0, self.size.1);
        scratch.layers = self.layers[r].to_vec();
        let base = header.depth;
        for l in &mut scratch.layers {
            l.depth -= base;
            l.recording = None;
        }
        let h = scratch.layers.len() - 1;
        scratch.layers[h].opacity = 1.0;
        scratch.layers[h].blend = Blend::Normal;
        scratch.layers[h].visible = true;
        let img = crate::export::composite(&scratch, crate::export::Background::Transparent);
        let mut out = Layer::new(header.name.clone());
        out.opacity = header.opacity;
        out.blend = header.blend;
        out.visible = header.visible;
        fill_layer_from_image(&mut out, self.size, &img);
        out
    }

    /// CSP Layer ▸ Flatten image: every visible layer composites into ONE
    /// raster layer and the rest of the stack goes — hidden layers too, as
    /// CSP discards them. One structural undo step. Refused on a
    /// single-layer stack that is already a plain raster (nothing to do).
    pub fn flatten(&mut self) -> bool {
        if self.layers.len() == 1 {
            let l = &self.layers[0];
            if !l.folder && !l.is_vector() && l.tone.is_none() && l.strokes.is_none() {
                return false;
            }
        }
        let (before, active_before) = (self.stack_snapshot(), self.active);
        let img = crate::export::composite(self, crate::export::Background::Transparent);
        let mut out = Layer::new("Flattened");
        fill_layer_from_image(&mut out, self.size, &img);
        self.layers = vec![out];
        self.active = 0;
        self.record_structure("Flatten image", before, active_before);
        self.touch();
        true
    }

    /// CSP "Merge selected layers" (選択中のレイヤーを結合, the owner's
    /// Shift+Alt+E): flatten the palette's multi-selection into ONE raster
    /// layer at the LOWEST selected position, bottom-up, honouring each
    /// layer's blend, opacity and visibility. ONE structural undo step.
    ///
    /// Same refusals as [`Self::merge_down`], for the same reasons, applied
    /// to the whole set: folders, vector kinds and stroke-recording
    /// destinations never merge, a locked layer refuses edits, a clipped
    /// layer's raw pixels are not what it shows, and a set spanning a folder
    /// boundary would smuggle pixels in or out of a mask. The lowest
    /// selected layer MAY be clipped — it keeps its clip and the merge lands
    /// inside it, exactly as merging down into a clipped layer does.
    ///
    /// Unselected layers sitting BETWEEN selected ones are skipped, not
    /// preserved in order: the merged result lands at the lowest selected
    /// position with the rest of the sandwich still above it. That is CSP's
    /// behaviour and it only shows when an interleaved layer blends
    /// non-Normally — for the ordinary contiguous selection the page
    /// composites identically before and after.
    pub fn merge_selected(&mut self, indices: &[usize]) -> bool {
        let mut idx: Vec<usize> = indices
            .iter()
            .copied()
            .filter(|&i| i < self.layers.len())
            .collect();
        idx.sort_unstable();
        idx.dedup();
        if idx.len() < 2 {
            return false;
        }
        let depth = self.layers[idx[0]].depth;
        let refuses = |l: &Layer| {
            l.folder || l.is_vector() || l.records_strokes() || l.lock || l.depth != depth
        };
        if idx.iter().any(|&i| refuses(&self.layers[i]))
            || idx[1..].iter().any(|&i| self.layers[i].clip)
        {
            return false;
        }
        let (before, active_before) = (self.stack_snapshot(), self.active);
        let dst = idx[0];
        for &i in &idx[1..] {
            let upper = self.layers[i].clone();
            bake_layer_into(&mut self.layers[dst], &upper);
        }
        // Top-down, so the indices below each removal stay valid.
        for &i in idx[1..].iter().rev() {
            self.layers.remove(i);
        }
        self.active = dst;
        self.record_structure("Merge selected layers", before, active_before);
        self.touch();
        true
    }

    /// CSP "Release folder" (レイヤーフォルダーを解除, the owner's
    /// Ctrl+Shift+G): dissolve the folder at `index` — every descendant
    /// rises one level (nested folders keep their own nesting), the header
    /// goes, order is untouched. ONE structural undo step.
    ///
    /// Refused on a FRAME folder: its header carries the panel vectors AND
    /// the coverage mask its children are clipped by, so dropping it would
    /// quietly un-clip the art. [`Self::rasterize_frame_folder`] is that
    /// folder's release — it hands the mask down first. Also refused on a
    /// locked header, and on the last layer standing (a document always has
    /// a layer).
    ///
    /// The header's own rendering state — opacity, blend, mask, border
    /// effect, and the isolation a non-Through folder gives its children —
    /// cannot come with it, because no per-child value reproduces a group
    /// effect over overlapping children. A neutral folder therefore
    /// composites identically afterwards and a dressed one does not;
    /// [`Self::folder_release_is_lossless`] is the door callers warn from.
    pub fn release_folder(&mut self, index: usize) -> bool {
        let Some(h) = self.layers.get(index) else {
            return false;
        };
        if !h.folder || h.is_frame() || h.lock || self.layers.len() <= 1 {
            return false;
        }
        let (before, active_before) = (self.stack_snapshot(), self.active);
        for k in self.children_range(index).collect::<Vec<_>>() {
            // Every descendant is at least one deeper than the header, so
            // this is a plain rise; nesting between them is preserved.
            self.layers[k].depth = self.layers[k].depth.saturating_sub(1);
        }
        self.layers.remove(index);
        self.active = match self.active.cmp(&index) {
            std::cmp::Ordering::Greater => self.active - 1,
            std::cmp::Ordering::Equal => index.saturating_sub(1),
            std::cmp::Ordering::Less => self.active,
        }
        .min(self.layers.len() - 1);
        self.normalize_depths();
        self.record_structure("Release folder", before, active_before);
        self.touch();
        true
    }

    /// Whether releasing the folder at `index` leaves the page compositing
    /// the same — i.e. the header dresses nothing its children can inherit.
    /// `false` is not a refusal, it is the warning the command says out
    /// loud before doing it anyway (the artist can undo).
    pub fn folder_release_is_lossless(&self, index: usize) -> bool {
        let Some(h) = self.layers.get(index) else {
            return false;
        };
        let isolates = !h.through
            && self
                .children_range(index)
                .any(|k| self.layers[k].blend != Blend::Normal);
        h.visible
            && h.opacity >= 1.0
            && h.blend == Blend::Normal
            && h.mask.is_none()
            && h.edge.is_none()
            && h.tone.is_none()
            && !isolates
    }

    /// Batch: one tone change across many layers as ONE undo step
    /// (`UndoGroup::Compound` of the individual `Tones` groups). Layers
    /// that refuse (folders, vector kinds, already-equal) are skipped;
    /// returns how many changed. Zero changes push nothing.
    pub fn set_tone_many(
        &mut self,
        indices: &[usize],
        tone: Option<crate::tone::ToneParams>,
    ) -> usize {
        let mut members = Vec::new();
        for &i in indices {
            let Some(l) = self.layers.get_mut(i) else {
                continue;
            };
            if l.folder || l.is_vector() || l.tone == tone {
                continue;
            }
            let before = l.tone;
            l.tone = tone;
            l.tone_tiles = None;
            members.push(UndoGroup::Tones {
                layer: i,
                tone: before,
            });
        }
        let n = members.len();
        if n > 0 {
            self.history
                .push_labeled("Batch tone", UndoGroup::Compound(members));
            self.touch();
        }
        n
    }

}
