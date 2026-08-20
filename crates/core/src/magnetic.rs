//! Magnetic lasso (TRIAGE row 36, L-001/L-002): a selection outline that
//! snaps to the lineart while you trace it only roughly.
//!
//! The algorithm is the standard LIVE WIRE (Mortensen & Barrett): the page
//! becomes a cost image in which strong edges are cheap to walk along, and
//! every stretch of the outline is a Dijkstra shortest path from the last
//! anchor to the cursor. Two things make it survive a 6000x8600 manga page
//! rather than the 512x512 photo the paper was written for:
//!
//! * **The cost image is built lazily, one 64x64 block at a time**
//!   ([`EdgeField`]). Only the neighbourhood the trace actually passes
//!   through is ever composited and differentiated. A whole-page cost image
//!   is ~50 MB of work before the first pixel moves — i.e. the first click
//!   hangs, which is the failure everyone who writes this feature ships once.
//! * **Every search is confined to a window**: the bounding box of (last
//!   anchor, cursor) inflated by the tool's snap range, with a hard ceiling.
//!   So anchors are not a nicety — they are what keeps each search small AND
//!   what stops the path re-routing behind you when you move on.
//!
//! Manga is the *easy* case here. Hard black lineart on white paper gives a
//! Sobel response that saturates, so the wire has an unambiguous rail to run
//! along; where there is no edge at all the costs go flat and the wire
//! degenerates to a straight line, which is the honest answer rather than a
//! wrong snap.
//!
//! [`Lasso`] is the session: anchors, the committed wire behind them, and the
//! live wire ahead. It owns its [`EdgeField`], which is a SESSION cache —
//! build one when the lasso starts, drop it when the lasso ends. It does not
//! watch the document for edits, because within one traced outline there are
//! none.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use crate::doc::Document;
use crate::tile::{TILE_SIZE, TileIdx};

/// Edge-strength cache granularity. One tile wide, so building a block reads
/// at most 2x2 tiles per layer.
const BLOCK: usize = TILE_SIZE;
const BLOCK_PX: usize = BLOCK * BLOCK;

/// Cache ceiling: 4096 blocks x 4 KB = 16 MB. Tracing every edge of a full
/// 6000x8600 page would otherwise cache ~50 MB. Past the ceiling the cache is
/// dropped whole and refills as the trace continues — an LRU buys nothing
/// here, because a trace does not come back to where it has been.
const MAX_BLOCKS: usize = 4096;

/// Widest live-wire search window, per side. A search costs O(area), so this
/// is the hang guard: past it the wire stops snapping and runs straight.
const MAX_WINDOW: i32 = 512;

/// Cost floor per pixel step. At zero a strong edge would be FREE to walk,
/// and the wire would happily run a thousand px along one rather than cross
/// two px of paper; the floor keeps length itself worth something.
const COST_FLOOR: u32 = 8;

/// Integer step lengths, 10 = 1 px (so the diagonal is 14 ~ 10*sqrt(2)).
const STEP_ORTHO: u32 = 10;
const STEP_DIAG: u32 = 14;

/// How far the simplifier may move a traced point, in px. The wire comes back
/// pixel-stepped; the overlay and the scanline polygon fill are both happier
/// with a tenth of the points, and a 3/4 px wobble is invisible in a mask.
const THIN_EPS: f32 = 0.75;

/// Default snap range: how far off the cursor the wire may wander to find an
/// edge. Photoshop calls this Width. 40 px is ~1.7 mm on the owner's 600 dpi
/// page — loose enough to be worth having, tight enough not to jump panels.
pub const DEFAULT_REACH: i32 = 40;

/// Auto-anchor spacing while tracing with the pen down: once the cursor is
/// this far from the last anchor the wire behind it freezes into a new
/// anchor. Doubles as the search-window bound during a trace.
pub const AUTO_ANCHOR_PX: f32 = 48.0;

// --- the cost image ------------------------------------------------------

/// Lazily built edge strength over a document: 0 = flat paper, 255 = a hard
/// ink boundary. Blocks are filled on first read and kept for the session.
pub struct EdgeField {
    size: (i32, i32),
    blocks: HashMap<(i32, i32), Box<[u8; BLOCK_PX]>>,
}

impl EdgeField {
    pub fn new(doc: &Document) -> Self {
        Self {
            size: (doc.size.0 as i32, doc.size.1 as i32),
            blocks: HashMap::new(),
        }
    }

