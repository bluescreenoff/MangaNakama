//! Selection: a sparse 8-bit coverage mask on the tile grid, plus the outline
//! polygon the UI draws.
//!
//! Semantics follow CSP: **no selection = everything selectable**. A selection
//! never blocks input directly — the brush paints freely and
//! [`Document::mask_op_to_selection`] clamps the open op's changes back to the
//! mask (out = old·(1−m) + new·m), using the undo pre-images that are being
//! recorded anyway. That keeps the brush engine 100% selection-unaware.

use std::collections::HashMap;

use crate::doc::Document;
use crate::tile::{TILE_SIZE, TileIdx};

type Mask = Box<[u8; TILE_SIZE * TILE_SIZE]>;

/// The one "is this pixel selected" predicate: ≥ half coverage. The
/// PREDICATE consumers — boolean ops, invert/grow/shrink, combine, the
/// Select tool's inside hit-test — go through here. The WEIGHT consumers
/// (paint/fill masking, `move_selected`, the transform lift + Cut clear,
/// which take each pixel's `cov/255` fraction) read raw coverage and must
/// NOT be unified onto this threshold — feathered edges (SE-007 blur)
/// live in the weights (DECISIONS 8.73). (Audit rounds 36–48: the
/// threshold was split `>= 128` vs `> 0`, which agree only while every
/// construction site writes binary 0/255.)
pub const SEL_ON: u8 = 128;

#[inline]
pub fn selected(coverage: u8) -> bool {
    coverage >= SEL_ON
}

#[derive(Clone, Debug, Default)]
pub struct Selection {
    tiles: HashMap<TileIdx, Mask>,
    /// Closed outline in canvas px, for display only (dashed line in the UI).
    pub outline: Vec<(f32, f32)>,
    /// Additional closed loops (a mask-built selection can have holes and
    /// islands). Display only, like `outline`.
    pub extra_outlines: Vec<Vec<(f32, f32)>>,
}

/// Boundary loops of a painted mask field (the live preview for a
/// selection-paint stroke — bbox trace over the SPARSE tiles, so cost
/// tracks the stroke, not the canvas).
pub fn scratch_outlines(m: &crate::doc::LayerMask) -> Vec<Vec<(f32, f32)>> {
    if m.tiles.is_empty() {
        return Vec::new();
    }
    let dense: HashMap<TileIdx, Box<[u8; TILE_SIZE * TILE_SIZE]>> = m
        .tiles
        .iter()
        .map(|(i, t)| {
            let mut cov = Box::new([0u8; TILE_SIZE * TILE_SIZE]);
            for (d, px) in cov.iter_mut().zip(t.data().chunks_exact(4)) {
                *d = ((px[3] as u32 * 255 + 16384) >> 15).min(255) as u8;
            }
            (*i, cov)
        })
        .collect();
    let (x0, y0, w, h, region) = sparse_region(&dense, |v| selected(v));
    if region.is_empty() {
        return Vec::new();
    }
    shift_loops(trace_outlines(&region, w, h), x0, y0)
}

/// Bbox bool region over sparse per-tile coverage. Empty region = the
/// bbox was absurd (a guard against two speckles in opposite corners
/// allocating a canvas-sized buffer for a preview).
fn sparse_region(
    tiles: &HashMap<TileIdx, Box<[u8; TILE_SIZE * TILE_SIZE]>>,
    on: impl Fn(u8) -> bool,
) -> (i64, i64, usize, usize, Vec<bool>) {
    let mut any = false;
    let (mut x0, mut y0, mut x1, mut y1) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
    for (idx, t) in tiles {
        if !t.iter().any(|&v| on(v)) {
            continue;
        }
        let (ox, oy) = idx.origin();
        any = true;
        x0 = x0.min(ox as i64);
        y0 = y0.min(oy as i64);
        x1 = x1.max(ox as i64 + TILE_SIZE as i64);
        y1 = y1.max(oy as i64 + TILE_SIZE as i64);
    }
    let (w, h) = if any {
        ((x1 - x0) as usize, (y1 - y0) as usize)
    } else {
        (0, 0)
    };
    // The preview cap: 64 M px of bool is already generous; beyond that,
    // no live trace (the commit still traces through the same guard).
    if w == 0 || h == 0 || w.saturating_mul(h) > 64_000_000 {
        return (0, 0, 0, 0, Vec::new());
    }
    let mut region = vec![false; w * h];
    for (idx, t) in tiles {
        if !t.iter().any(|&v| on(v)) {
            continue;
        }
        let (ox, oy) = idx.origin();
        let (bx, by) = ((ox as i64 - x0) as usize, (oy as i64 - y0) as usize);
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                if on(t[y * TILE_SIZE + x]) {
                    region[(by + y) * w + bx + x] = true;
                }
            }
        }
    }
    (x0, y0, w, h, region)
}

fn shift_loops(loops: Vec<Vec<(f32, f32)>>, x0: i64, y0: i64) -> Vec<Vec<(f32, f32)>> {
    loops
        .into_iter()
        .map(|l| {
            l.into_iter()
                .map(|(x, y)| (x + x0 as f32, y + y0 as f32))
                .collect()
        })
        .collect()
}

impl Selection {
    /// Axis-aligned rectangle, any corner order, clipped to the document.
    pub fn from_rect(doc: &Document, ax: f32, ay: f32, bx: f32, by: f32) -> Self {
        let (x0, x1) = (ax.min(bx), ax.max(bx));
        let (y0, y1) = (ay.min(by), ay.max(by));
        let poly = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
        Self::from_polygon(doc, &poly)
    }

