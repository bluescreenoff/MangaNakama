use super::*;

impl Document {
    /// Record one ruler gesture as ONE undo step: `before` is the whole set
    /// as it was when the gesture began, and the live `self.rulers` is
    /// already in its finished state. A no-op gesture pushes nothing.
    ///
    /// DOES `touch()` since rulers persist (`mnc/rulers.json`): a ruler
    /// edit must move `revision` or page stashing would skip the page and
    /// the edit would never reach disk.
    pub fn record_rulers(&mut self, before: crate::ruler::Rulers, label: &str) -> bool {
        if before == self.rulers {
            return false;
        }
        self.history
            .push_labeled(label, UndoGroup::Rulers { rulers: before });
        self.touch();
        true
    }

    /// Push a MaskField undo group (the BEFORE state; masks are small,
    /// whole-field snapshots — the Frames/Texts pattern).
    fn push_mask_group(&mut self, layer: usize, before: Option<LayerMask>, label: &str) {
        self.history.push_labeled(
            label,
            UndoGroup::Mask {
                layer,
                mask: before,
            },
        );
    }

    /// Push a Mask undo group for `layer` with `before` as the pre-image.
    /// For mutations that shift a linked mask OUTSIDE the LM-004 stroke
    /// bracket (the Object tool's folder move): snapshot the mask first,
    /// and hand the before-state here when the revision moved.
    pub fn record_mask_change(&mut self, layer: usize, before: Option<LayerMask>, label: &str) {
        self.push_mask_group(layer, before, label);
    }

    /// LM-004 stroke bracket: snapshot at stroke start; mask_op_end pushes
    /// one group when the coverage changed (revision moved).
    ///
    /// The snapshot records the mask's ABSENCE too (`None`), which is what
    /// lets [`Self::arm_full_window`] run inside the bracket and be undone
    /// with the stroke it armed for. Nothing is pushed when the state at
    /// `end` matches the state at `begin` — including maskless-to-maskless,
    /// the H1 backstop's case, which spent no undo step before and does not
    /// now.
    pub fn mask_op_begin(&mut self) {
        let m = self.active_layer().mask.as_ref();
        self.mask_op_snapshot = Some((m.cloned(), m.map(|m| m.revision)));
    }

    /// Row 105: ARM an all-visible window on the active layer, for the
    /// stroke that is about to start. A maskless correction layer refused
    /// brush strokes the way a maskless live fill does — but "erase the
    /// correction off the face" is what a CSP user reaches for first, and
    /// there is nothing to refuse: the window a correction wants here is
    /// the whole page, and [`LayerMask::full_window`] is that in an empty
    /// map. Call INSIDE the [`Self::mask_op_begin`] bracket; the pre-image
    /// is then "no mask at all" and one undo takes the window and the
    /// stroke together.
    ///
    /// Returns false when a mask is already there (nothing to arm).
    pub fn arm_full_window(&mut self) -> bool {
        let li = self.active;
        let Some(l) = self.layers.get_mut(li) else {
            return false;
        };
        if l.mask.is_some() {
            return false;
        }
        l.mask = Some(LayerMask::full_window());
        self.touch();
        true
    }

    /// Returns true when a group was pushed.
    pub fn mask_op_end(&mut self) -> bool {
        let Some((before, rev0)) = self.mask_op_snapshot.take() else {
            return false;
        };
        if self.active_layer().mask.as_ref().map(|m| m.revision) == rev0 {
            return false;
        }
        let li = self.active;
        // An armed window the stroke never wrote into: the arm was
        // speculative (the pen came down and went up again), so it is taken
        // back rather than spending the undo step an empty stroke has never
        // spent. An armed window that DID take a dab keeps every tile it
        // materialised.
        if before.is_none()
            && self.layers[li]
                .mask
                .as_ref()
                .is_some_and(|m| m.full && m.tiles.is_empty())
        {
            self.layers[li].mask = None;
            return false;
        }
        let label = if before.is_none() {
            "Correction window"
        } else {
            "Mask stroke"
        };
        self.push_mask_group(li, before, label);
        true
    }

    /// LM-002: Mask Outside Selection — the mask hides everything the
    /// selection does not cover (coverage = the selection). No selection
    /// = the whole layer is masked. Returns false for a bad layer.
    pub fn mask_outside_selection(&mut self, index: usize) -> bool {
        let sel = self.selection.clone();
        self.mask_from_coverage(index, move |x, y| {
            sel.as_ref()
                .map(|s| s.coverage(x, y) as u32 * 32768 / 255)
                .unwrap_or(0)
        })
    }