    /// How many blocks have actually been built — the laziness is a
    /// correctness property here, not an implementation detail, so it is
    /// observable (and tested).
    pub fn blocks_built(&self) -> usize {
        self.blocks.len()
    }

    /// Edge strength at a canvas pixel. Off-canvas reads 0: the page rim is
    /// not an edge to snap to, or every trace near the margin would stick.
    pub fn strength(&mut self, doc: &Document, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.size.0 || y >= self.size.1 {
            return 0;
        }
        let b = BLOCK as i32;
        let key = (x.div_euclid(b), y.div_euclid(b));
        if !self.blocks.contains_key(&key) {
            if self.blocks.len() >= MAX_BLOCKS {
                self.blocks.clear();
            }
            let built = build_block(doc, key);
            self.blocks.insert(key, built);
        }
        let bx = x.rem_euclid(b) as usize;
        let by = y.rem_euclid(b) as usize;
        self.blocks[&key][by * BLOCK + bx]
    }
}

/// Sobel magnitude over one block, from the visible page rendered over white.
fn build_block(doc: &Document, key: (i32, i32)) -> Box<[u8; BLOCK_PX]> {
    // One pixel of apron on every side: the 3x3 operator needs it.
    const W: usize = BLOCK + 2;
    let x0 = key.0 * BLOCK as i32 - 1;
    let y0 = key.1 * BLOCK as i32 - 1;
    let luma = luma_window(doc, x0, y0, W, W);
    let mut out = Box::new([0u8; BLOCK_PX]);
    for y in 0..BLOCK {
        for x in 0..BLOCK {
            let at = |dx: usize, dy: usize| luma[(y + dy) * W + (x + dx)] as i32;
            let gx = at(2, 0) + 2 * at(2, 1) + at(2, 2) - at(0, 0) - 2 * at(0, 1) - at(0, 2);
            let gy = at(0, 2) + 2 * at(1, 2) + at(2, 2) - at(0, 0) - 2 * at(1, 0) - at(2, 0);
            // A clean black/white step saturates one axis at 4*255, so /4
            // puts a page's real lineart edge at the top of the range.
            out[y * BLOCK + x] = ((gx.abs() + gy.abs()) / 4).min(255) as u8;
        }
    }
    out
}

/// The visible page over white paper in one window, as 8-bit luminance.
///
/// Blend modes, layer opacity and folder isolation are deliberately ignored
/// (the same simplification `fill::layers_over_white` makes): this feeds an
/// EDGE DETECTOR, and a manga page's edges are wherever the ink is, whatever
/// mode it was laid down in. Draft layers are skipped, matching the
/// fill/wand convention — snapping to the rough underdrawing instead of the
/// finished line is exactly the wrong answer.
fn luma_window(doc: &Document, x0: i32, y0: i32, w: usize, h: usize) -> Vec<u8> {
    // Accumulate in fix15 straight RGB on white paper, quantize once.
    let mut acc = vec![[32768u32; 3]; w * h];
    let vis = doc.effective_visibility();
    let drafts = doc.effective_drafts();
    let t = TILE_SIZE as i32;
    let tx0 = x0.div_euclid(t);
    let ty0 = y0.div_euclid(t);
    let tx1 = (x0 + w as i32 - 1).div_euclid(t);
    let ty1 = (y0 + h as i32 - 1).div_euclid(t);
    for (li, layer) in doc.layers.iter().enumerate() {
        if !vis[li] || drafts[li] {
            continue;
        }
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                let idx = TileIdx::new(tx, ty);
                let Some(tile) = layer.display_tile(idx) else {
                    continue;
                };
                let (ox, oy) = idx.origin();
                for py in 0..TILE_SIZE {
                    let y = oy + py as i32 - y0;
                    if y < 0 || y >= h as i32 {
                        continue;
                    }
                    for px in 0..TILE_SIZE {
                        let x = ox + px as i32 - x0;
                        if x < 0 || x >= w as i32 {
                            continue;
                        }
                        let p = tile.pixel(px, py);
                        let inv = 32768 - p[3] as u32;
                        let o = &mut acc[y as usize * w + x as usize];
                        for c in 0..3 {
                            o[c] = p[c] as u32 + o[c] * inv / 32768;
                        }
                    }
                }
            }
        }
    }
    acc.iter()
        .map(|p| {
            // Rec.601 weights x256, so the shift lands back on the fix15 scale.
            let l = (p[0] as u64 * 77 + p[1] as u64 * 150 + p[2] as u64 * 29) >> 8;
            ((l * 255 + 16384) / 32768).min(255) as u8
        })
        .collect()
}