    /// Even-odd scanline fill of a closed polygon (the lasso), clipped to the
    /// document. Hard edges — CSP-style anti-aliased selection can come later.
    pub fn from_polygon(doc: &Document, pts: &[(f32, f32)]) -> Self {
        let mut sel = Selection {
            tiles: HashMap::new(),
            outline: pts.to_vec(),
            extra_outlines: Vec::new(),
        };
        if pts.len() < 3 {
            return sel;
        }
        let (w, h) = (doc.size.0 as i32, doc.size.1 as i32);
        let y_min = pts
            .iter()
            .map(|p| p.1)
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as i32;
        let y_max = pts
            .iter()
            .map(|p| p.1)
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min(h as f32) as i32;

        let mut xs: Vec<f32> = Vec::new();
        for y in y_min..y_max {
            // Sample the scanline at the pixel centre.
            let yc = y as f32 + 0.5;
            xs.clear();
            for i in 0..pts.len() {
                let (x0, y0) = pts[i];
                let (x1, y1) = pts[(i + 1) % pts.len()];
                if (y0 <= yc) != (y1 <= yc) {
                    xs.push(x0 + (yc - y0) / (y1 - y0) * (x1 - x0));
                }
            }
            xs.sort_by(|a, b| a.total_cmp(b));
            for pair in xs.chunks_exact(2) {
                let sx = pair[0].round().max(0.0) as i32;
                let ex = pair[1].round().min(w as f32) as i32;
                for x in sx..ex {
                    sel.set(x, y, 255);
                }
            }
        }
        sel
    }

    fn set(&mut self, x: i32, y: i32, v: u8) {
        let idx = TileIdx::of_pixel(x, y);
        let (ox, oy) = idx.origin();
        let mask = self.tiles.entry(idx).or_insert_with(|| {
            vec![0u8; TILE_SIZE * TILE_SIZE]
                .into_boxed_slice()
                .try_into()
                .unwrap()
        });
        mask[(y - oy) as usize * TILE_SIZE + (x - ox) as usize] = v;
    }

    /// Coverage at a canvas pixel, 0..255.
    pub fn coverage(&self, x: i32, y: i32) -> u8 {
        let idx = TileIdx::of_pixel(x, y);
        let Some(m) = self.tiles.get(&idx) else {
            return 0;
        };
        let (ox, oy) = idx.origin();
        m[(y - oy) as usize * TILE_SIZE + (x - ox) as usize]
    }

