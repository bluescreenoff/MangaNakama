use super::*;

impl Document {
    pub fn rename_layer(&mut self, index: usize, name: impl Into<String>) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        l.name = name.into();
        self.touch();
        true
    }

    /// Select the active layer. Out-of-range indices are rejected.
    ///
    /// A plain selection collapses the palette multi-selection (CSP: any
    /// non-modified click or keyboard move leaves one row selected) — the
    /// gesture methods below manage `active` directly instead.
    pub fn set_active(&mut self, index: usize) -> bool {
        if index >= self.layers.len() {
            return false;
        }
        self.active = index;
        self.layer_multi.clear();
        self.touch();
        true
    }

    /// TC-013 Ctrl+click: toggle `index` in the palette multi-selection.
    /// Toggling a row ON also makes it the editing target (CSP moves the
    /// pen); toggling the ACTIVE row off hands the target to the nearest
    /// remaining selected row. The last selected row cannot be toggled off.
    pub fn toggle_multi(&mut self, index: usize) -> bool {
        if index >= self.layers.len() {
            return false;
        }
        if index == self.active {
            // Deselect the target: someone else must take the pen.
            let Some(pos) = self.layer_multi.iter().rposition(|&m| m < index).or(
                if self.layer_multi.is_empty() {
                    None
                } else {
                    Some(0)
                },
            ) else {
                return false; // the only selected row stays selected
            };
            self.active = self.layer_multi.remove(pos);
        } else if let Some(pos) = self.layer_multi.iter().position(|&m| m == index) {
            self.layer_multi.remove(pos);
        } else {
            self.layer_multi.push(self.active);
            self.active = index;
            self.layer_multi.retain(|&m| m != index);
            self.layer_multi.sort_unstable();
        }
        self.touch();
        true
    }

    /// TC-013 Shift+click: select the contiguous range between the active
    /// row and `index`; the active row keeps the pen. Replaces any prior
    /// multi-selection, like CSP's range gesture.
    pub fn range_multi(&mut self, index: usize) -> bool {
        if index >= self.layers.len() {
            return false;
        }
        let (lo, hi) = (self.active.min(index), self.active.max(index));
        self.layer_multi = (lo..=hi).filter(|&i| i != self.active).collect();
        self.touch();
        true
    }

    /// The rows a "selected layers" operation targets: the active layer
    /// plus the multi-selection, bottom-to-top. Never empty.
    pub fn multi_targets(&self) -> Vec<usize> {
        let mut t = self.layer_multi.clone();
        t.push(self.active);
        t.sort_unstable();
        t
    }

    /// Set layer opacity (clamped 0..1) and publish a new document revision —
    /// the renderer cannot see this through tile revisions.
    pub fn set_layer_opacity(&mut self, index: usize, opacity: f32) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        l.opacity = opacity.clamp(0.0, 1.0);
        self.touch();
        true
    }

    /// Row 33 (CSP Convert layer): convert `li` — v1 rasterizes Text /
    /// Balloon / vector layers (the rendered tiles are kept as-is, the
    /// vector state dropped), optionally changes the expression colour
    /// and blend mode, renames, and either keeps or replaces the
    /// original. ONE structural undo step.
    pub fn convert_layer(
        &mut self,
        li: usize,
        rasterize: bool,
        expression: Option<LayerExpression>,
        blend: Option<Blend>,
        keep_original: bool,
        name: Option<String>,
    ) -> bool {
        let Some(src) = self.layers.get(li) else {
            return false;
        };
        if src.folder {
            return false;
        }
        let before = self.stack_snapshot();
        let active_before = self.active;
        let mut l = src.clone();
        if rasterize {
            // Bake: the tiles already hold the rendered vectors — the
            // conversion is dropping the vector state that regenerates
            // them.
            l.kind = LayerKind::Raster;
            l.strokes = None;
        }
        if let Some(e) = expression {
            l.expression = e;
        }
        if let Some(b) = blend {
            l.blend = b;
        }
        if let Some(n) = name {
            if !n.trim().is_empty() {
                l.name = n.trim().to_owned();
            }
        }
        if keep_original {
            // Original and converted copy both live — the copy is new.
            l.id = mint_id();
            self.layers.insert(li + 1, l);
            self.active = li + 1;
        } else {
            self.layers[li] = l;
            self.active = li;
        }
        self.record_structure("Convert layer", before, active_before);
        self.touch();
        true
    }
    /// Row 32 (CSP Rasterize on a FRAME folder — "do it their way"): the
    /// border is just ink, so the header becomes a plain raster layer
    /// holding it; the panel clipping is expressible as a layer mask, so
    /// every child gains the folder's interior-coverage mask (children
    /// that already carry their own keep it — the two combine visually);
    /// the children STAY separate layers, hoisted loose at the folder's
    /// depth. What you give up is the frame object — no more dragging
    /// panel edges. ONE structural undo step.
    pub fn rasterize_frame_folder(&mut self, li: usize) -> bool {
        let Some(h) = self.layers.get(li) else {
            return false;
        };
        if !(h.folder && h.is_frame()) {
            return false;
        }
        let (before, active_before) = (self.stack_snapshot(), self.active);
        let depth = h.depth;
        let mask_tiles = h.mask_tiles().cloned();
        let mut header = h.clone();
        let kids: Vec<usize> = self.children_range(li).collect();
        for k in kids {
            if let Some(c) = self.layers.get_mut(k) {
                // Loose at the folder's own depth — the folder is gone.
                c.depth = depth;
                if c.mask.is_none()
                    && let Some(tiles) = mask_tiles.clone()
                {
                    c.mask = Some(LayerMask {
                        tiles,
                        enabled: true,
                        revision: crate::tile::next_revision(),
                        full: false,
                    });
                }
            }
        }
        header.folder = false;
        header.kind = LayerKind::Raster;
        self.layers[li] = header;
        self.record_structure("Rasterize frame folder", before, active_before);
        self.touch();
        true
    }


    /// Row 31 (CSP 画像から線画を抽出, Extract lines): lift the active
    /// layer's DARK pixels as lineart onto a fresh layer above — per
    /// pixel, alpha scales with how far below the threshold the luma
    /// sits (a black line is a full-opacity line; a mid grey is a faint
    /// one), colour straight black. Returns the new layer's index.
    pub fn extract_lines(&mut self, li: usize, detection: f32) -> Option<usize> {
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        let thr = detection.clamp(0.02, 1.0);
        let src: Vec<(TileIdx, std::sync::Arc<Tile>)> =
            self.layers.get(li)?.tiles().map(|(i, t)| (i, t.clone())).collect();
        if src.is_empty() {
            return None;
        }
        let mut out = Layer::new("Extracted lines");
        out.name = "Extracted lines".into();
        for (idx, t) in &src {
            let (ox, oy) = idx.origin();
            let d = t.data();
            let nt = out.tile_mut(*idx);
            let nd = nt.data_mut();
            for py in 0..crate::tile::TILE_SIZE {
                for px in 0..crate::tile::TILE_SIZE {
                    let o = (py * crate::tile::TILE_SIZE + px) * 4;
                    let a = d[o + 3] as f32;
                    if a == 0.0 {
                        continue;
                    }
                    let (x, y) = (ox + px as i32, oy + py as i32);
                    if x < 0 || y < 0 || x >= w || y >= h {
                        continue;
                    }
                    // Straight luma over the alpha (the pixel's real
                    // coverage is carried by its own alpha below).
                    let inv = 1.0 / a;
                    let luma = (d[o] as f32 * inv).min(1.0) * 0.2126
                        + (d[o + 1] as f32 * inv).min(1.0) * 0.7152
                        + (d[o + 2] as f32 * inv).min(1.0) * 0.0722;
                    if luma >= thr {
                        continue;
                    }
                    // How much darker than the threshold = the line's
                    // strength, times the pixel's own alpha.
                    let strength = (thr - luma) / thr;
                    let alpha = (strength * a / crate::blend::FIX15_ONE_F)
                        .clamp(0.0, 1.0);
                    let f15 = crate::blend::f32_to_fix15(alpha);
                    nd[o] = 0;
                    nd[o + 1] = 0;
                    nd[o + 2] = 0;
                    nd[o + 3] = f15;
                }
            }
        }
        let before = self.stack_snapshot();
        let active_before = self.active;
        self.layers.insert(li + 1, out);
        self.active = li + 1;
        self.record_structure("Extract lines", before, active_before);
        self.touch();
        Some(self.active)
    }

    pub fn set_layer_blend(&mut self, index: usize, blend: Blend) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        l.blend = blend;
        self.touch();
        true
    }

    /// LF-002: set a folder Through (children stop isolating — they blend
    /// against everything beneath, as if loose). Presentation-only like
    /// visibility: no undo, composites recompute. Non-folder layers refuse.
    pub fn set_folder_through(&mut self, index: usize, on: bool) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        if !l.folder {
            return false;
        }
        l.through = on;
        self.touch();
        true
    }

    pub fn set_layer_visible(&mut self, index: usize, visible: bool) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        l.visible = visible;
        self.touch();
        true
    }

    /// The EYE solo (RF-001's hover promise, made real r113): hide every
    /// layer except `index`, returning the previous visibility vector for
    /// the restoring press. Presentation state like visibility itself —
    /// no undo.
    pub fn set_layer_visibility_solo(&mut self, index: usize) -> Option<Vec<bool>> {
        if self.layers.get(index).is_none() {
            return None;
        }
        let backup: Vec<bool> = self.layers.iter().map(|l| l.visible).collect();
        // The folders ENCLOSING the soloed row stay on: a hidden parent
        // hides its children, so soloing a layer inside a folder would
        // otherwise show a blank page (surface pass 2026-09-02).
        let keep = self.ancestors(index);
        for (i, l) in self.layers.iter_mut().enumerate() {
            l.visible = i == index || keep.contains(&i);
        }
        self.touch();
        Some(backup)
    }

    /// Restore a visibility snapshot (the solo's second press). A layer
    /// list that changed length in between simply truncates the restore.
    pub fn restore_visibility(&mut self, vis: &[bool]) {
        for (l, v) in self.layers.iter_mut().zip(vis) {
            l.visible = *v;
        }
        self.touch();
    }

    /// Every folder header enclosing `index`, innermost first.
    pub fn ancestors(&self, index: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut cur = index;
        while let Some(f) = self.enclosing_folder(cur) {
            out.push(f);
            cur = f;
        }
        out
    }

    /// Is `index` the only visible layer, its enclosing folders aside?
    /// (The solo's second-press test.)
    pub fn only_visible(&self, index: usize) -> bool {
        let keep = self.ancestors(index);
        if keep.iter().any(|&f| self.layers[f].visible)
            && self
                .layers
                .iter()
                .enumerate()
                .all(|(i, l)| l.visible == (i == index || keep.contains(&i)))
        {
            return true;
        }
        self.layers
            .iter()
            .enumerate()
            .all(|(i, l)| !l.visible || i == index)
    }

    /// Set/clear the palette-colour label (presentation only, like rename).
    pub fn set_layer_label(&mut self, index: usize, label: Option<[u8; 3]>) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        l.label = label;
        self.touch();
        true
    }

    /// PC-002: the palette colour the layer palette should PAINT for this
    /// row — its own label, or, for a folder without one, the topmost label
    /// found inside it. A collapsed folder still says what is in it, which is
    /// the only reason to colour layers in the first place.
    ///
    /// THE RULE, stated: a folder's own label always wins (CSP skips the
    /// inheritance entirely when the folder has one). Otherwise scan the
    /// folder's contents from the top down and take the first label. Nested
    /// folders need no recursion — a nested folder without a label of its own
    /// is skipped and the scan carries straight on into its children, which is
    /// the answer recursion would give. An empty folder, or one whose contents
    /// are all unlabelled, keeps a bare strip (`None`) rather than inventing a
    /// colour. Non-folders are always their own label.
    ///
    /// `layers` is bottom-first and a folder header sits ABOVE its contents,
    /// so "inside" is the run of lower indices at a greater depth.
    pub fn palette_colour(&self, index: usize) -> Option<[u8; 3]> {
        let l = self.layers.get(index)?;
        if !l.folder || l.label.is_some() {
            return l.label;
        }
        (0..index)
            .rev()
            .take_while(|&j| self.layers[j].depth > l.depth)
            .find_map(|j| self.layers[j].label)
    }

    /// PA-001: show/hide the paper. VIEW STATE like a layer's eye — no undo
    /// and no effect on export; hiding it puts the transparency checker
    /// under the stack so holes in a flat fill stop being invisible.
    /// Returns false when nothing changed (the caller skips its redraw).
    pub fn set_paper_visible(&mut self, visible: bool) -> bool {
        if self.paper.visible == visible {
            return false;
        }
        self.paper.visible = visible;
        self.touch();
        true
    }

    /// PA-001: set the paper colour. UNDOABLE, unlike the eye above — this
    /// one changes what the page exports, so it is content.
    pub fn set_paper_colour(&mut self, colour: [u8; 3]) -> bool {
        if self.paper.colour == colour {
            return false;
        }
        self.history.push_labeled(
            "Paper colour",
            UndoGroup::Paper {
                colour: self.paper.colour,
            },
        );
        self.paper.colour = colour;
        self.touch();
        true
    }

    /// PA-001: what sits under the stack ON SCREEN. `Transparent` when the
    /// paper's eye is off — the viewer then shows the transparency checker
    /// through the holes, which is the whole point of the switch.
    pub fn paper_background(&self) -> crate::export::Background {
        if self.paper.visible {
            crate::export::Background::Solid(self.paper.colour)
        } else {
            crate::export::Background::Transparent
        }
    }

    /// PA-001: what sits under the stack ON EXPORT — the paper colour,
    /// **whatever the eye says**. Hiding the paper is a look-at-it check, not
    /// an export mode: a hidden paper must never be the reason a page ships
    /// with a transparent background. (An explicitly transparent PNG is still
    /// available: the caller passes [`Background::Transparent`] itself.)
    ///
    /// [`Background::Transparent`]: crate::export::Background::Transparent
    pub fn paper_export_background(&self) -> crate::export::Background {
        crate::export::Background::Solid(self.paper.colour)
    }

    /// LP-016: set the layer colour (display tint). Presentation-only like
    /// visibility: no undo, composites recompute.
    pub fn set_layer_colour(&mut self, index: usize, colour: Option<[u8; 3]>) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        l.layer_colour = colour;
        self.touch();
        true
    }

    /// LP-017: set the two-tone SUB colour (the white end). Same
    /// presentation-only contract as the main colour above.
    pub fn set_layer_sub_colour(&mut self, index: usize, colour: Option<[u8; 3]>) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        l.layer_sub_colour = colour;
        self.touch();
        true
    }

    /// LP-022: set the decrease-colour PREVIEW. Display only — no pixel
    /// changes, no undo step, and the export composite ignores it.
    pub fn set_layer_expression(&mut self, index: usize, e: LayerExpression) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        if l.expression == e {
            return false;
        }
        l.expression = e;
        self.touch();
        true
    }

    /// Blend If: set (or clear) this layer's underlying-luminance gate.
    ///
    /// Folders refuse — v1 offers the gate on painted layers only, and the
    /// compositors read `Layer::blend_if` through [`Layer::gate`], which
    /// refuses folders too. Returns false on a no-op so a slider tick that
    /// did not move the value records no undo step.
    ///
    /// The value is [`crate::blendif::BlendIf::normalized`] on the way in:
    /// no compositor ever sees `hi < lo` (it would hide the layer outright).
    pub fn set_layer_blend_if(
        &mut self,
        index: usize,
        gate: Option<crate::blendif::BlendIf>,
    ) -> bool {
        let gate = gate.map(|g| g.normalized());
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        if l.folder || l.blend_if == gate {
            return false;
        }
        l.blend_if = gate;
        self.touch();
        true
    }

    /// Toggle clip-to-layer-below. Folders refuse (their group already
    /// isolates). Like visibility, not undoable.
    pub fn set_layer_clip(&mut self, index: usize, clip: bool) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        if l.folder {
            return false;
        }
        l.clip = clip;
        self.touch();
        true
    }

    /// Toggle the edit lock (the app's stroke/fill/clear paths check it).
    pub fn set_layer_lock(&mut self, index: usize, lock: bool) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        l.lock = lock;
        self.touch();
        true
    }

    /// Toggle the transparent-pixel lock.
    pub fn set_layer_lock_alpha(&mut self, index: usize, lock: bool) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        l.lock_alpha = lock;
        self.touch();
        true
    }

    /// The reference-layer SET (RF-001, owner spec 2026-08-17): any number
    /// of layers, marked independently. Stack order (bottom→top).
    pub fn reference_layers(&self) -> Vec<usize> {
        self.layers
            .iter()
            .enumerate()
            .filter(|(_, l)| l.reference)
            .map(|(i, _)| i)
            .collect()
    }

    /// The topmost reference layer, if any (compat for single-layer
    /// consumers; the SET is `reference_layers`).
    pub fn reference_layer_index(&self) -> Option<usize> {
        self.layers.iter().rposition(|l| l.reference)
    }

    /// Toggle ONE layer's reference flag, independently of every other
    /// (click). The owner REJECTED CSP's exclusivity: marking a sixth with
    /// five marked must not clear them. Like visibility, presentation-only
    /// and not undoable.
    pub fn set_layer_reference(&mut self, index: usize, on: bool) -> bool {
        if self.layers.get(index).is_none() {
            return false;
        }
        self.layers[index].reference = on;
        self.touch();
        true
    }

    /// SOLO: clear every other layer's flag and set this one (Alt+click).
    pub fn set_layer_reference_solo(&mut self, index: usize) -> bool {
        if self.layers.get(index).is_none() {
            return false;
        }
        for (i, l) in &mut self.layers.iter_mut().enumerate() {
            l.reference = i == index;
        }
        self.touch();
        true
    }

    /// Clear every reference flag (the owner's "clear all" command).
    pub fn clear_references(&mut self) {
        for l in &mut self.layers {
            l.reference = false;
        }
        self.touch();
    }

    /// Mark this layer as a draft (CSP 下書き): visible on screen, excluded
    /// from fill reference sampling and from export.
    pub fn set_layer_draft(&mut self, index: usize, on: bool) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        l.draft = on;
        self.touch();
        true
    }

    /// FB-overflow: refuses folders (re-seating a whole group is a v2) and
    /// layers with no frame folder above them (the flag would be a lie).
    pub fn set_layer_escape(&mut self, index: usize, on: bool) -> bool {
        let ok = self
            .layers
            .get(index)
            .is_some_and(|l| !l.folder && (!on || self.enclosing_frame_folder(index).is_some()));
        if !ok {
            return false;
        }
        self.layers[index].escape_frame = on;
        self.touch();
        true
    }

}