// --- the live wire -------------------------------------------------------

/// Shortest path from `from` to `to` over the edge field, confined to the
/// bounding box of the two points inflated by `reach` px.
///
/// Returns the pixel path inclusive of both ends. Falls back to a straight
/// line when the window would exceed [`MAX_WINDOW`] on either side — the
/// user has dragged a long way without anchoring, and a straight line is
/// both the cheap answer and the visible signal to drop an anchor.
pub fn wire(
    field: &mut EdgeField,
    doc: &Document,
    from: (i32, i32),
    to: (i32, i32),
    reach: i32,
) -> Vec<(i32, i32)> {
    let (cw, ch) = (field.size.0, field.size.1);
    if cw <= 0 || ch <= 0 {
        return vec![from];
    }
    let clamp = |p: (i32, i32)| (p.0.clamp(0, cw - 1), p.1.clamp(0, ch - 1));
    let (from, to) = (clamp(from), clamp(to));
    if from == to {
        return vec![from];
    }
    let reach = reach.max(1);
    let x0 = (from.0.min(to.0) - reach).max(0);
    let y0 = (from.1.min(to.1) - reach).max(0);
    let x1 = (from.0.max(to.0) + reach).min(cw - 1);
    let y1 = (from.1.max(to.1) + reach).min(ch - 1);
    let (ww, wh) = (x1 - x0 + 1, y1 - y0 + 1);
    if ww > MAX_WINDOW || wh > MAX_WINDOW {
        return straight(from, to);
    }
    let (ww, wh) = (ww as usize, wh as usize);
    let n = ww * wh;

    // Per-pixel walk cost, resolved once: `strength` is a hash lookup, and
    // Dijkstra would otherwise ask for the same pixel up to eight times.
    let mut node: Vec<u32> = Vec::with_capacity(n);
    for y in 0..wh {
        for x in 0..ww {
            let s = field.strength(doc, x0 + x as i32, y0 + y as i32) as u32;
            node.push(COST_FLOOR + (255 - s));
        }
    }

    let start = (from.1 - y0) as usize * ww + (from.0 - x0) as usize;
    let goal = (to.1 - y0) as usize * ww + (to.0 - x0) as usize;
    let mut dist = vec![u32::MAX; n];
    let mut prev = vec![usize::MAX; n];
    let mut heap: BinaryHeap<Reverse<(u32, usize)>> = BinaryHeap::new();
    dist[start] = 0;
    heap.push(Reverse((0, start)));
    while let Some(Reverse((d, i))) = heap.pop() {
        if i == goal {
            break;
        }
        if d > dist[i] {
            continue;
        }
        let (ix, iy) = ((i % ww) as i32, (i / ww) as i32);
        for (dx, dy) in [
            (1, 0),
            (-1, 0),
            (0, 1),
            (0, -1),
            (1, 1),
            (1, -1),
            (-1, 1),
            (-1, -1),
        ] {
            let (nx, ny) = (ix + dx, iy + dy);
            if nx < 0 || ny < 0 || nx >= ww as i32 || ny >= wh as i32 {
                continue;
            }
            let j = ny as usize * ww + nx as usize;
            let step = if dx != 0 && dy != 0 {
                STEP_DIAG
            } else {
                STEP_ORTHO
            };
            let nd = d + node[j] * step;
            if nd < dist[j] {
                dist[j] = nd;
                prev[j] = i;
                heap.push(Reverse((nd, j)));
            }
        }
    }
    if dist[goal] == u32::MAX {
        // Cannot happen on a connected grid, but a wrong answer here would
        // be an empty outline rather than a visible fault.
        return straight(from, to);
    }
    let mut path = Vec::new();
    let mut i = goal;
    loop {
        path.push((x0 + (i % ww) as i32, y0 + (i / ww) as i32));
        if i == start {
            break;
        }
        i = prev[i];
    }
    path.reverse();
    path
}