    pub fn tile_mask(&self, t: TileIdx) -> Option<&[u8; TILE_SIZE * TILE_SIZE]> {
        self.tiles.get(&t).map(|b| &**b)
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// The operand rect for Cut/Copy/Transform/Crop, `[x0, y0, x1, y1]`
    /// with an EXCLUSIVE max — read off the COVERAGE, never `outline`.
    /// The display loops keep one arbitrary island in `outline` and the
    /// rest in `extra_outlines`, and a feathered selection can have no
    /// loop at all, so an outline bbox silently operates on part of the
    /// mask (or none of it). Sub-`SEL_ON` coverage still has bounds — a
    /// blur that lands entirely under half is invisible but active, and
    /// the weight consumers must still be able to reach its pixels.
    pub fn bounds(&self) -> Option<[i32; 4]> {
        self.bounds_where(selected)
            .or_else(|| self.bounds_where(|v| v > 0))
    }

    /// Whether anything reaches [`SEL_ON`], i.e. whether `retrace` can
    /// draw ants at all.
    pub fn has_visible_outline(&self) -> bool {
        self.bounds_where(selected).is_some()
    }

    fn bounds_where(&self, on: impl Fn(u8) -> bool) -> Option<[i32; 4]> {
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for (idx, m) in &self.tiles {
            let (ox, oy) = idx.origin();
            for y in 0..TILE_SIZE {
                for x in 0..TILE_SIZE {
                    if on(m[y * TILE_SIZE + x]) {
                        let (px, py) = (ox + x as i32, oy + y as i32);
                        x0 = x0.min(px);
                        y0 = y0.min(py);
                        x1 = x1.max(px + 1);
                        y1 = y1.max(py + 1);
                    }
                }
            }
        }
        (x0 < x1).then_some([x0, y0, x1, y1])
    }

    /// The selection-paint stroke's landing: coverage from a MASK FIELD
    /// the brush engine painted (selection pen / eraser / Quick Mask —
    /// alpha is the payload, KEPT as graduated u8 so a soft brush makes
    /// a soft selection, unlike [`Self::from_layer_alpha`]'s binary cut).
    pub fn from_mask_field(_doc: &Document, m: &crate::doc::LayerMask) -> Self {
        let mut sel = Selection::default();
        for (idx, t) in &m.tiles {
            let mut cov = Box::new([0u8; TILE_SIZE * TILE_SIZE]);
            for (d, px) in cov.iter_mut().zip(t.data().chunks_exact(4)) {
                *d = ((px[3] as u32 * 255 + 16384) >> 15).min(255) as u8;
            }
            sel.tiles.insert(*idx, cov);
        }
        let loops = scratch_outlines(m);
        sel.outline = loops.first().cloned().unwrap_or_default();
        sel.extra_outlines = if loops.is_empty() {
            Vec::new()
        } else {
            loops[1..].to_vec()
        };
        sel
    }

    /// Re-derive the display outlines from the coverage (sparse bbox
    /// trace — cheap while the coverage is stroke-local).
    pub fn retrace(&mut self) {
        let mut loops = Vec::new();
        if !self.tiles.is_empty() {
            let (x0, y0, w, h, region) = sparse_region(&self.tiles, |v| selected(v));
            if !region.is_empty() {
                loops = shift_loops(trace_outlines(&region, w, h), x0, y0);
            }
        }
        self.outline = loops.first().cloned().unwrap_or_default();
        self.extra_outlines = if loops.is_empty() {
            Vec::new()
        } else {
            loops[1..].to_vec()
        };
    }

    /// SE-011: the selection from a layer's ALPHA — Ctrl+click a layer
    /// row, the most-used selection action in any layered app. The same
    /// ≥ half coverage threshold as every bool op here; tone layers
    /// select their displayed raster (`display_tiles`).
    pub fn from_layer_alpha(doc: &Document, index: usize) -> Self {
        let (w, h) = (doc.size.0 as usize, doc.size.1 as usize);
        let mut region = vec![false; w * h];
        if let Some(layer) = doc.layers.get(index) {
            for (idx, tile) in layer.display_tiles() {
                let (ox, oy) = idx.origin();
                for y in 0..TILE_SIZE {
                    let cy = oy as i64 + y as i64;
                    if cy < 0 || cy >= h as i64 {
                        continue;
                    }
                    for x in 0..TILE_SIZE {
                        let cx = ox as i64 + x as i64;
                        if cx < 0 || cx >= w as i64 {
                            continue;
                        }
                        region[cy as usize * w + cx as usize] = tile.pixel(x, y)[3] >= 16384;
                    }
                }
            }
        }
        Self::from_mask(doc, &region, w)
    }

    /// Full-canvas selection (Ctrl+A).
    pub fn all(doc: &Document) -> Self {
        Self::from_rect(doc, 0.0, 0.0, doc.size.0 as f32, doc.size.1 as f32)
    }

    /// Build from a row-major bool region (`w`×`h`, canvas-sized). Boundary
    /// loops are traced for the dashed display.
    pub fn from_mask(doc: &Document, region: &[bool], w: usize) -> Self {
        let h = if w == 0 { 0 } else { region.len() / w };
        let mut sel = Selection::default();
        for y in 0..h {
            for x in 0..w {
                if region[y * w + x] {
                    sel.set(x as i32, y as i32, 255);
                }
            }
        }
        let _ = doc;
        sel.set_outlines(trace_outlines(region, w, h));
        sel
    }

    /// The inverse: everything on the canvas this selection does not cover.
    /// Fully-uncovered tiles stay sparse; the outline is re-traced.
    pub fn inverted(&self, doc: &Document) -> Self {
        let (w, h) = (doc.size.0 as usize, doc.size.1 as usize);
        let mut region = vec![false; w * h];
        for y in 0..h {
            for x in 0..w {
                region[y * w + x] = !selected(self.coverage(x as i32, y as i32));
            }
        }
        Self::from_mask(doc, &region, w)
    }

    /// Grow (dilate) the selection outward by `px` px (CSP 選択範囲の拡張).
    /// A box dilation, run as two prefix-sum window passes (horizontal, then
    /// vertical) so the cost is linear in pixels regardless of `px`. The
    /// `selected` predicate decides membership; outlines re-trace.
    pub fn grow(&self, doc: &Document, px: u32) -> Self {
        let (w, h) = (doc.size.0 as usize, doc.size.1 as usize);
        if w == 0 || h == 0 || px == 0 || self.is_empty() {
            return self.clone();
        }
        let px = px.min(w.max(h) as u32) as i64;
        let mut cur = vec![false; w * h];
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                cur[y as usize * w + x as usize] = selected(self.coverage(x, y));
            }
        }
        let mut tmp = vec![false; w * h];
        let mut out = vec![false; w * h];
        let mut prefix = vec![0u32; w.max(h) + 1];
        // Horizontal window-OR into tmp.
        for y in 0..h {
            prefix[0] = 0;
            for x in 0..w {
                prefix[x + 1] = prefix[x] + cur[y * w + x] as u32;
            }
            for x in 0..w {
                let lo = x.saturating_sub(px as usize);
                let hi = (x + px as usize + 1).min(w);
                tmp[y * w + x] = prefix[hi] > prefix[lo];
            }
        }
        // Vertical window-OR into out.
        for x in 0..w {
            prefix[0] = 0;
            for y in 0..h {
                prefix[y + 1] = prefix[y] + tmp[y * w + x] as u32;
            }
            for y in 0..h {
                let lo = y.saturating_sub(px as usize);
                let hi = (y + px as usize + 1).min(h);
                out[y * w + x] = prefix[hi] > prefix[lo];
            }
        }
        Self::from_mask(doc, &out, w)
    }

    /// Shrink (erode) by `px` px (CSP 選択範囲の縮小): erosion is dilation
    /// of the complement.
    pub fn shrink(&self, doc: &Document, px: u32) -> Self {
        if px == 0 || self.is_empty() {
            return self.clone();
        }
        self.inverted(doc).grow(doc, px).inverted(doc)
    }

    /// SE-007 Blur border (CSP 選択範囲の境界をぼかす): a box blur over
    /// the GRADUATED u8 coverage — the first construct that makes a
    /// feathered selection (soft brush ⇒ soft selection already could,
    /// r97; this softens any selection's edge). The paint/fill paths read
    /// raw coverage as weight, so a blurred edge feathers everything
    /// downstream; the transform lift/clear pair reads the same weights
    /// since r107 (DECISIONS 8.73 — it was boolean before that).
    /// Separable (horizontal then vertical window means) so the cost is
    /// linear in pixels regardless of `px`; outlines re-trace on the
    /// ≥-half view.
    pub fn blur(&self, doc: &Document, px: u32) -> Self {
        let (w, h) = (doc.size.0 as usize, doc.size.1 as usize);
        if w == 0 || h == 0 || px == 0 || self.is_empty() {
            return self.clone();
        }
        let r = px.min(w.max(h) as u32) as usize;
        let mut cur = vec![0u8; w * h];
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                cur[y as usize * w + x as usize] = self.coverage(x, y);
            }
        }
        let mut tmp = vec![0u8; w * h];
        let mut out = vec![0u8; w * h];
        let mut prefix = vec![0u32; w.max(h) + 1];
        // Horizontal window mean into tmp (edge-clamped windows).
        for y in 0..h {
            prefix[0] = 0;
            for x in 0..w {
                prefix[x + 1] = prefix[x] + cur[y * w + x] as u32;
            }
            for x in 0..w {
                let lo = x.saturating_sub(r);
                let hi = (x + r + 1).min(w);
                let n = (hi - lo) as u32;
                tmp[y * w + x] = ((prefix[hi] - prefix[lo] + n / 2) / n).min(255) as u8;
            }
        }
        // Vertical window mean into out.
        for x in 0..w {
            prefix[0] = 0;
            for y in 0..h {
                prefix[y + 1] = prefix[y] + tmp[y * w + x] as u32;
            }
            for y in 0..h {
                let lo = y.saturating_sub(r);
                let hi = (y + r + 1).min(h);
                let n = (hi - lo) as u32;
                out[y * w + x] = ((prefix[hi] - prefix[lo] + n / 2) / n).min(255) as u8;
            }
        }
        let mut sel = Selection::default();
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let v = out[y as usize * w + x as usize];
                if v != 0 {
                    sel.set(x, y, v);
                }
            }
        }
        sel.retrace();
        sel
    }

    /// Combine with `next` under a boolean op (SE-022 / the owner's
    /// everyday path: Shift = Add, Alt = Subtract, Shift+Alt = Intersect,
    /// no modifier = Replace). The `selected` predicate decides membership;
    /// outlines re-trace, so the ants always match the mask.
    pub fn combine(&self, next: &Selection, doc: &Document, op: SelectionOp) -> Selection {
        if op == SelectionOp::Replace {
            return next.clone();
        }
        let (w, h) = (doc.size.0 as usize, doc.size.1 as usize);
        let mut region = vec![false; w * h];
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let a = selected(self.coverage(x, y));
                let b = selected(next.coverage(x, y));
                region[y as usize * w + x as usize] = match op {
                    SelectionOp::Add => a || b,
                    SelectionOp::Subtract => a && !b,
                    SelectionOp::Intersect => a && b,
                    SelectionOp::Replace => unreachable!(),
                };
            }
        }
        Self::from_mask(doc, &region, w)
    }

    fn set_outlines(&mut self, mut loops: Vec<Vec<(f32, f32)>>) {
        loops.sort_by_key(|l| std::cmp::Reverse(l.len()));
        self.outline = loops.first().cloned().unwrap_or_default();
        self.extra_outlines = if loops.len() > 1 {
            loops.split_off(1)
        } else {
            Vec::new()
        };
    }

    /// Translate the whole selection (mask + outline) by whole pixels.
    pub fn translate(&mut self, dx: i32, dy: i32) {
        for p in self
            .outline
            .iter_mut()
            .chain(self.extra_outlines.iter_mut().flatten())
        {
            p.0 += dx as f32;
            p.1 += dy as f32;
        }
        // Pixel-granular remap: cheap enough at selection sizes, and correct
        // for non-tile-aligned deltas.
        let old = std::mem::take(&mut self.tiles);
        for (idx, mask) in old {
            let (ox, oy) = idx.origin();
            for y in 0..TILE_SIZE {
                for x in 0..TILE_SIZE {
                    let v = mask[y * TILE_SIZE + x];
                    if v != 0 {
                        self.set(ox + x as i32 + dx, oy + y as i32 + dy, v);
                    }
                }
            }
        }
    }
}