    /// LM-006: apply the mask to the layer — bake. Every layer pixel is
    /// multiplied by its coverage (premultiplied fix15), the mask is
    /// deleted, and the whole thing is ONE undoable op (tile pre-images).
    /// A disabled mask bakes nothing (CSP compares by toggling first).
    pub fn mask_apply_bake(&mut self, index: usize) -> bool {
        let Some(mask) = self
            .layers
            .get(index)
            .and_then(|l| l.mask.clone())
            .filter(|m| m.enabled)
        else {
            return false;
        };
        let idxs: Vec<TileIdx> = self.layers[index].tiles().map(|(i, _)| i).collect();
        self.begin_op();
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        for ti in idxs {
            let cov = mask.tiles.get(&ti);
            let tile = l.tile_mut(ti);
            let d = tile.data_mut();
            for p in 0..d.len() / 4 {
                // An ABSENT mask tile is VISIBLE — that is what both
                // compositors render (export.rs LM-005, gpu/lib.rs), and
                // mask tiles only exist where the layer had ink when the
                // mask was made. `unwrap_or(0)` here silently DELETED any
                // ink painted after that: shown on screen, erased by the
                // bake. Bake must match the screen.
                let m = cov.map(|c| c.data()[p * 4 + 3] as u32).unwrap_or(32768);
                if m == 32768 {
                    continue;
                }
                for c in 0..4 {
                    let i = p * 4 + c;
                    d[i] = (d[i] as u32 * m / 32768) as u16;
                }
            }
        }
        self.end_op();
        self.mask_delete(index);
        true
    }

    /// LM-001: Mask Selection — CSP's starter mask: ALL-VISIBLE (hides
    /// nothing yet; you paint the hiding in — part 2). With a selection
    /// present CSP still starts blank; the row's text is explicit.
    pub fn mask_selection_blank(&mut self, index: usize) -> bool {
        self.mask_from_coverage(index, |_, _| 32768)
    }

