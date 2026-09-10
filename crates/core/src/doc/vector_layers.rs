use super::*;

impl Document {
    /// New balloon layer at the **top** of the stack — balloons sit above the
    /// art and the frames they annotate — rasterized from `balloons` and made
    /// active. Records one structural undo step.
    pub fn add_balloon_layer(&mut self, name: impl Into<String>, balloons: BalloonSet) -> usize {
        let mut sp = SpeechSet::of_balloons(balloons);
        sp.mint_ids();
        let (before, active_before) = (self.stack_snapshot(), self.active);
        let mut l = Layer::new(name);
        l.replace_tiles(sp.rasterize(self.size));
        l.kind = LayerKind::Speech(sp);
        self.layers.push(l);
        self.active = self.layers.len() - 1;
        self.record_structure("New balloon layer", before, active_before);
        self.touch();
        self.active
    }

    /// Replace a balloon layer's vector state, re-rasterize, and push one
    /// undo step. Returns `false` when `index` is not a balloon layer.
    pub fn set_balloons(&mut self, index: usize, mut balloons: BalloonSet) -> bool {
        // New items arrive with id 0 (and a duplicated item carries its
        // source's id) — the commit door is where identities become real.
        balloons.mint_ids();
        let size = self.size;
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        let LayerKind::Speech(sp) = &mut l.kind else {
            return false;
        };
        let before = std::mem::replace(&mut sp.balloons, balloons);
        let raster = sp.rasterize(size);
        l.replace_tiles(raster);
        self.history.push_labeled(
            "Balloon",
            UndoGroup::Balloons {
                layer: index,
                balloons: before,
            },
        );
        self.touch();
        true
    }

    /// New text layer at the **top** of the stack (text sits above everything,
    /// balloons included), rasterized from `texts` and made active. Records
    /// one structural undo step.
    pub fn add_text_layer(&mut self, name: impl Into<String>, texts: TextSet) -> usize {
        let mut sp = SpeechSet::of_texts(texts);
        sp.mint_ids();
        let (before, active_before) = (self.stack_snapshot(), self.active);
        let mut l = Layer::new(name);
        l.replace_tiles(sp.rasterize(self.size));
        l.kind = LayerKind::Speech(sp);
        self.layers.push(l);
        self.active = self.layers.len() - 1;
        self.record_structure("New text layer", before, active_before);
        self.touch();
        self.active
    }

    /// Replace a text layer's vector state, re-rasterize, and push one undo
    /// step. Returns `false` when `index` is not a text layer.
    /// Set a text layer's vector state with NO rasterize and NO undo —
    /// the Story Editor's non-active-page path (the doc re-encodes to
    /// bytes; its raster rebuilds when the page loads and warms).
    pub fn set_texts_raw(&mut self, index: usize, mut texts: TextSet) -> bool {
        texts.mint_ids();
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        let LayerKind::Speech(sp) = &mut l.kind else {
            return false;
        };
        sp.texts = texts;
        self.touch();
        true
    }

    /// Re-blit a text layer from the sprites it already holds — no undo
    /// step, no reshaping, no change to the vector state.
    ///
    /// `IO-060`'s text half: a work resample drops every sprite (they were
    /// shaped at the old dpi) and leaves the RESAMPLED pixels standing so
    /// the page is never blank. Once the app has re-warmed the caches at
    /// the new dpi this lays the crisp sprites down over them. Pushing an
    /// undo step here would be a lie — the step before it is a text layer
    /// with no sprites, which rasterizes to nothing.
    pub fn reraster_text(&mut self, index: usize) -> bool {
        let size = self.size;
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        let LayerKind::Speech(sp) = &l.kind else {
            return false;
        };
        if sp.texts.texts.iter().all(|t| t.cache.is_none()) {
            return false;
        }
        let raster = sp.rasterize(size);
        l.replace_tiles(raster);
        self.touch();
        true
    }

    pub fn set_texts(&mut self, index: usize, mut texts: TextSet) -> bool {
        // Same contract as `set_balloons`: the commit mints new identities.
        texts.mint_ids();
        let size = self.size;
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        let LayerKind::Speech(sp) = &mut l.kind else {
            return false;
        };
        let before = std::mem::replace(&mut sp.texts, texts);
        let raster = sp.rasterize(size);
        l.replace_tiles(raster);
        self.history.push_labeled(
            "Text",
            UndoGroup::Texts {
                layer: index,
                texts: before,
            },
        );
        self.touch();
        true
    }

    /// Fill missing sprite caches on a text layer in place — no history, no
    /// re-raster. An ORA-loaded text layer keeps its PNG pixels and has no
    /// caches; the app must warm them (via the text engine) before the first
    /// `set_texts`, or undoing that first edit would restore cache-less items
    /// that rasterize to nothing.
    pub fn warm_text_caches(
        &mut self,
        index: usize,
        mut shape: impl FnMut(&TextItem) -> Option<Arc<RenderedText>>,
    ) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        let LayerKind::Speech(sp) = &mut l.kind else {
            return false;
        };
        for item in &mut sp.texts.texts {
            if item.cache.is_none() {
                item.cache = shape(item);
            }
        }
        true
    }

}