/// How a new selection shape combines with the current one. The owner's
/// spec (2026-08-17 evening): "the everyday path is Shift = add, Alt =
/// subtract, Shift+Alt = intersect, plus the same four as a persistent
/// mode in Tool Settings" — a held modifier OVERRIDES the persistent mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionOp {
    Replace,
    Add,
    Subtract,
    Intersect,
}

/// Trace the boundary of a bool region into closed loops of pixel-corner
/// points (display only — coverage is the truth). Collinear runs collapse.
fn trace_outlines(region: &[bool], w: usize, h: usize) -> Vec<Vec<(f32, f32)>> {
    use std::collections::HashMap as Map;
    let at = |x: i64, y: i64| -> bool {
        x >= 0
            && y >= 0
            && (x as usize) < w
            && (y as usize) < h
            && region[y as usize * w + x as usize]
    };
    // Directed edges around every inside pixel, counter-clockwise in screen
    // space, so loops chain consistently: outside above → edge runs right,
    // below → left, left → down... keyed by start point.
    let mut edges: Map<(i32, i32), Vec<(i32, i32)>> = Map::new();
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            if !at(x, y) {
                continue;
            }
            let (xi, yi) = (x as i32, y as i32);
            if !at(x, y - 1) {
                edges.entry((xi, yi)).or_default().push((xi + 1, yi));
            }
            if !at(x, y + 1) {
                edges
                    .entry((xi + 1, yi + 1))
                    .or_default()
                    .push((xi, yi + 1));
            }
            if !at(x - 1, y) {
                edges.entry((xi, yi + 1)).or_default().push((xi, yi));
            }
            if !at(x + 1, y) {
                edges
                    .entry((xi + 1, yi))
                    .or_default()
                    .push((xi + 1, yi + 1));
            }
        }
    }
    let mut loops = Vec::new();
    while let Some((&start, _)) = edges.iter().next() {
        let mut pts: Vec<(i32, i32)> = vec![start];
        let mut cur = start;
        loop {
            let Some(nexts) = edges.get_mut(&cur) else {
                break;
            };
            let Some(next) = nexts.pop() else {
                edges.remove(&cur);
                break;
            };
            if nexts.is_empty() {
                edges.remove(&cur);
            }
            if next == start {
                break;
            }
            pts.push(next);
            cur = next;
        }
        if pts.len() >= 3 {
            // Drop collinear midpoints (long straight runs become 2 points).
            let n = pts.len();
            let mut out: Vec<(f32, f32)> = Vec::new();
            for i in 0..n {
                let p = pts[i];
                let a = pts[(i + n - 1) % n];
                let b = pts[(i + 1) % n];
                let col = (a.0 == p.0 && p.0 == b.0) || (a.1 == p.1 && p.1 == b.1);
                if !col {
                    out.push((p.0 as f32, p.1 as f32));
                }
            }
            if out.len() >= 3 {
                loops.push(out);
            }
        }
    }
    loops
}

