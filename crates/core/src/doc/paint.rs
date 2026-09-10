use super::*;

impl Document {
    /// Tiles per freeform batch — the same 256 the compositor's
    /// `UPLOAD_BATCH`, `correction::DERIVE_BATCH` and the kernel's
    /// `TILE_BATCH` use: 1 Mpx, 8 MB of pixels, one dispatch's worth.
    ///
    /// Also the granularity of a kernel's decline, so a page whose densest
    /// batch overflows a segment cap still runs every other batch on the
    /// GPU.
    const FREEFORM_BATCH: usize = 256;

    /// Guard for the paint ops below: the active layer must accept pixels.
    fn paint_guard(&self) -> bool {
        let l = self.active_layer();
        l.paintable() && !l.lock
    }

    /// Src-over one premultiplied fix15 pixel onto `dst`.
    fn over_pixel(dst: &mut [u16], s: [u16; 4]) {
        let a = s[3] as u32;
        let inv = 32768 - a;
        for k in 0..3 {
            let d = dst[k] as u32;
            dst[k] = ((s[k] as u32).saturating_add((d * inv + 16384) >> 15)).min(32768) as u16;
        }
        let d = dst[3] as u32;
        dst[3] = (a.saturating_add((d * inv + 16384) >> 15)).min(32768) as u16;
    }

    /// Paint a linear gradient on the active layer (the CSP Gradient tool's
    /// core op): colours interpolate along `a`→`b` (canvas px, straight
    /// colour RGBA, premultiplied here); perpendicular strips are constant.
    /// Selection-clipped, one undo step. Returns false on a refusing layer.
    ///
    /// The two-colour form — every ramp option at its default. Anything the
    /// Tool Property panel authored goes through [`Self::paint_gradient_ramp`].
    pub fn paint_gradient(
        &mut self,
        a: [f32; 2],
        b: [f32; 2],
        from: [f32; 4],
        to: [f32; 4],
    ) -> bool {
        self.paint_gradient_ramp(a, b, &crate::gradient::Ramp::two(from, to))
    }

    /// The authored-ramp form: interior colour stops, edge process, flip,
    /// dithering, centre-out, mixing mode and mixing rate all apply. Pixels
    /// the ramp declines to draw ("do not draw" outside the dragged span)
    /// are left byte-untouched, not painted transparent.
    pub fn paint_gradient_ramp(
        &mut self,
        a: [f32; 2],
        b: [f32; 2],
        ramp: &crate::gradient::Ramp,
    ) -> bool {
        if !self.paint_guard() {
            return false;
        }
        let ab = [b[0] - a[0], b[1] - a[1]];
        let ab2 = ab[0] * ab[0] + ab[1] * ab[1];
        if ab2 < 1e-6 {
            return false;
        }
        // The gradient spans infinitely ACROSS `a→b` (CSP behaviour: the ramp
        // band extends over the whole canvas perpendicular to the drag).
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        let sel = self.selection.clone();
        let li = self.active;
        let lock_alpha = self.layers[li].lock_alpha;
        self.begin_op();
        for ty in 0..(h + TILE_SIZE as i32 - 1) / TILE_SIZE as i32 {
            for tx in 0..(w + TILE_SIZE as i32 - 1) / TILE_SIZE as i32 {
                let idx = TileIdx::new(tx, ty);
                if let Some(s) = &sel {
                    if s.tile_mask(idx).is_none() {
                        continue;
                    }
                }
                let (ox, oy) = idx.origin();
                // The projection is affine, so the tile's four corners bound
                // it: a "do not draw" ramp can reject the whole tile here,
                // BEFORE `tile_mut` allocates it and stashes an undo
                // pre-image for a tile it was never going to touch.
                let proj = |px: f32, py: f32| ((px - a[0]) * ab[0] + (py - a[1]) * ab[1]) / ab2;
                let (tx1, ty1) = (
                    (ox + TILE_SIZE as i32) as f32,
                    (oy + TILE_SIZE as i32) as f32,
                );
                let us = [
                    proj(ox as f32, oy as f32),
                    proj(tx1, oy as f32),
                    proj(ox as f32, ty1),
                    proj(tx1, ty1),
                ];
                let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
                for u in us {
                    lo = lo.min(u);
                    hi = hi.max(u);
                }
                if !ramp.draws_span(lo, hi) {
                    continue;
                }
                let tile = self.layers[li].tile_mut(idx);
                let data = tile.data_mut();
                for p in 0..TILE_SIZE * TILE_SIZE {
                    let x = ox + (p % TILE_SIZE) as i32;
                    let y = oy + (p / TILE_SIZE) as i32;
                    if x >= w || y >= h {
                        continue;
                    }
                    let px = [x as f32 + 0.5, y as f32 + 0.5];
                    let u = proj(px[0], px[1]);
                    let Some(s) = ramp.eval(u, x, y) else {
                        continue; // edge process "do not draw"
                    };
                    // PREMULTIPLY. This op used to write the straight colour
                    // beside a faded alpha, which is not a valid fix15 pixel:
                    // a mid-grey fading to transparent came out brightening
                    // toward white, and the LIVE gradient layer (which does
                    // premultiply, `fill_layer::build_fill_tile`) disagreed
                    // with the destructive tool on the same parameters.
                    let al = crate::blend::f32_to_fix15(s[3]);
                    let pr = |v: f32| crate::blend::f32_to_fix15(v * s[3]).min(al);
                    let c = [pr(s[0]), pr(s[1]), pr(s[2]), al];
                    Self::over_pixel(&mut data[p * 4..p * 4 + 4], c);
                }
            }
        }
        self.mask_op_to_selection();
        if lock_alpha {
            self.mask_op_to_alpha();
        }
        self.end_op();
        true
    }