/// Bresenham, the no-snap answer.
fn straight(from: (i32, i32), to: (i32, i32)) -> Vec<(i32, i32)> {
    let (mut x, mut y) = from;
    let (dx, dy) = ((to.0 - x).abs(), -(to.1 - y).abs());
    let (sx, sy) = (if x < to.0 { 1 } else { -1 }, if y < to.1 { 1 } else { -1 });
    let mut err = dx + dy;
    let mut out = vec![(x, y)];
    while (x, y) != to {
        let e2 = err * 2;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
        out.push((x, y));
    }
    out
}

/// Pixel path -> drawable/fillable polyline at pixel centres, thinned.
fn thin(path: &[(i32, i32)]) -> Vec<(f32, f32)> {
    let raw: Vec<[f32; 2]> = path
        .iter()
        .map(|&(x, y)| [x as f32 + 0.5, y as f32 + 0.5])
        .collect();
    crate::balloon::simplify_polyline(&raw, THIN_EPS)
        .into_iter()
        .map(|p| (p[0], p[1]))
        .collect()
}

// --- the session ---------------------------------------------------------

/// One magnetic-lasso trace in progress.
pub struct Lasso {
    field: EdgeField,
    /// Placed anchors, canvas px. `anchors[0]` is where the trace began and
    /// where [`Lasso::close`] returns to.
    anchors: Vec<(i32, i32)>,
    /// One committed wire per anchor step: `segs[i]` runs `anchors[i]` ->
    /// `anchors[i + 1]`. Always `anchors.len() - 1` long.
    segs: Vec<Vec<(f32, f32)>>,
    /// The uncommitted wire from the last anchor to the cursor.
    live: Vec<(f32, f32)>,
    /// Where `live` was computed to, so a jittering pen does not re-run
    /// Dijkstra for a third of a pixel.
    live_to: (i32, i32),
    /// Snap range in px (Tool Property).
    pub reach: i32,
}

impl Lasso {
    /// Begin a trace at `at`, which becomes the first anchor.
    pub fn start(doc: &Document, at: (i32, i32), reach: i32) -> Self {
        Self {
            field: EdgeField::new(doc),
            anchors: vec![at],
            segs: Vec::new(),
            live: Vec::new(),
            live_to: at,
            reach,
        }
    }

    pub fn anchors(&self) -> &[(i32, i32)] {
        &self.anchors
    }

    /// The anchor the live wire currently grows from.
    pub fn last_anchor(&self) -> (i32, i32) {
        *self.anchors.last().expect("a lasso always has its first anchor")
    }

    /// Straight-line distance from the last anchor — the auto-anchor trigger
    /// and the search-window size in one number.
    pub fn drift(&self, at: (i32, i32)) -> f32 {
        let a = self.last_anchor();
        (((at.0 - a.0) as f32).powi(2) + ((at.1 - a.1) as f32).powi(2)).sqrt()
    }

    /// Is `at` within `tol` px of the first anchor (the close target)?
    pub fn near_start(&self, at: (i32, i32), tol: f32) -> bool {
        let f = self.anchors[0];
        ((at.0 - f.0) as f32).abs() + ((at.1 - f.1) as f32).abs() <= tol
    }

    /// Re-run the live wire to the cursor. Cheap for a stationary pen.
    pub fn track(&mut self, doc: &Document, to: (i32, i32)) {
        if to == self.live_to && !self.live.is_empty() {
            return;
        }
        self.live_to = to;
        let from = self.last_anchor();
        let path = wire(&mut self.field, doc, from, to, self.reach);
        self.live = thin(&path);
    }

    /// Freeze the live wire and place an anchor at its end.
    pub fn anchor(&mut self, doc: &Document, at: (i32, i32)) {
        self.track(doc, at);
        let seg = std::mem::take(&mut self.live);
        self.segs.push(seg);
        self.anchors.push(at);
        self.live_to = at;
    }

    /// Backspace: drop the last anchor and the wire that reached it. Returns
    /// false at the first anchor, where there is nothing left to undo — the
    /// caller cancels the trace instead.
    ///
    /// The live wire is dropped rather than re-aimed from the anchor that is
    /// now last: re-aiming would instantly redraw the stretch just undone
    /// (same two endpoints, same wire), which reads as "Backspace did
    /// nothing". The next [`Lasso::track`] puts it back deliberately.
    pub fn undo_anchor(&mut self) -> bool {
        if self.anchors.len() < 2 {
            return false;
        }
        self.anchors.pop();
        self.segs.pop();
        self.live.clear();
        self.live_to = self.last_anchor();
        true
    }