impl Document {
    /// Fill the selection (or the whole layer without one) with an opaque
    /// colour on the active layer, as one undo step (CSP Alt+Delete). Returns
    /// false when the active layer refuses (vector/folder/locked).
    pub fn fill_selection(&mut self, color: [f32; 3]) -> bool {
        let l = self.active_layer();
        if !l.paintable() || l.lock {
            return false;
        }
        let px: [u16; 4] = [
            crate::blend::f32_to_fix15(color[0]),
            crate::blend::f32_to_fix15(color[1]),
            crate::blend::f32_to_fix15(color[2]),
            crate::blend::f32_to_fix15(1.0),
        ];
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        let sel = self.selection.clone();
        self.begin_op();
        let li = self.active;
        let lock_alpha = self.layers[li].lock_alpha;
        let t = TILE_SIZE as i32;
        for ty in 0..(h + t - 1) / t {
            for tx in 0..(w + t - 1) / t {
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
                    let (x, y) = (ox + (p % TILE_SIZE) as i32, oy + (p / TILE_SIZE) as i32);
                    if x >= w || y >= h {
                        continue;
                    }
                    data[p * 4..p * 4 + 4].copy_from_slice(&px);
                }
            }
        }
        // The op-mask restores everything outside the selection (and blends
        // partial coverage); alpha lock clamps once, like a stroke.
        self.mask_op_to_selection();
        if lock_alpha {
            self.mask_op_to_alpha();
        }
        self.end_op();
        true
    }

    /// Clear everything the selection does NOT cover on the active layer
    /// (CSP Shift+Delete), one undo step. No-op without a selection.
    pub fn clear_outside_selection(&mut self) -> bool {
        let Some(sel) = self.selection.clone() else {
            return false;
        };
        let l = self.active_layer();
        if !l.paintable() || l.lock {
            return false;
        }
        self.begin_op();
        let li = self.active;
        let touched: Vec<TileIdx> = self.layers[li].tiles().map(|(i, _)| i).collect();
        for idx in touched {
            let mask = sel.tile_mask(idx);
            let tile = self.layers[li].tile_mut(idx);
            let data = tile.data_mut();
            match mask {
                None => data.fill(0),
                Some(m) => {
                    for p in 0..TILE_SIZE * TILE_SIZE {
                        let mv = m[p] as u32;
                        if mv == 255 {
                            continue;
                        }
                        for c in 0..4 {
                            let i = p * 4 + c;
                            data[i] = ((data[i] as u32 * mv + 127) / 255) as u16;
                        }
                    }
                }
            }
        }
        self.end_op();
        true
    }

    /// Clamp the open op's changes on its layer back to the selection:
    /// `out = old·(1−m) + new·m` per pixel, using the op's recorded pre-images.
    /// No-op without an open op or without a selection. Call after each sample
    /// batch (cheap: only touched tiles) and before `end_op`.
    pub fn mask_op_to_selection(&mut self) {
        let Some(li) = self.op_layer_index() else {
            return;
        };
        let Some(sel) = self.selection.clone() else {
            return;
        };
        let layer = &mut self.layers[li];
        let touched: Vec<TileIdx> = layer.recorded_tiles();
        for idx in touched {
            let mask = sel.tile_mask(idx);
            let pre = layer.recorded_pre_image(idx);
            match (mask, pre) {
                // Fully outside the selection: restore the pre-image wholesale.
                (None, Some(old)) => {
                    let old = old.clone();
                    layer.tile_mut(idx).data_mut().copy_from_slice(old.data());
                }
                (None, None) => {
                    // Tile did not exist and is not selected: wipe it.
                    for v in layer.tile_mut(idx).data_mut() {
                        *v = 0;
                    }
                }
                (Some(m), pre) => {
                    let old: Option<std::sync::Arc<crate::tile::Tile>> = pre.cloned();
                    let t = layer.tile_mut(idx);
                    let data = t.data_mut();
                    for p in 0..TILE_SIZE * TILE_SIZE {
                        let mv = m[p] as u32;
                        if mv == 255 {
                            continue;
                        }
                        for c in 0..4 {
                            let i = p * 4 + c;
                            let new = data[i] as u32;
                            let oldv = old.as_ref().map(|o| o.data()[i] as u32).unwrap_or(0);
                            data[i] = ((new * mv + oldv * (255 - mv) + 127) / 255) as u16;
                        }
                    }
                }
            }
        }
    }

    /// Move the selected pixels of the active layer by whole pixels, inside an
    /// undo op, and translate the selection with them. The uncovered area goes
    /// transparent; the moved pixels composite source-over at the destination.
    pub fn move_selected(&mut self, dx: i32, dy: i32) {
        if dx == 0 && dy == 0 {
            return;
        }
        let Some(sel) = self.selection.clone() else {
            return;
        };
        if sel.is_empty() {
            return;
        }
        self.begin_op();
        let li = self.active;
        // 1. Cut: collect the covered fraction of every selected pixel.
        //    (px, py, premultiplied fix15 rgba)
        let mut cut: Vec<(i32, i32, [u16; 4])> = Vec::new();
        {
            let layer = &mut self.layers[li];
            let sel_tiles: Vec<TileIdx> = layer
                .tiles()
                .map(|(i, _)| i)
                .filter(|i| sel.tile_mask(*i).is_some())
                .collect();
            for idx in sel_tiles {
                let Some(m) = sel.tile_mask(idx) else {
                    continue;
                };
                let (ox, oy) = idx.origin();
                let t = layer.tile_mut(idx);
                let data = t.data_mut();
                for p in 0..TILE_SIZE * TILE_SIZE {
                    let mv = m[p] as u32;
                    if mv == 0 {
                        continue;
                    }
                    let mut px = [0u16; 4];
                    let mut any = false;
                    for c in 0..4 {
                        let i = p * 4 + c;
                        let v = data[i] as u32;
                        let taken = (v * mv + 127) / 255;
                        px[c] = taken as u16;
                        data[i] = (v - taken) as u16;
                        any |= taken != 0;
                    }
                    if any {
                        cut.push((ox + (p % TILE_SIZE) as i32, oy + (p / TILE_SIZE) as i32, px));
                    }
                }
            }
        }
        // 2. Paste source-over at the destination (clipped to the canvas).
        {
            let (w, h) = (self.size.0 as i32, self.size.1 as i32);
            let layer = &mut self.layers[li];
            for (x, y, src) in cut {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= w || ny >= h {
                    continue;
                }
                let idx = TileIdx::of_pixel(nx, ny);
                let (ox, oy) = idx.origin();
                let t = layer.tile_mut(idx);
                let i = ((ny - oy) as usize * TILE_SIZE + (nx - ox) as usize) * 4;
                let data = t.data_mut();
                // src-over in premultiplied fix15: d = s + d·(1 − s.a)
                let sa = src[3] as u32;
                for c in 0..4 {
                    let s = src[c] as u32;
                    let d = data[i + c] as u32;
                    data[i + c] = (s + (d * (32768 - sa) >> 15)) as u16;
                }
            }
        }
        self.end_op();
        let mut sel = sel;
        sel.translate(dx, dy);
        self.selection = Some(sel);
        self.touch();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::FIX15_ONE;

    const F1: u16 = FIX15_ONE as u16;

    /// The single-threshold contract (audit 36–48): partial coverage must
    /// read one way everywhere. `selected` is the predicate lift, the Cut
    /// clear and the Select tool share with the boolean ops — sub-half is
    /// OUT, half and above is IN. (Today every construction site writes
    /// binary 0/255; this pins the boundary for feathered masks.)
    #[test]
    fn partial_coverage_reads_one_way_everywhere() {
        let doc = Document::new(256, 256);
        let mut s = Selection::from_rect(&doc, 0.0, 0.0, 64.0, 64.0);
        s.set(70, 10, 1); // below half
        s.set(71, 10, 127); // still below half
        s.set(72, 10, 128); // exactly half — in
        s.set(73, 10, 255);
        assert!(!selected(0) && !selected(1) && !selected(127));
        assert!(selected(128) && selected(255));
        // inverted flips the same boundary.
        let inv = s.inverted(&doc);
        assert_eq!(inv.coverage(71, 10), 255, "127/255 was OUT, comes IN");
        assert_eq!(inv.coverage(72, 10), 0, "128/255 was IN, goes OUT");
        // Add with an empty selection = the same ≥-half view of `s`.
        let add = s.combine(&Selection::default(), &doc, SelectionOp::Add);
        assert_eq!(add.coverage(71, 10), 0);
        assert_eq!(add.coverage(72, 10), 255);
        // Grow dilates from the ≥-half view: the apron reaches the 128
        // pixel, and the 1/255 pixel was never a seed.
        let g = s.grow(&doc, 1);
        assert_eq!(g.coverage(71, 10), 255);
        assert_eq!(g.coverage(70, 10), 0);
    }

    #[test]
    fn rect_selection_covers_inside_not_outside() {
        let doc = Document::new(256, 256);
        let s = Selection::from_rect(&doc, 10.0, 10.0, 100.0, 50.0);
        assert_eq!(s.coverage(50, 30), 255);
        assert_eq!(s.coverage(5, 30), 0);
        assert_eq!(s.coverage(50, 60), 0);
        assert!(!s.is_empty());
    }

    #[test]
    fn grow_expands_by_px_and_retraces() {
        let doc = Document::new(256, 256);
        let s = Selection::from_rect(&doc, 50.0, 50.0, 60.0, 60.0);
        let g = s.grow(&doc, 2);
        assert!(g.coverage(48, 48) >= 128, "2px apron outside the old edge");
        assert!(g.coverage(47, 47) == 0, "no further than the apron");
        assert!(g.coverage(55, 55) >= 128);
        assert!(!g.outline.is_empty(), "outline re-traced for the ants");
    }

    #[test]
    fn shrink_erodes_edges() {
        let doc = Document::new(256, 256);
        let s = Selection::from_rect(&doc, 50.0, 50.0, 60.0, 60.0);
        let e = s.shrink(&doc, 2);
        assert!(e.coverage(50, 50) == 0, "edge eroded away");
        assert!(e.coverage(55, 55) >= 128, "deep interior survives");
    }

    #[test]
    fn shrink_can_empty_a_selection() {
        let doc = Document::new(256, 256);
        let s = Selection::from_rect(&doc, 50.0, 50.0, 56.0, 56.0); // ~6px wide
        let e = s.shrink(&doc, 8);
        assert!(e.is_empty(), "eroded past existence");
    }

    #[test]
    fn grow_shrink_round_trip_restores_interior() {
        let doc = Document::new(256, 256);
        let s = Selection::from_rect(&doc, 50.0, 50.0, 100.0, 100.0);
        let rt = s.grow(&doc, 3).shrink(&doc, 3);
        // Morphological opening: round trip keeps the interior, trims spikes.
        assert!(rt.coverage(75, 75) >= 128);
    }

    /// SE-007: blur feathers the edge with GRADUATED coverage — the
    /// interior stays opaque, a band on BOTH sides of the old edge goes
    /// partial, far outside stays zero, and the ants re-trace on the
    /// ≥-half view.
    #[test]
    fn blur_feathers_the_border() {
        let doc = Document::new(256, 256);
        let s = Selection::from_rect(&doc, 50.0, 50.0, 100.0, 100.0);
        let b = s.blur(&doc, 3);
        // Deep interior unchanged (window fully inside the 255 region).
        assert_eq!(b.coverage(75, 75), 255);
        // The feather band straddles the old edge (x=100): just inside
        // and just outside both go partial — ~4/7 and ~3/7 of 255.
        let ci = b.coverage(99, 75) as u32;
        let co = b.coverage(101, 75) as u32;
        assert!(
            (140..=190).contains(&ci),
            "just inside the edge is partial: {ci}"
        );
        assert!(
            (60..=90).contains(&co),
            "just outside the edge is partial: {co}"
        );
        assert!(ci > co, "the ramp falls off outward");
        // Far outside stays untouched.
        assert_eq!(b.coverage(120, 75), 0);
        // The ≥-half view (the ants, the boolean ops) sits between the
        // old edge and the new half-cover point.
        assert!(
            selected(b.coverage(99, 75)),
            "the half-cover line sits just inside the old edge"
        );
        assert!(!selected(b.coverage(104, 75)));
        // And the outlines re-traced (non-empty display loops).
        assert!(!b.outline.is_empty() || !b.extra_outlines.is_empty());
        // Blur(0) is a no-op clone.
        let z = s.blur(&doc, 0);
        assert_eq!(z.coverage(75, 75), 255);
        assert_eq!(z.coverage(101, 75), 0);
    }
    /// The operand rect for Cut/Copy/Transform/Crop must see EVERY
    /// island. `outline` holds one loop and `extra_outlines` the rest
    /// (the vertex-count sort in `set_outlines` picks which), so an
    /// outline bbox operates on one arbitrary island of a Shift-added
    /// selection.
    #[test]
    fn bounds_covers_every_island() {
        let doc = Document::new(256, 256);
        let a = Selection::from_rect(&doc, 10.0, 10.0, 30.0, 30.0);
        // An L: 6 corners against the rect's 4, so the loops sort
        // unequally and `outline` deterministically keeps the L.
        let b = Selection::from_polygon(
            &doc,
            &[
                (100.0, 100.0),
                (160.0, 100.0),
                (160.0, 120.0),
                (120.0, 120.0),
                (120.0, 160.0),
                (100.0, 160.0),
            ],
        );
        let both = a.combine(&b, &doc, SelectionOp::Add);
        assert!(!both.extra_outlines.is_empty(), "two islands, two loops");
        let bb = both.bounds().expect("a covered selection has bounds");
        assert!(bb[0] <= 10 && bb[1] <= 10, "island A is inside {bb:?}");
        assert!(bb[2] >= 160 && bb[3] >= 160, "island B is inside {bb:?}");
        // The one loop `outline` kept spans far less than the mask does.
        let (mut x0, mut x1) = (f32::INFINITY, f32::NEG_INFINITY);
        for &(x, _) in &both.outline {
            x0 = x0.min(x);
            x1 = x1.max(x);
        }
        assert!(
            x1 - x0 < (bb[2] - bb[0]) as f32,
            "one loop is not the selection's bbox"
        );
        assert!(Selection::default().bounds().is_none());
    }

    /// A wide blur can put every pixel under [`SEL_ON`]: no loops, no
    /// ants, no launcher — but the mask is live and still weights the
    /// brush, so `bounds` falls back to any coverage and the weight
    /// consumers can still reach it.
    #[test]
    fn bounds_survives_an_invisible_feather() {
        let doc = Document::new(256, 256);
        let s = Selection::from_rect(&doc, 100.0, 100.0, 140.0, 140.0);
        let b = s.blur(&doc, 32);
        assert!(
            b.outline.is_empty() && b.extra_outlines.is_empty(),
            "the ≥-half view is empty, so retrace found no loop"
        );
        assert!(!b.has_visible_outline());
        assert!(!b.is_empty(), "the coverage is still there");
        let bb = b.bounds().expect("the feather still has bounds");
        assert!(bb[0] < 100 && bb[2] > 140, "the feather's spread: {bb:?}");
    }

    #[test]
    fn lasso_triangle_covers_centroid_only() {
        let doc = Document::new(256, 256);
        let s = Selection::from_polygon(&doc, &[(10.0, 10.0), (200.0, 10.0), (10.0, 200.0)]);
        assert_eq!(s.coverage(50, 50), 255, "centroid-ish inside");
        assert_eq!(s.coverage(190, 190), 0, "opposite corner outside");
    }

    #[test]
    fn op_masking_confines_paint_to_the_selection() {
        let mut doc = Document::new(256, 256);
        doc.selection = Some(Selection::from_rect(&doc, 0.0, 0.0, 64.0, 64.0));
        doc.begin_op();
        // Paint two tiles: one inside the selection, one outside.
        doc.active_layer_mut()
            .tile_mut(TileIdx::new(0, 0))
            .set_pixel(5, 5, [F1, 0, 0, F1]);
        doc.active_layer_mut()
            .tile_mut(TileIdx::new(2, 2))
            .set_pixel(5, 5, [F1, 0, 0, F1]);
        doc.mask_op_to_selection();
        doc.end_op();
        let l = doc.active_layer();
        assert_eq!(l.tile(TileIdx::new(0, 0)).unwrap().pixel(5, 5)[3], F1);
        assert_eq!(
            l.tile(TileIdx::new(2, 2)).unwrap().pixel(5, 5)[3],
            0,
            "outside wiped"
        );
    }

    #[test]
    fn select_all_invert_and_from_mask() {
        let doc = Document::new(128, 128);
        let all = Selection::all(&doc);
        assert_eq!(all.coverage(0, 0), 255);
        assert_eq!(all.coverage(127, 127), 255);

        let s = Selection::from_rect(&doc, 0.0, 0.0, 64.0, 128.0);
        let inv = s.inverted(&doc);
        assert_eq!(inv.coverage(32, 64), 0, "was selected, now not");
        assert_eq!(inv.coverage(100, 64), 255, "was outside, now selected");

        // from_mask traces a display outline.
        let mut region = vec![false; 128 * 128];
        for y in 10..20 {
            for x in 10..30 {
                region[y * 128 + x] = true;
            }
        }
        let m = Selection::from_mask(&doc, &region, 128);
        assert_eq!(m.coverage(15, 15), 255);
        assert_eq!(m.coverage(50, 15), 0);
        assert_eq!(m.outline.len(), 4, "a rectangle collapses to 4 corners");
    }

    #[test]
    fn fill_selection_and_clear_outside_are_undoable() {
        let mut doc = Document::new(128, 128);
        // Some existing ink outside the future selection.
        doc.begin_op();
        doc.active_layer_mut()
            .tile_mut(TileIdx::new(1, 1))
            .set_pixel(10, 10, [0, 0, 0, F1]);
        doc.end_op();

        doc.selection = Some(Selection::from_rect(&doc, 0.0, 0.0, 64.0, 64.0));
        assert!(doc.fill_selection([1.0, 0.0, 0.0]));
        let px = |doc: &Document, t: (i32, i32), p: (usize, usize)| {
            doc.active_layer()
                .tile(TileIdx::new(t.0, t.1))
                .map(|tl| tl.pixel(p.0, p.1))
                .unwrap_or([0; 4])
        };
        assert_eq!(px(&doc, (0, 0), (5, 5))[0], F1, "inside filled red");
        assert_eq!(px(&doc, (1, 1), (10, 10)), [0, 0, 0, F1], "outside kept");
        assert!(doc.undo());
        assert_eq!(px(&doc, (0, 0), (5, 5))[3], 0, "fill undone");

        assert!(doc.clear_outside_selection());
        assert_eq!(px(&doc, (1, 1), (10, 10))[3], 0, "outside cleared");
        assert!(doc.undo());
        assert_eq!(px(&doc, (1, 1), (10, 10))[3], F1, "clear-outside undone");
    }

    #[test]
    fn move_selected_translates_pixels_and_is_undoable() {
        let mut doc = Document::new(256, 256);
        doc.begin_op();
        doc.active_layer_mut()
            .tile_mut(TileIdx::new(0, 0))
            .set_pixel(10, 10, [0, 0, 0, F1]);
        doc.end_op();

        doc.selection = Some(Selection::from_rect(&doc, 0.0, 0.0, 64.0, 64.0));
        doc.move_selected(100, 0);

        let l = doc.active_layer();
        assert_eq!(
            l.tile(TileIdx::new(0, 0)).unwrap().pixel(10, 10)[3],
            0,
            "cut"
        );
        assert_eq!(
            l.tile(TileIdx::new(1, 0)).unwrap().pixel(110 - 64, 10)[3],
            F1,
            "pasted at +100"
        );
        assert_eq!(
            doc.selection.as_ref().unwrap().coverage(110, 10),
            255,
            "selection moved too"
        );

        assert!(doc.undo());
        let l = doc.active_layer();
        assert_eq!(
            l.tile(TileIdx::new(0, 0)).unwrap().pixel(10, 10)[3],
            F1,
            "undo restores"
        );
    }
}