    fn mask_from_coverage(&mut self, index: usize, cov: impl Fn(i32, i32) -> u32) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        // NOT `paintable()`: a mask lives beside the tiles and the stroke
        // replay never touches it, so a stroke-recording layer masks like
        // any other raster one. Only derived rasters and folders refuse.
        if l.is_vector() || l.lock || l.folder {
            return false;
        }
        let idxs: Vec<TileIdx> = l.tiles().map(|(i, _)| i).collect();
        if idxs.is_empty() {
            return false;
        }
        let mut mask = LayerMask {
            tiles: HashMap::new(),
            enabled: true,
            revision: crate::tile::next_revision(),
            // LM-001/002 cut a window over the layer's own inked tiles.
            full: false,
        };
        for ti in idxs {
            let (ox, oy) = ti.origin();
            let mut t = Tile::new_transparent();
            let d = t.data_mut();
            for p in 0..crate::tile::TILE_PIXELS {
                let (x, y) = (
                    ox + (p % crate::tile::TILE_SIZE) as i32,
                    oy + (p / crate::tile::TILE_SIZE) as i32,
                );
                let c = cov(x, y).min(32768) as u16;
                d[p * 4] = c;
                d[p * 4 + 1] = c;
                d[p * 4 + 2] = c;
                d[p * 4 + 3] = c;
            }
            mask.tiles.insert(ti, Arc::new(t));
        }
        self.layers[index].mask = Some(mask);
        self.push_mask_group(index, None, "Mask");
        self.touch();
        true
    }

    /// LM-007: toggle the mask's effect without deleting it.
    pub fn mask_set_enabled(&mut self, index: usize, on: bool) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        let Some(m) = l.mask.as_mut() else {
            return false;
        };
        let before = m.clone();
        m.enabled = on;
        m.revision = crate::tile::next_revision();
        self.push_mask_group(index, Some(before), "Mask");
        self.touch();
        true
    }

    /// LM-003: Delete Mask — remove entirely (the layer shows as-is).
    pub fn mask_delete(&mut self, index: usize) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        let before = l.mask.take();
        if before.is_some() {
            self.push_mask_group(index, before, "Mask");
            self.touch();
            true
        } else {
            false
        }
    }

    /// LM-003: Clear Mask — keep the mask, empty its coverage (all
    /// hidden). The two destructive actions stay distinct commands.
    pub fn mask_clear(&mut self, index: usize) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        let Some(m) = l.mask.as_mut() else {
            return false;
        };
        let before = m.clone();
        for t in m.tiles.values_mut() {
            Arc::make_mut(t).data_mut().fill(0);
        }
        // A FULL window cannot express "all hidden" by zeroing what it
        // holds — the tiles it does NOT hold are the visible part, and there
        // is no dense form of them worth 30 MB. Clearing one drops the flag
        // instead: an empty CARVED window is a window that reaches nothing,
        // which is the same page and the same command's meaning.
        if m.full {
            m.full = false;
            m.tiles.clear();
        }
        m.revision = crate::tile::next_revision();
        self.push_mask_group(index, Some(before), "Mask");
        self.touch();
        true
    }

    /// EL-002: luminance → alpha per pixel (white becomes transparent,
    /// black stays opaque; Rec.709 luma on the UNPREMULTIPLIED colour,
    /// scaled by the existing alpha). THE scanned-lineart import path —
    /// one undoable op over the layer's tiles.
    pub fn convert_brightness_to_opacity(&mut self, index: usize) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        if !l.paintable() || l.lock {
            return false;
        }
        let idxs: Vec<TileIdx> = l.tiles().map(|(i, _)| i).collect();
        if idxs.is_empty() {
            return false;
        }
        self.begin_op();
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        for ti in idxs {
            let tile = l.tile_mut(ti);
            let data = tile.data_mut();
            for p in 0..data.len() / 4 {
                let i = p * 4;
                let (r, g, b, a) = (
                    data[i] as u32,
                    data[i + 1] as u32,
                    data[i + 2] as u32,
                    data[i + 3] as u32,
                );
                if a == 0 {
                    continue;
                }
                // Un-premultiply (fix15), luma, then re-premultiply with the
                // new alpha = a · (1 − luma).
                let un = |c: u32| c * 32768 / a;
                let (ur, ug, ub) = (un(r), un(g), un(b));
                let luma = (ur * 6967 + ug * 23435 + ub * 2366) / 32768; // Rec.709 in fix15 (sums to 32768)
                let na = a * (32768 - luma) / 32768;
                let sc = |c: u32| (un(c) * na / 32768).min(na) as u16;
                data[i] = sc(r);
                data[i + 1] = sc(g);
                data[i + 2] = sc(b);
                data[i + 3] = na as u16;
            }
        }
        self.end_op();
        true
    }

    /// Clamp the open op's changes back to the alpha the layer had before the
    /// op (transparent-pixel lock, CSP 透明ピクセルをロック): per pixel the
    /// alpha stays EXACTLY what it was; the colour takes the stroke as if it
    /// had painted a surface of that opacity. Recovering the stroke from the
    /// pre-image and the src-over result gives
    ///
    /// ```text
    /// sa    = (new.a − old.a) / (1 − old.a)      (0 when erasing)
    /// out.c = (1 − old.a) · old.c · (1 − sa) + old.a · new.c
    /// out.a = old.a
    /// ```
    ///
    /// Call ONCE, right before `end_op` — unlike the selection mask it is not
    /// idempotent.
    pub fn mask_op_to_alpha(&mut self) {
        use crate::blend::FIX15_ONE_F;
        let Some(li) = self.op_layer_index() else {
            return;
        };
        let layer = &mut self.layers[li];
        for idx in layer.recorded_tiles() {
            let pre = layer.recorded_pre_image(idx).cloned();
            match pre {
                // Tile did not exist: everything painted here lands on alpha
                // zero, so nothing sticks — drop the tile again.
                None => layer.set_tile(idx, None),
                Some(old) => {
                    let t = layer.tile_mut(idx);
                    let data = t.data_mut();
                    let od = old.data();
                    for p in 0..crate::tile::TILE_PIXELS {
                        let i = p * 4;
                        let m = od[i + 3] as f32 / FIX15_ONE_F;
                        if m >= 1.0 {
                            continue; // opaque: the stroke stands as painted
                        }
                        if m <= 0.0 {
                            for c in 0..4 {
                                data[i + c] = od[i + c];
                            }
                            continue;
                        }
                        let new_a = data[i + 3] as f32 / FIX15_ONE_F;
                        let sa = ((new_a - m) / (1.0 - m)).clamp(0.0, 1.0);
                        for c in 0..3 {
                            let oldv = od[i + c] as f32 / FIX15_ONE_F;
                            let newv = data[i + c] as f32 / FIX15_ONE_F;
                            let out = (1.0 - m) * oldv * (1.0 - sa) + m * newv;
                            data[i + c] = crate::blend::f32_to_fix15(out);
                        }
                        data[i + 3] = od[i + 3];
                    }
                }
            }
        }
    }
}