    /// `FI-050` — the FREEFORM gradient. The ramp runs from guide polyline
    /// `l1` (parameter 0) to guide polyline `l2` (parameter 1), FOLLOWING
    /// their shapes: every pixel takes the ratio of its distances to the two
    /// guides. Same layer rules, selection clip and single undo step as
    /// [`Self::paint_gradient_ramp`]. False on a refusing layer, or when
    /// either guide has no usable point.
    ///
    /// See [`crate::freeform`] for the geometry, the per-tile segment cull
    /// that makes a full page affordable, and why this needs no
    /// anti-aliasing switch on the guides (they are never pixels).
    ///
    /// Unlike the linear ramp there is no tile to skip: the parameter is
    /// defined over the whole canvas, so "do not draw" has no outside to
    /// leave alone and every selected tile is painted.
    pub fn paint_gradient_freeform(
        &mut self,
        l1: &[[f32; 2]],
        l2: &[[f32; 2]],
        ramp: &crate::gradient::Ramp,
    ) -> bool {
        self.paint_gradient_freeform_with(l1, l2, ramp, &mut |_, _| None)
    }

    /// [`Self::paint_gradient_freeform`] with a kernel lent by the caller —
    /// the GPU seam's door into the freeform field.
    ///
    /// The batch is [`FREEFORM_BATCH`] tiles at a time, each carrying its own
    /// CULLED segment lists (`Window::pack_into`) so a kernel does exactly
    /// the work the CPU loop does and no more. `run` sees the flattened
    /// field plus the batch's CURRENT pixels and returns the painted ones —
    /// src-over included, because the destination is what it was handed —
    /// or `None` to decline the batch, in which case the loop below paints
    /// it. Declining per batch rather than per page is deliberate: a
    /// segment pool that overflows the kernel's cap on ONE dense batch does
    /// not cost the rest of the page its speed-up.
    ///
    /// Undo is untouched: every tile still goes through `tile_mut`, which is
    /// what stashes the pre-image, and the whole apply is one `begin_op` /
    /// `end_op` bracket whoever painted the pixels.
    pub fn paint_gradient_freeform_with(
        &mut self,
        l1: &[[f32; 2]],
        l2: &[[f32; 2]],
        ramp: &crate::gradient::Ramp,
        run: &mut crate::freeform::FieldKernel<'_>,
    ) -> bool {
        if !self.paint_guard() {
            return false;
        }
        let Some(field) = crate::freeform::Freeform::new(l1, l2) else {
            return false;
        };
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        let sel = self.selection.clone();
        let li = self.active;
        let lock_alpha = self.layers[li].lock_alpha;
        // Half the diagonal of one tile: no pixel in a tile is further than
        // this from its centre, which is exactly what the cull's bound needs.
        let hd = TILE_SIZE as f32 * 0.5 * std::f32::consts::SQRT_2;
        let half = TILE_SIZE as f32 * 0.5;
        let todo: Vec<TileIdx> = (0..(h + TILE_SIZE as i32 - 1) / TILE_SIZE as i32)
            .flat_map(|ty| {
                (0..(w + TILE_SIZE as i32 - 1) / TILE_SIZE as i32).map(move |tx| TileIdx::new(tx, ty))
            })
            .filter(|idx| sel.as_ref().is_none_or(|s| s.tile_mask(*idx).is_some()))
            .collect();
        self.begin_op();
        for chunk in todo.chunks(Self::FREEFORM_BATCH) {
            // Cull BEFORE `tile_mut`: a guide with hundreds of segments is
            // otherwise re-scanned 4096 times per tile.
            let mut pool: Vec<f32> = Vec::new();
            let mut plans = Vec::with_capacity(chunk.len());
            let mut wins = Vec::with_capacity(chunk.len());
            for &idx in chunk {
                let (ox, oy) = idx.origin();
                let win = field.window([ox as f32 + half, oy as f32 + half], hd);
                plans.push(win.pack_into(&mut pool, (ox, oy)));
                wins.push(win);
            }
            // The tiles as they stand — the kernel's source AND destination.
            let mut px = vec![0u16; chunk.len() * crate::tile::TILE_LEN];
            for (n, &idx) in chunk.iter().enumerate() {
                if let Some(t) = self.layers[li].tile(idx) {
                    px[n * crate::tile::TILE_LEN..(n + 1) * crate::tile::TILE_LEN]
                        .copy_from_slice(t.data());
                }
            }
            let job = crate::freeform::FieldJob {
                segs: &pool,
                plans: &plans,
                ramp,
                size: self.size,
            };
            // A host that hands back the wrong length is a bug on its side,
            // never a reason to write short tiles — same guard the
            // correction derive puts on its lent kernel.
            let lent = run(&job, &px).filter(|o| o.len() == px.len());
            for (n, &idx) in chunk.iter().enumerate() {
                let tile = self.layers[li].tile_mut(idx);
                let data = tile.data_mut();
                if let Some(out) = &lent {
                    data.copy_from_slice(
                        &out[n * crate::tile::TILE_LEN..(n + 1) * crate::tile::TILE_LEN],
                    );
                    continue;
                }
                let (ox, oy) = idx.origin();
                for p in 0..TILE_SIZE * TILE_SIZE {
                    let x = ox + (p % TILE_SIZE) as i32;
                    let y = oy + (p / TILE_SIZE) as i32;
                    if x >= w || y >= h {
                        continue;
                    }
                    let t = wins[n].t_at([x as f32 + 0.5, y as f32 + 0.5]);
                    let s = ramp.eval_unit(t, x, y);
                    // Premultiply, for the reason spelled out in
                    // `paint_gradient_ramp`.
                    let al = crate::blend::f32_to_fix15(s[3]);
                    let pr = |v: f32| crate::blend::f32_to_fix15(v * s[3]).min(al);
                    let c = [pr(s[0]), pr(s[1]), pr(s[2]), al];
                    Self::over_pixel(&mut data[p * 4..p * 4 + 4], c);
                }
            }
        }
        self.mask_op_to_selection();
        if lock_alpha {
            self.mask_op_to_alpha();
        }
        self.end_op();
        true
    }

