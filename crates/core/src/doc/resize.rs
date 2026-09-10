use super::*;

impl Document {
    /// Resize with an explicit content offset: after this, the pixel that was
    /// at (x, y) sits at (x + dx, y + dy) — Crop is `resize_to(w, h, -x0, -y0)`
    /// for the selection's bbox. Tiles that land fully outside the new canvas
    /// are dropped (the crop is destructive: undo cannot express it, so the
    /// history is cleared like every structural op). Vector layers translate
    /// and re-derive their rasters at the new size; a frame folder's White
    /// base re-extends to cover the grown canvas so panels placed in the new
    /// area still sit on paper.
    pub fn resize_to(&mut self, new_w: u32, new_h: u32, dx: i32, dy: i32) {
        let old = self.size;
        let new = (new_w.max(1), new_h.max(1));
        if new == old && dx == 0 && dy == 0 {
            return;
        }
        for l in &mut self.layers {
            // A canvas-filling uniform-white layer is a frame folder's White
            // base: keep it covering the whole (possibly grown) canvas.
            let was_paper = l.covers_canvas(old) && l.is_uniform_white();
            l.translate_content(dx, dy);
            if was_paper {
                l.extend_white(new);
            }
        }
        // Derived rasters rebuild at the new size (translate_content already
        // moved the vectors; its raster blit is discarded by the re-derive).
        for l in &mut self.layers {
            if l.is_frame() {
                Self::derive_frame_raster(l, new);
            } else if let Some(bs) = l.balloons().cloned() {
                let raster = bs.rasterize(new);
                l.replace_tiles(raster);
            } else if let Some(ts) = l.texts().cloned() {
                let raster = ts.rasterize(new);
                l.replace_tiles(raster);
            }
        }
        self.trim_outside(new);
        self.size = new;
        self.selection = None;
        // STILL clears (2026-08-21, the structural-undo round): the canvas
        // SIZE is not in a `UndoGroup::Structure` snapshot, and dropped
        // outside-tiles are destroyed for real — a resize is the one
        // structural change undo genuinely cannot express yet.
        self.clear_history();
        self.touch();
    }

    /// CSP's Image ▸ **Change image resolution** (`IO-060`), the other half
    /// of the pair whose first half is [`Self::resize_canvas`]: the paper
    /// stays the same PHYSICAL page and every pixel is re-made at a new
    /// resolution. Nothing is re-framed and nothing is cropped.
    ///
    /// Returns `false` (document untouched) for a degenerate target.
    ///
    /// # Why this is not "scale everything by the same number"
    ///
    /// * RASTER content resamples through `interp` — [`Interp::HighAccuracy`]
    ///   is the reduction kernel and the reason a 1 px hairline comes through
    ///   grey instead of missing.
    /// * DERIVED content does not resample: a tone layer's dots, a live
    ///   fill, a border effect and a frame folder's mat are all dropped here
    ///   and rebuilt by `refresh_derived(new_dpi)` / the re-derive below.
    ///   That is the whole tone-awareness the JP guides ask for — the screen
    ///   re-flows at the new dpi at the SAME lpi rather than being filtered
    ///   like a photograph.
    /// * VECTOR geometry scales and re-derives (`Layer::scale_vectors`).
    /// * PHYSICAL numbers — lpi, pt — do not move at all.
    ///
    /// The caller owns `PageSetup`: `dpi` changes, `paper_mm` does not.
    ///
    /// Like every structural op the history is cleared — a whole-work
    /// resample is not an undo step, it is a decision (see `resize_to`).
    pub fn resample_to(&mut self, new_w: u32, new_h: u32, interp: crate::transform::Interp) -> bool {
        let old = self.size;
        let new = (new_w.max(1), new_h.max(1));
        if old.0 == 0 || old.1 == 0 {
            return false;
        }
        if new == old {
            return true;
        }
        let sx = new.0 as f32 / old.0 as f32;
        let sy = new.1 as f32 / old.1 as f32;
        for l in &mut self.layers {
            // A canvas-filling uniform-white layer is a frame folder's White
            // base (and a new page's paper): re-lay it at the new size
            // rather than resampling a page of solid white, which would only
            // buy a fringe of half-alpha along the edges.
            if l.covers_canvas(old) && l.is_uniform_white() {
                l.tiles.clear();
                l.extend_white(new);
                l.resample_meta(sx, sy, interp);
                continue;
            }
            l.resample_content(sx, sy, interp);
        }
        // Vector rasters rebuild from the geometry that just scaled — the
        // resampled blit `resample_content` produced is discarded here.
        // Text is the exception: its sprites are shaped by the APP at a dpi
        // the core cannot reach, so a re-raster now would blank the layer.
        // The resampled pixels stand in until the app re-warms the caches.
        for l in &mut self.layers {
            if l.is_frame() {
                Self::derive_frame_raster(l, new);
            } else if let Some(bs) = l.balloons().cloned() {
                let raster = bs.rasterize(new);
                l.replace_tiles(raster);
            }
        }
        self.rulers.scale(sx, sy);
        self.size = new;
        self.selection = None;
        self.sel_scratch = LayerMask::default();
        self.clear_history();
        self.touch();
        true
    }

    /// CSP "Change canvas size": pin content to `anchor` while the canvas
    /// becomes `new_w × new_h`.
    pub fn resize_canvas(&mut self, new_w: u32, new_h: u32, anchor: ResizeAnchor) {
        let (dx, dy) = anchor.offsets(self.size, (new_w, new_h));
        self.resize_to(new_w, new_h, dx, dy);
    }

    /// Drop every tile that lies entirely outside the canvas (crop residue).
    fn trim_outside(&mut self, size: (u32, u32)) {
        let ts = TILE_SIZE as i32;
        let (cw, ch) = (size.0 as i32, size.1 as i32);
        for l in &mut self.layers {
            l.tiles.retain(|ti, _| {
                let (ox, oy) = ti.origin();
                ox < cw && oy < ch && ox + ts > 0 && oy + ts > 0
            });
        }
    }
}