    /// Everything to draw: the committed wire plus the live wire.
    pub fn preview(&self) -> Vec<(f32, f32)> {
        let mut out: Vec<(f32, f32)> =
            Vec::with_capacity(self.segs.iter().map(|s| s.len()).sum::<usize>() + self.live.len());
        let a = self.anchors[0];
        out.push((a.0 as f32 + 0.5, a.1 as f32 + 0.5));
        for s in &self.segs {
            out.extend_from_slice(s);
        }
        out.extend_from_slice(&self.live);
        out
    }

    /// Close the loop: wire back to the first anchor and hand over the
    /// polygon. Fewer than three points means the trace never went anywhere
    /// and the caller should treat it as a cancel.
    pub fn close(&mut self, doc: &Document) -> Vec<(f32, f32)> {
        let first = self.anchors[0];
        // Whatever the cursor was pointing at is part of the outline, then
        // the wire runs home along the edge like every other stretch.
        if !self.live.is_empty() {
            self.anchor(doc, self.live_to);
        }
        let (last, reach) = (self.last_anchor(), self.reach);
        let home = wire(&mut self.field, doc, last, first, reach);
        let mut out = self.preview();
        out.extend(thin(&home));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::FIX15_ONE;

    fn paint(doc: &mut Document, x: i32, y: i32) {
        paint_luma(doc, x, y, 0);
    }

    /// Opaque grey of luminance `l` (0 = ink, 255 = paper), premultiplied.
    fn paint_luma(doc: &mut Document, x: i32, y: i32, l: u32) {
        let v = (l * FIX15_ONE / 255) as u16;
        let idx = TileIdx::of_pixel(x, y);
        let (ox, oy) = idx.origin();
        doc.active_layer_mut().tile_mut(idx).set_pixel(
            (x - ox) as usize,
            (y - oy) as usize,
            [v, v, v, FIX15_ONE as u16],
        );
    }

    /// A page with one thick horizontal rule across the middle — the
    /// simplest thing a magnetic lasso should stick to.
    fn ruled_page() -> Document {
        let mut doc = Document::new(256, 256);
        for x in 0..256 {
            for y in 100..104 {
                paint(&mut doc, x, y);
            }
        }
        doc
    }

    #[test]
    fn edge_strength_is_high_at_the_line_and_zero_on_paper() {
        let doc = ruled_page();
        let mut f = EdgeField::new(&doc);
        // The line's top boundary at y=100: the operator centred one pixel
        // above sees the full black/white step.
        assert!(
            f.strength(&doc, 128, 99) > 200,
            "the ink boundary saturates: {}",
            f.strength(&doc, 128, 99)
        );
        assert_eq!(f.strength(&doc, 128, 20), 0, "blank paper is flat");
        assert_eq!(f.strength(&doc, 128, 102), 0, "inside the ink is flat too");
        assert_eq!(f.strength(&doc, -1, 20), 0, "off-canvas reads flat");
    }

    #[test]
    fn the_field_only_builds_the_blocks_it_is_asked_for() {
        // 256x256 is 16 blocks; touching two pixels must not build 16.
        let doc = ruled_page();
        let mut f = EdgeField::new(&doc);
        assert_eq!(f.blocks_built(), 0, "nothing built before the first read");
        f.strength(&doc, 10, 10);
        assert_eq!(f.blocks_built(), 1);
        f.strength(&doc, 20, 20); // same block
        assert_eq!(f.blocks_built(), 1, "a second read in the block is free");
        f.strength(&doc, 200, 200);
        assert_eq!(f.blocks_built(), 2);
    }

    /// The whole point: asked to get from one side of the rule to the other
    /// via two points that sit ON it, the wire must ride the ink boundary
    /// rather than cut across the paper.
    #[test]
    fn the_wire_rides_the_edge_instead_of_cutting_across_it() {
        let doc = ruled_page();
        let mut f = EdgeField::new(&doc);
        // Both ends near the line's top boundary, but 6 px off it in the
        // paper — a straight line would sag through blank white.
        let path = wire(&mut f, &doc, (40, 94), (140, 94), 40);
        assert_eq!(path.first().copied(), Some((40, 94)));
        assert_eq!(path.last().copied(), Some((140, 94)));
        let on_edge = path
            .iter()
            .filter(|(_, y)| (98..=101).contains(y))
            .count();
        assert!(
            on_edge * 2 > path.len(),
            "most of the path snapped onto the boundary ({on_edge} of {})",
            path.len()
        );
    }

    /// The one that matters: WHICH edge it snaps to. A magnetic lasso that
    /// grabs the wrong line is worse than none, so the cost function is
    /// measured against a page holding two candidates the same distance
    /// away — hard black lineart on one side, a pale tone boundary on the
    /// other. Only the ink is worth leaving the straight line for.
    #[test]
    fn the_wire_prefers_the_strong_edge_over_an_equally_close_weak_one() {
        let mut doc = Document::new(220, 200);
        for x in 0..220 {
            for y in 80..84 {
                paint(&mut doc, x, y); // black lineart, 20 px above the ends
            }
            for y in 116..120 {
                paint_luma(&mut doc, x, y, 200); // pale tone, 20 px below
            }
        }
        let mut f = EdgeField::new(&doc);
        let path = wire(&mut f, &doc, (40, 100), (180, 100), 40);
        let on_ink = path.iter().filter(|(_, y)| (78..=84).contains(y)).count();
        let on_tone = path
            .iter()
            .filter(|(_, y)| (114..=121).contains(y))
            .count();
        assert!(
            on_ink * 2 > path.len(),
            "most of the wire rode the ink ({on_ink} of {})",
            path.len()
        );
        assert_eq!(on_tone, 0, "and none of it touched the pale boundary");
    }

    /// Snap range is a real limit, not decoration: an edge further off than
    /// `reach` is outside the search window, so the wire runs straight past
    /// it rather than lunging across the page for it.
    #[test]
    fn an_edge_beyond_the_snap_range_is_not_snapped_to() {
        let doc = ruled_page(); // the rule is at y=100..104
        let mut f = EdgeField::new(&doc);
        let path = wire(&mut f, &doc, (40, 130), (140, 130), 6);
        assert!(
            path.iter().all(|(_, y)| (124..=136).contains(y)),
            "the wire stayed inside its window"
        );
        assert_eq!(path.first().copied(), Some((40, 130)));
        assert_eq!(path.last().copied(), Some((140, 130)));
    }

    /// A corner is where a straight-line lasso costs you the most, so it is
    /// where the wire has to earn its keep: given the two ends of an L of
    /// lineart, it must go round the corner rather than cut the diagonal.
    #[test]
    fn the_wire_turns_a_corner_instead_of_cutting_it() {
        let mut doc = Document::new(220, 220);
        for x in 40..141 {
            for y in 60..64 {
                paint(&mut doc, x, y); // the L's top arm
            }
        }
        for y in 60..181 {
            for x in 136..140 {
                paint(&mut doc, x, y); // the L's right arm
            }
        }
        let mut f = EdgeField::new(&doc);
        let path = wire(&mut f, &doc, (44, 56), (132, 170), DEFAULT_REACH);
        // Cutting the diagonal would never come near the outside corner.
        assert!(
            path.iter()
                .any(|&(x, y)| (x - 135).abs() <= 5 && (y - 59).abs() <= 5),
            "the path went round by the corner"
        );
        // And it is a detour: the straight run is ~114 px of steps.
        assert!(
            path.len() > 180,
            "it took the long way round: {} steps",
            path.len()
        );
    }

    #[test]
    fn a_blank_page_gives_a_straight_line() {
        let doc = Document::new(256, 256);
        let mut f = EdgeField::new(&doc);
        let path = wire(&mut f, &doc, (20, 20), (60, 60), 30);
        // With flat costs the diagonal is optimal and unique in length.
        assert_eq!(path.len(), 41, "one diagonal step per pixel");
        for (i, p) in path.iter().enumerate() {
            assert_eq!(*p, (20 + i as i32, 20 + i as i32), "step {i}");
        }
    }

    /// The hang guard: a drag that spans more than the window ceiling stops
    /// searching and runs straight, instead of paying O(page).
    #[test]
    fn an_over_long_drag_falls_back_to_a_straight_line() {
        // 1090 px apart on a page big enough to hold the window: the search
        // would be ~1.3M nodes, so it must not run at all.
        let doc = Document::new(1200, 1200);
        let mut f = EdgeField::new(&doc);
        let path = wire(&mut f, &doc, (10, 94), (1100, 94), DEFAULT_REACH);
        assert_eq!(f.blocks_built(), 0, "the fallback costs no cost image");
        assert_eq!(path.len(), 1091, "one pixel per step of the run");
        assert!(path.iter().all(|(_, y)| *y == 94), "dead straight");
    }

    #[test]
    fn anchors_commit_and_backspace_takes_them_off_one_at_a_time() {
        let doc = ruled_page();
        let mut l = Lasso::start(&doc, (40, 94), DEFAULT_REACH);
        assert_eq!(l.anchors().len(), 1);
        assert!(!l.undo_anchor(), "nothing to undo at the first anchor");

        l.anchor(&doc, (90, 94));
        l.anchor(&doc, (140, 94));
        assert_eq!(l.anchors().len(), 3);
        let long = l.preview().len();

        assert!(l.undo_anchor());
        assert_eq!(l.anchors().len(), 2);
        assert_eq!(l.last_anchor(), (90, 94));
        assert!(
            l.preview().len() < long,
            "the undone stretch left the preview"
        );
        assert!(l.undo_anchor());
        assert!(!l.undo_anchor(), "back at the first anchor again");
    }

    #[test]
    fn tracking_is_free_when_the_pen_has_not_moved() {
        let doc = ruled_page();
        let mut l = Lasso::start(&doc, (40, 94), DEFAULT_REACH);
        l.track(&doc, (80, 94));
        let a = l.preview();
        l.track(&doc, (80, 94));
        assert_eq!(l.preview(), a, "same cursor, same wire");
        l.track(&doc, (81, 94));
        assert_ne!(l.preview(), a, "a moved cursor re-wires");
    }

    #[test]
    fn closing_returns_a_loop_that_fills_as_a_selection() {
        // Blank paper on purpose: with nothing to snap to the four wires are
        // exactly the four sides, so this measures the CLOSE, not the snap.
        let doc = Document::new(256, 256);
        let mut l = Lasso::start(&doc, (40, 60), DEFAULT_REACH);
        l.anchor(&doc, (140, 60));
        l.anchor(&doc, (140, 90));
        l.track(&doc, (40, 90));
        let poly = l.close(&doc);
        assert!(poly.len() >= 4, "a closed quad at least: {}", poly.len());
        let sel = crate::Selection::from_polygon(&doc, &poly);
        assert_eq!(sel.coverage(90, 75), 255, "the interior is selected");
        assert_eq!(sel.coverage(10, 10), 0, "the outside is not");
    }

    #[test]
    fn drift_and_the_close_target_measure_from_the_right_anchors() {
        let doc = ruled_page();
        let mut l = Lasso::start(&doc, (40, 60), DEFAULT_REACH);
        assert!(l.near_start((42, 61), 5.0));
        assert!(!l.near_start((80, 60), 5.0));
        assert!((l.drift((40, 90)) - 30.0).abs() < 0.01);
        l.anchor(&doc, (40, 90));
        assert!(l.drift((40, 90)) < 0.01, "drift resets at a fresh anchor");
        assert!(l.near_start((40, 60), 5.0), "close still means anchor zero");
    }

    /// Draft layers are the rough underdrawing; snapping to them instead of
    /// the finished line is the wrong answer, and the fill/wand family
    /// already agrees.
    #[test]
    fn draft_layers_do_not_pull_the_wire() {
        let mut doc = Document::new(128, 128);
        for x in 0..128 {
            paint(&mut doc, x, 60);
        }
        doc.set_layer_draft(doc.active, true);
        let mut f = EdgeField::new(&doc);
        assert_eq!(f.strength(&doc, 64, 59), 0, "a draft line is not an edge");
        doc.set_layer_draft(doc.active, false);
        let mut f2 = EdgeField::new(&doc);
        assert!(f2.strength(&doc, 64, 59) > 200, "un-drafted, it is");
    }

    /// A hidden layer is not on the page, so it must not be on the rail.
    #[test]
    fn hidden_layers_do_not_pull_the_wire() {
        let mut doc = Document::new(128, 128);
        for x in 0..128 {
            paint(&mut doc, x, 60);
        }
        doc.layers[doc.active].visible = false;
        let mut f = EdgeField::new(&doc);
        assert_eq!(f.strength(&doc, 64, 59), 0);
    }
}