    /// `FI-051` — the freeform gradient with THREE OR MORE guides, each
    /// carrying its own colour. Every pixel is the inverse-distance blend of
    /// the guide colours, so the field follows all of the drawn shapes at
    /// once ([`crate::freeform::Multi`] for the maths and the cull).
    ///
    /// TWO guides are NOT blended here: they route to
    /// [`Self::paint_gradient_freeform`], the shipped ramp path, which is
    /// what carries the ramp's interior stops, flip and edge process. The
    /// caller passes `ramp` for that case; with three or more guides only
    /// its mixing space, brightness lift and dither are read (module doc).
    ///
    /// Same layer rules, selection clip and single undo step as the two-line
    /// form. False on a refusing layer, on fewer than two guides, or when
    /// any guide has no usable point.
    pub fn paint_gradient_freeform_multi(
        &mut self,
        guides: &[crate::freeform::ColourGuide],
        ramp: &crate::gradient::Ramp,
    ) -> bool {
        self.paint_gradient_freeform_multi_with(guides, ramp, &mut |_, _| None)
    }

    /// [`Self::paint_gradient_freeform_multi`] with a kernel lent by the
    /// caller.
    ///
    /// The kernel reaches the TWO-guide path only. `FI-051`'s N-guide field
    /// is an inverse-distance blend whose colour stage is a SEQUENTIAL mix —
    /// `idw_colour` folds guide k into the running average with
    /// `mix::mix_rgba`, and two of the three mixing spaces are `powf`/`cbrt`
    /// — so it has no exact-parity kernel form the way the two-line ramp
    /// does. It runs the CPU reference, which is where it is exact.
    pub fn paint_gradient_freeform_multi_with(
        &mut self,
        guides: &[crate::freeform::ColourGuide],
        ramp: &crate::gradient::Ramp,
        run: &mut crate::freeform::FieldKernel<'_>,
    ) -> bool {
        if let [a, b] = guides {
            // The pinned two-line path, byte for byte — the ramp's ends are
            // the two guides' colours, which is where the gesture put them.
            let ramp = crate::gradient::Ramp::new(a.colour, b.colour, ramp.mid, ramp.opts);
            return self.paint_gradient_freeform_with(&a.pts, &b.pts, &ramp, run);
        }
        if guides.len() < 2 || !self.paint_guard() {
            return false;
        }
        let Some(field) = crate::freeform::Multi::new(guides) else {
            return false;
        };
        let (mix, bright, dither) = (ramp.opts.mix, ramp.opts.bright, ramp.opts.dither);
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        let sel = self.selection.clone();
        let li = self.active;
        let lock_alpha = self.layers[li].lock_alpha;
        let hd = TILE_SIZE as f32 * 0.5 * std::f32::consts::SQRT_2;
        let half = TILE_SIZE as f32 * 0.5;
        self.begin_op();
        for ty in 0..(h + TILE_SIZE as i32 - 1) / TILE_SIZE as i32 {
            for tx in 0..(w + TILE_SIZE as i32 - 1) / TILE_SIZE as i32 {
                let idx = TileIdx::new(tx, ty);
                if let Some(s) = &sel {
                    if s.tile_mask(idx).is_none() {
                        continue;
                    }
                }
                let (ox, oy) = idx.origin();
                // Cull BEFORE `tile_mut`, as the two-line form does.
                let win = field.window([ox as f32 + half, oy as f32 + half], hd);
                let tile = self.layers[li].tile_mut(idx);
                let data = tile.data_mut();
                for p in 0..TILE_SIZE * TILE_SIZE {
                    let x = ox + (p % TILE_SIZE) as i32;
                    let y = oy + (p / TILE_SIZE) as i32;
                    if x >= w || y >= h {
                        continue;
                    }
                    let mut s = win.colour_at([x as f32 + 0.5, y as f32 + 0.5], mix, bright);
                    if dither {
                        // No ramp parameter to be "inside" of — the whole
                        // field is interior, so the noise applies flat.
                        let d = crate::gradient::dither_offset(x, y) / 255.0;
                        for v in s.iter_mut() {
                            *v = (*v + d).clamp(0.0, 1.0);
                        }
                    }
                    // Premultiply, for the reason spelled out in
                    // `paint_gradient_ramp`.
                    let al = crate::blend::f32_to_fix15(s[3]);
                    let pr = |v: f32| crate::blend::f32_to_fix15(v * s[3]).min(al);
                    let c = [pr(s[0]), pr(s[1]), pr(s[2]), al];
                    Self::over_pixel(&mut data[p * 4..p * 4 + 4], c);
                }
            }
        }
        self.mask_op_to_selection();
        if lock_alpha {
            self.mask_op_to_alpha();
        }
        self.end_op();
        true
    }