#[cfg(test)]
mod combine_tests {
    use super::*;

    /// SE-022 / the owner's everyday path: the four combine ops, on two
    /// overlapping rectangles — and the marching-ants outline always
    /// matches the mask (the traced loop stays inside the combined mask).
    #[test]
    fn combine_ops_four_ways() {
        let doc = Document::new(256, 256);
        let a = Selection::from_rect(&doc, 10.0, 10.0, 100.0, 100.0);
        let b = Selection::from_rect(&doc, 50.0, 50.0, 150.0, 150.0);

        let add = a.combine(&b, &doc, SelectionOp::Add);
        assert!(add.coverage(15, 15) > 0, "a-only pixel kept");
        assert!(add.coverage(120, 120) > 0, "b-only pixel kept");
        assert!(add.coverage(75, 75) > 0, "overlap kept");
        assert_eq!(add.coverage(200, 200), 0, "outside both stays out");

        let sub = a.combine(&b, &doc, SelectionOp::Subtract);
        assert!(sub.coverage(15, 15) > 0, "a-minus-b kept");
        assert_eq!(sub.coverage(75, 75), 0, "overlap removed");
        assert_eq!(sub.coverage(120, 120), 0, "b-only never was in a");

        let inter = a.combine(&b, &doc, SelectionOp::Intersect);
        assert!(inter.coverage(75, 75) > 0, "overlap kept");
        assert_eq!(inter.coverage(15, 15), 0, "a-only dropped");
        assert_eq!(inter.coverage(120, 120), 0, "b-only dropped");

        let rep = a.combine(&b, &doc, SelectionOp::Replace);
        assert_eq!(rep.coverage(15, 15), 0, "replace discards a");

        // Subtracting a SUPERSET empties (the caller deselects); a
        // disjoint shape leaves the selection untouched.
        let c = Selection::from_rect(&doc, 200.0, 200.0, 250.0, 250.0);
        let same = a.combine(&c, &doc, SelectionOp::Subtract);
        assert!(
            same.coverage(15, 15) > 0,
            "a disjoint subtract changes nothing"
        );
        let gone = a.combine(&Selection::all(&doc), &doc, SelectionOp::Subtract);
        assert!(gone.is_empty(), "subtracting a superset empties");

        // Outline re-trace: the ants' loop points sit on mask boundaries.
        assert!(
            !add.outline.is_empty(),
            "a combined selection retraces its outline"
        );
    }
}
