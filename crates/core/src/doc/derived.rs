use super::*;

impl Document {
    /// Convert the layer at `index` into a screentone layer (or back): sets
    /// the screen params and drops the derived raster — the caller's next
    /// `refresh_derived` builds it. The painted pixels are the ink source and
    /// survive both directions (CSP トーンレイヤー, non-destructive). One undo
    /// step storing the previous params. Refuses vector layers and folders.
    pub fn set_tone(&mut self, index: usize, tone: Option<crate::tone::ToneParams>) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        if l.folder || l.is_vector() {
            return false;
        }
        if l.tone == tone {
            return false;
        }
        let before = l.tone;
        l.tone = tone;
        l.tone_tiles = None;
        self.history.push_labeled(
            "Tone",
            UndoGroup::Tones {
                layer: index,
                tone: before,
            },
        );
        self.touch();
        true
    }

    /// LP-002/LP-003: turn the border effect on (or off) for the layer at
    /// `index`. Non-destructive in both directions like the tone conversion —
    /// the painted pixels are the source and survive — so this follows the
    /// same model: one undo step holding the previous params, the derived
    /// raster dropped for the next `refresh_derived` to rebuild.
    ///
    /// Refuses FOLDERS: a folder header composites an isolated group, and
    /// its own raster is only the frame border ink — there is no single
    /// alpha for an outline to grow from. Vector layers (text, balloons,
    /// frames) are allowed; a keyline round a balloon is exactly the want.
    pub fn set_edge(&mut self, index: usize, edge: Option<crate::edge::EdgeParams>) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        // A PLAIN folder takes the effect as FB-knockout: the outline grows
        // from the union of its children's ink and lies just beneath the
        // group — the hand-painted "White" mat, automated. Frame folders
        // still refuse: their close already owns a mask + border ink.
        if (l.folder && l.is_frame()) || l.edge == edge {
            return false;
        }
        let before = l.edge;
        l.edge = edge;
        l.edge_tiles = None;
        l.edge_stamp = None;
        self.history.push_labeled(
            "Border effect",
            UndoGroup::Edges {
                layer: index,
                edge: before,
            },
        );
        self.touch();
        true
    }

    /// Re-derive every tone layer's halftone raster at `dpi`. Cheap when
    /// nothing changed (per-tile revision compare). The app calls this before
    /// compositing — render, export, fill/wand/eyedropper sampling, save-side
    /// composites.
    pub fn refresh_derived(&mut self, dpi: u32) {
        self.refresh_derived_with(dpi, &mut |_, _| None);
    }

    /// [`Self::refresh_derived`] with a correction kernel lent by the caller
    /// (`crate::correction::CorrKernel`). Only the correction stage consults
    /// it; every other derived raster is CPU-only and unaffected. The app's
    /// render loop passes the GPU seam here — see `App::refresh_tones`.
    pub fn refresh_derived_with(
        &mut self,
        dpi: u32,
        corr: &mut crate::correction::CorrKernel<'_>,
    ) {
        let size = self.size;
        for l in &mut self.layers {
            if l.tone.is_some() {
                l.refresh_tone(dpi);
            }
            if matches!(l.kind, LayerKind::Fill(_)) {
                l.refresh_fill(dpi, size);
            }
            // LAST: the border effect grows around whatever the two above
            // produced, so it must see their output, not their input.
            // Folders derive from their CHILDREN (below), not from here —
            // a folder's own raster is border ink, not the group's alpha.
            if !l.folder && (l.edge.is_some() || l.edge_tiles.is_some()) {
                l.refresh_edge(size);
            }
        }
        // FB-knockout: folder mats, AFTER the loop so children's derived
        // rasters (tone, live fill, their own edges) are what gets grown.
        for fi in 0..self.layers.len() {
            if self.layers[fi].folder {
                self.refresh_folder_edge(fi);
            }
        }
        // Correction layers LAST of all: their derivation composites the
        // layers below through the real compositor, so every other derived
        // raster must already be fresh when they read it.
        self.refresh_corrections_with(dpi, corr);
    }

    /// FB-knockout: derive a plain folder's border-effect raster from the
    /// union of its effectively-visible children's DISPLAY ink. The result
    /// is the mat alone (no source baked in — the group draws itself on
    /// top); both compositors lay it on the page just beneath the group at
    /// the folder's close, scaled by the folder's opacity.
    fn refresh_folder_edge(&mut self, fi: usize) {
        use std::collections::HashMap;
        use std::sync::Arc;
        let Some(p) = self.layers[fi].edge else {
            if self.layers[fi].edge_tiles.is_some() || self.layers[fi].edge_stamp.is_some() {
                self.layers[fi].edge_tiles = None;
                self.layers[fi].edge_stamp = None;
            }
            return;
        };
        let size = self.size;
        // Children that actually show: own eye on, and every folder between
        // them and this header open-eyed too. A hidden child must not leave
        // a ghost mat.
        let kids: Vec<usize> = self
            .children_range(fi)
            .filter(|&k| {
                if !self.layers[k].visible {
                    return false;
                }
                let mut a = k;
                while let Some(f) = self.enclosing_folder(a) {
                    if f == fi {
                        return true;
                    }
                    if !self.layers[f].visible {
                        return false;
                    }
                    a = f;
                }
                true
            })
            .collect();
        let stamp = {
            // Same shape as `refresh_edge`'s stamp: params, an
            // order-independent hash of WHICH tiles exist (child identity
            // and visibility folded in), and the newest source revision.
            let mut keys = 0i64;
            let mut newest = 0u64;
            for &k in &kids {
                keys = keys.wrapping_add((k as i64).wrapping_mul(0x51_7C_C1B7));
                for (idx, t) in self.layers[k].display_tiles() {
                    keys = keys.wrapping_add(
                        (idx.x as i64).wrapping_mul(0x9E37_79B9) ^ ((idx.y as i64) << 21),
                    );
                    newest = newest.max(t.revision());
                }
            }
            (p, keys, newest)
        };
        if self.layers[fi].edge_stamp == Some(stamp) && self.layers[fi].edge_tiles.is_some() {
            return;
        }
        let keep_cache =
            self.layers[fi].edge_stamp.map(|(q, k, _)| (q, k)) == Some((stamp.0, stamp.1));

        let r = p.reach();
        let ts = TILE_SIZE as i32;
        let span = r as i32 / ts + 1;
        let tsu = TILE_SIZE as u32;
        let (cw, chh) = (size.0.div_ceil(tsu) as i32, size.1.div_ceil(tsu) as i32);
        let mut cands: std::collections::HashSet<TileIdx> = Default::default();
        for &k in &kids {
            for (idx, _) in self.layers[k].display_tiles() {
                for dy in -span..=span {
                    for dx in -span..=span {
                        let i = TileIdx::new(idx.x + dx, idx.y + dy);
                        if i.x >= 0 && i.y >= 0 && i.x < cw && i.y < chh {
                            cands.insert(i);
                        }
                    }
                }
            }
        }
        let old = if keep_cache {
            self.layers[fi].edge_tiles.clone()
        } else {
            None
        };
        let mut out: HashMap<TileIdx, Arc<Tile>> = HashMap::new();
        let side = TILE_SIZE + 2 * r;
        let mut seed = vec![crate::edge::INF; side * side];
        for idx in cands {
            let neighbours = || {
                (-span..=span).flat_map(move |dy| {
                    (-span..=span).map(move |dx| TileIdx::new(idx.x + dx, idx.y + dy))
                })
            };
            let newest = neighbours()
                .map(|n| {
                    kids.iter()
                        .filter_map(|&k| self.layers[k].display_tile(n))
                        .map(|t| t.revision())
                        .max()
                        .unwrap_or(0)
                })
                .max()
                .unwrap_or(0);
            if let Some(t) = old.as_ref().and_then(|m| m.get(&idx))
                && t.revision() > newest
            {
                out.insert(idx, t.clone());
                continue;
            }
            seed.fill(crate::edge::INF);
            let (ox, oy) = idx.origin();
            let (px0, py0) = (ox - r as i32, oy - r as i32);
            for n in neighbours() {
                let (nx, ny) = n.origin();
                let x0 = px0.max(nx);
                let x1 = (px0 + side as i32).min(nx + ts);
                let y0 = py0.max(ny);
                let y1 = (py0 + side as i32).min(ny + ts);
                if x0 >= x1 || y0 >= y1 {
                    continue;
                }
                for &k in &kids {
                    let Some(t) = self.layers[k].display_tile(n) else {
                        continue;
                    };
                    let d = t.data();
                    for y in y0..y1 {
                        for x in x0..x1 {
                            let o = Tile::offset((x - nx) as usize, (y - ny) as usize);
                            if d[o + 3] >= crate::edge::INK_ALPHA {
                                seed[(y - py0) as usize * side + (x - px0) as usize] = 0.0;
                            }
                        }
                    }
                }
            }
            // No colour window: the knockout mat is a solid backing — a
            // watercolour style on a folder header would fall back to the
            // picked colour (nearest_ink_colour's empty-window rule).
            let t = crate::edge::derive_tile(&mut seed, r, None, p, &[]);
            out.insert(idx, Arc::new(t));
        }
        self.layers[fi].edge_stamp = Some(stamp);
        self.layers[fi].edge_tiles = Some(out);
    }

}