    /// Fill a closed polygon on the active layer (even-odd scanline, canvas
    /// px, anti-aliasing by 2x2 subsampling), src-over. Used by the Figure
    /// tool's "fill shape" option. One undo step; false on a refusing layer.
    pub fn fill_polygon(&mut self, pts: &[[f32; 2]], color: [f32; 3], alpha: f32) -> bool {
        if pts.len() < 3 || !self.paint_guard() {
            return false;
        }
        let mut bb = [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ];
        for p in pts {
            bb[0] = bb[0].min(p[0]);
            bb[1] = bb[1].min(p[1]);
            bb[2] = bb[2].max(p[0]);
            bb[3] = bb[3].max(p[1]);
        }
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        let inside = |x: f32, y: f32| -> bool {
            // Even-odd crossing test.
            let mut inside = false;
            let n = pts.len();
            let mut j = n - 1;
            for i in 0..n {
                let pi = pts[i];
                let pj = pts[j];
                if (pi[1] > y) != (pj[1] > y) {
                    let xint = pj[0] + (y - pj[1]) * (pi[0] - pj[0]) / (pi[1] - pj[1]);
                    if x < xint {
                        inside = !inside;
                    }
                }
                j = i;
            }
            inside
        };
        let sel = self.selection.clone();
        let li = self.active;
        let lock_alpha = self.layers[li].lock_alpha;
        let base = [
            crate::blend::f32_to_fix15(color[0]),
            crate::blend::f32_to_fix15(color[1]),
            crate::blend::f32_to_fix15(color[2]),
            crate::blend::f32_to_fix15(alpha),
        ];
        self.begin_op();
        let t0x = (bb[0].floor().max(0.0) as i32 / TILE_SIZE as i32).max(0);
        let t0y = (bb[1].floor().max(0.0) as i32 / TILE_SIZE as i32).max(0);
        let t1x = (((bb[2].ceil().min(w as f32) as i32) + TILE_SIZE as i32 - 1) / TILE_SIZE as i32)
            .min((w + TILE_SIZE as i32 - 1) / TILE_SIZE as i32);
        let t1y = (((bb[3].ceil().min(h as f32) as i32) + TILE_SIZE as i32 - 1) / TILE_SIZE as i32)
            .min((h + TILE_SIZE as i32 - 1) / TILE_SIZE as i32);
        for ty in t0y..t1y {
            for tx in t0x..t1x {
                let idx = TileIdx::new(tx, ty);
                if let Some(s) = &sel {
                    if s.tile_mask(idx).is_none() {
                        continue;
                    }
                }
                let (ox, oy) = idx.origin();
                let tile = self.layers[li].tile_mut(idx);
                let data = tile.data_mut();
                for p in 0..TILE_SIZE * TILE_SIZE {
                    let x = ox + (p % TILE_SIZE) as i32;
                    let y = oy + (p / TILE_SIZE) as i32;
                    if x >= w || y >= h {
                        continue;
                    }
                    let fx = x as f32;
                    let fy = y as f32;
                    if fx + 1.0 < bb[0] || fx > bb[2] || fy + 1.0 < bb[1] || fy > bb[3] {
                        continue;
                    }
                    let cov = [
                        inside(fx + 0.25, fy + 0.25),
                        inside(fx + 0.75, fy + 0.25),
                        inside(fx + 0.25, fy + 0.75),
                        inside(fx + 0.75, fy + 0.75),
                    ]
                    .iter()
                    .filter(|&&c| c)
                    .count() as f32
                        / 4.0;
                    if cov <= 0.0 {
                        continue;
                    }
                    let mut c = base;
                    c[3] = crate::blend::f32_to_fix15(alpha * cov);
                    c[0] = (c[0] as f32 * cov) as u16;
                    c[1] = (c[1] as f32 * cov) as u16;
                    c[2] = (c[2] as f32 * cov) as u16;
                    Self::over_pixel(&mut data[p * 4..p * 4 + 4], c);
                }
            }
        }
        self.mask_op_to_selection();
        if lock_alpha {
            self.mask_op_to_alpha();
        }
        self.end_op();
        true
    }
}
