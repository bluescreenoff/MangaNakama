//! Frames (koma) — vector panel polygons that rasterize into a layer.
//!
//! The model: a frame layer (`LayerKind::Frame`) owns a [`FrameSet`] — convex
//! polygons in canvas pixels plus a border width. Its raster content is fully
//! *derived*: opaque white everywhere outside the union of the frames (the
//! gutter — it hides art spilling out of panels, which is what "frames mask
//! their contents" means visually), transparent inside, and an anti-aliased
//! black border stroke centred on every polygon edge.
//!
//! Convex frames (the divide tool always makes them) rasterize through the
//! cheap max-half-plane signed distance, whose iso-contours have mitered
//! (sharp) corners — what a manga border wants. Since round 13, frames may
//! also be **concave simple polygons** (CSP's Polyline frame / Frame border
//! pen): those go through an exact polygon SDF (min edge distance, sign by
//! even-odd crossing) — same 1-Lipschitz bound, so the tile classification
//! still holds. The divide tool refuses to cut concave frames.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::ruler::CurveRuler;
use crate::tile::{FIX15_ONE, TILE_SIZE, Tile, TileIdx};

/// Cuts that would leave a panel smaller than this (px²) are refused.
pub const MIN_FRAME_AREA: f32 = 64.0;

/// One panel: a simple polygon (convex or concave), canvas px, any winding.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub points: Vec<[f32; 2]>,
}

/// A frame's prepared signed-distance evaluator: half-planes for convex
/// polygons (mitered corners), the exact polygon SDF for concave ones.
enum FrameSdf {
    Convex(Vec<HalfPlane>),
    Exact(Vec<[f32; 2]>),
}

impl FrameSdf {
    fn of(f: &Frame) -> Self {
        if f.is_convex() {
            FrameSdf::Convex(f.half_planes())
        } else {
            FrameSdf::Exact(f.points.clone())
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            FrameSdf::Convex(hp) => hp.is_empty(),
            FrameSdf::Exact(p) => p.len() < 3,
        }
    }

    /// Signed distance, negative inside. 1-Lipschitz either way, so the
    /// tile-centre classification in the rasterizers stays conservative.
    fn dist(&self, p: [f32; 2]) -> f32 {
        match self {
            FrameSdf::Convex(hp) => hp
                .iter()
                .map(|h| dot(h.n, p) - h.c)
                .fold(f32::NEG_INFINITY, f32::max),
            FrameSdf::Exact(pts) => {
                let n = pts.len();
                let mut d = f32::INFINITY;
                let mut inside = false;
                for i in 0..n {
                    let a = pts[i];
                    let b = pts[(i + 1) % n];
                    d = d.min(segment_distance(p, a, b));
                    // Even-odd crossing count for the sign.
                    if (a[1] > p[1]) != (b[1] > p[1]) {
                        let t = (p[1] - a[1]) / (b[1] - a[1]);
                        if a[0] + t * (b[0] - a[0]) > p[0] {
                            inside = !inside;
                        }
                    }
                }
                if inside { -d } else { d }
            }
        }
    }
}

/// Every frame on a frame layer + how thick the ink border is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrameSet {
    pub frames: Vec<Frame>,
    /// Border stroke width in canvas px, centred on the polygon edge.
    pub border_px: f32,
    /// Reading-order provenance (owner top item 2026-08-18): the rect
    /// this set's region was re-partitioned from by the division that
    /// produced it — halves of one divide share it, so they order
    /// INSIDE the slot and can never scatter across the page.
    /// `None` = never divided (a whole-page folder). Rides the ORA as
    /// part of this set's JSON; absent in old files = None.
    #[serde(default)]
    pub slot: Option<[f32; 4]>,
    /// Manual reading-order pin (owner top item 2026-08-18): this
    /// folder's panels occupy this 1-based reading position together,
    /// overruling the computed order. None = automatic.
    #[serde(default)]
    pub reading_pin: Option<u32>,
    /// `FB-053`/`FB-054` — CSP's *Draw border* unchecked, AFTER creation.
    /// The folder keeps its shape, its mask and its `border_px`, but lays
    /// down no ink: the outline becomes a **ruler** you ink yourself, so the
    /// panel edge carries pen pressure and brush character instead of being
    /// a uniform machine stroke. Kept as its own flag rather than zeroing
    /// `border_px`, so switching the ink back on restores the artist's
    /// width. `false` in old files = the border draws, as it always did.
    #[serde(default)]
    pub border_ruler: bool,
}

/// An edge as a half-plane: signed distance = `n·p - c`, positive outside.
#[derive(Clone, Copy, Debug)]
struct HalfPlane {
    n: [f32; 2],
    c: f32,
}

fn sub(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn dot(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

fn cross(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[1] - a[1] * b[0]
}

/// Does `o` sit ACROSS the gutter from the edge `a`–`b` (whose outward
/// normal is `nrm`), and if so, which of its vertices form the facing edge?
///
/// Returns `(distance, vertex indices)`. `None` when `o` is not "the next
/// panel over": its projection onto the edge's own direction must OVERLAP
/// the edge — a panel off to the side is beside this one, not facing it.
/// The distance is to the NEAREST vertex in front of the edge; the returned
/// indices are every vertex within 1.5 px of that depth, i.e. the whole
/// facing edge (both corners of a rectangle's near side).
///
/// This is what "Keep gutters aligned" moves: drag one panel's border and
/// the facing border of its neighbour travels with it, so the gutter keeps
/// its width instead of silently closing (audit P0-4).
pub fn facing_vertices(
    o: &Frame,
    a: [f32; 2],
    b: [f32; 2],
    nrm: [f32; 2],
) -> Option<(f32, Vec<usize>)> {
    if o.points.len() < 3 {
        return None;
    }
    let e = sub(b, a);
    let len = dot(e, e).sqrt();
    if len < 1e-3 {
        return None;
    }
    let dir = [e[0] / len, e[1] / len];
    let (lo, hi) = (dot(dir, a).min(dot(dir, b)), dot(dir, a).max(dot(dir, b)));
    let base = dot(nrm, a);
    let (mut olo, mut ohi) = (f32::INFINITY, f32::NEG_INFINITY);
    let mut near = f32::INFINITY;
    for p in &o.points {
        let s = dot(dir, *p);
        olo = olo.min(s);
        ohi = ohi.max(s);
        let t = dot(nrm, *p) - base;
        if t > 0.01 {
            near = near.min(t);
        }
    }
    if ohi <= lo || olo >= hi || !near.is_finite() {
        return None;
    }
    let idx: Vec<usize> = o
        .points
        .iter()
        .enumerate()
        .filter(|(_, p)| (dot(nrm, **p) - base - near).abs() <= 1.5)
        .map(|(k, _)| k)
        .collect();
    Some((near, idx))
}

impl Frame {
    pub fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self {
            points: vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
        }
    }

    pub fn centroid(&self) -> [f32; 2] {
        let n = self.points.len().max(1) as f32;
        let (mut x, mut y) = (0.0, 0.0);
        for p in &self.points {
            x += p[0];
            y += p[1];
        }
        [x / n, y / n]
    }

    /// Unsigned area (shoelace).
    pub fn area(&self) -> f32 {
        let n = self.points.len();
        if n < 3 {
            return 0.0;
        }
        let mut s = 0.0;
        for i in 0..n {
            s += cross(self.points[i], self.points[(i + 1) % n]);
        }
        (s * 0.5).abs()
    }

    /// True for a non-degenerate convex polygon (collinear runs allowed).
    pub fn is_convex(&self) -> bool {
        let n = self.points.len();
        if n < 3 || self.area() < f32::EPSILON {
            return false;
        }
        let (mut pos, mut neg) = (false, false);
        for i in 0..n {
            let a = self.points[i];
            let b = self.points[(i + 1) % n];
            let c = self.points[(i + 2) % n];
            let z = cross(sub(b, a), sub(c, b));
            if z > 1e-3 {
                pos = true;
            } else if z < -1e-3 {
                neg = true;
            }
        }
        !(pos && neg)
    }

    /// The polygon's edges as outward half-planes. Sign is fixed against the
    /// centroid (inside for a convex polygon), so winding never matters.
    fn half_planes(&self) -> Vec<HalfPlane> {
        let ctr = self.centroid();
        let n = self.points.len();
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let a = self.points[i];
            let b = self.points[(i + 1) % n];
            let e = sub(b, a);
            let len = dot(e, e).sqrt();
            if len < 1e-6 {
                continue;
            }
            let mut normal = [e[1] / len, -e[0] / len];
            if dot(normal, sub(ctr, a)) > 0.0 {
                normal = [-normal[0], -normal[1]];
            }
            out.push(HalfPlane {
                n: normal,
                c: dot(normal, a),
            });
        }
        out
    }

    /// Signed distance, negative inside. Convex frames get the mitered
    /// max-half-plane form; concave ones the exact polygon SDF.
    pub fn distance(&self, p: [f32; 2]) -> f32 {
        let sdf = FrameSdf::of(self);
        if sdf.is_empty() {
            return f32::INFINITY;
        }
        sdf.dist(p)
    }

    /// True when no two non-adjacent edges intersect (a drawable panel shape).
    /// O(n²) — frame polygons are small.
    pub fn is_simple(&self) -> bool {
        let n = self.points.len();
        if n < 3 || self.area() < f32::EPSILON {
            return false;
        }
        for i in 0..n {
            let (a, b) = (self.points[i], self.points[(i + 1) % n]);
            for j in i + 1..n {
                // Skip adjacent edges (they share a vertex).
                if j == i || (j + 1) % n == i || (i + 1) % n == j {
                    continue;
                }
                let (c, d) = (self.points[j], self.points[(j + 1) % n]);
                if segments_intersect(a, b, c, d) {
                    return false;
                }
            }
        }
        true
    }

    pub fn contains(&self, p: [f32; 2]) -> bool {
        self.distance(p) <= 0.0
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        for p in &mut self.points {
            p[0] += dx;
            p[1] += dy;
        }
    }

    /// Rotate every vertex `rad` (clockwise in y-down canvas space) around
    /// `c` — the Object tool's rotation lollipop.
    pub fn rotate_around(&mut self, c: [f32; 2], rad: f32) {
        let (s, co) = rad.sin_cos();
        for p in &mut self.points {
            let (dx, dy) = (p[0] - c[0], p[1] - c[1]);
            *p = [c[0] + dx * co - dy * s, c[1] + dx * s + dy * co];
        }
    }

    /// Scale every vertex by (sx, sy) around `c` — the Object tool's bbox
    /// corner/edge handles.
    pub fn scale_around(&mut self, c: [f32; 2], sx: f32, sy: f32) {
        for p in &mut self.points {
            p[0] = c[0] + (p[0] - c[0]) * sx;
            p[1] = c[1] + (p[1] - c[1]) * sy;
        }
    }

    /// Index of the vertex within `radius` of `p`, nearest first.
    pub fn vertex_near(&self, p: [f32; 2], radius: f32) -> Option<usize> {
        let mut best: Option<(usize, f32)> = None;
        for (i, v) in self.points.iter().enumerate() {
            let d = dot(sub(*v, p), sub(*v, p)).sqrt();
            if d <= radius && best.map_or(true, |(_, bd)| d < bd) {
                best = Some((i, d));
            }
        }
        best.map(|(i, _)| i)
    }

    /// Index of the edge (i → i+1) whose segment passes within `radius` of `p`.
    pub fn edge_near(&self, p: [f32; 2], radius: f32) -> Option<usize> {
        let n = self.points.len();
        let mut best: Option<(usize, f32)> = None;
        for i in 0..n {
            let a = self.points[i];
            let b = self.points[(i + 1) % n];
            let d = segment_distance(p, a, b);
            if d <= radius && best.map_or(true, |(_, bd)| d < bd) {
                best = Some((i, d));
            }
        }
        best.map(|(i, _)| i)
    }

    /// Keep the part of the polygon with `n·p <= c` (Sutherland–Hodgman, one
    /// plane). May return fewer than 3 points — callers check.
    fn clip_half_plane(&self, n: [f32; 2], c: f32) -> Frame {
        let pts = &self.points;
        let count = pts.len();
        let mut out = Vec::with_capacity(count + 1);
        for i in 0..count {
            let a = pts[i];
            let b = pts[(i + 1) % count];
            let da = dot(n, a) - c;
            let db = dot(n, b) - c;
            if da <= 0.0 {
                out.push(a);
            }
            if (da < 0.0) != (db < 0.0) && (da - db).abs() > 1e-9 {
                let t = da / (da - db);
                out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
            }
        }
        Frame { points: out }
    }

    /// Cut by the infinite line through `a`–`b`, leaving a `gutter` gap centred
    /// on the line. `None` when either side would be degenerate or tiny, or
    /// when the frame is concave (half-plane clipping is convex-only).
    pub fn split(&self, a: [f32; 2], b: [f32; 2], gutter: f32) -> Option<(Frame, Frame)> {
        if !self.is_convex() {
            return None;
        }
        let e = sub(b, a);
        let len = dot(e, e).sqrt();
        if len < 1e-3 {
            return None;
        }
        let n = [e[1] / len, -e[0] / len];
        let c = dot(n, a);
        let g = (gutter * 0.5).max(0.0);
        let side1 = self.clip_half_plane(n, c - g);
        let side2 = self.clip_half_plane([-n[0], -n[1]], -(c + g));
        if side1.points.len() < 3
            || side2.points.len() < 3
            || side1.area() < MIN_FRAME_AREA
            || side2.area() < MIN_FRAME_AREA
        {
            return None;
        }
        Some((side1, side2))
    }

    /// True when the drag segment `a`–`b` actually touches this frame — the
    /// divide tool only cuts frames the user dragged across.
    pub fn segment_touches(&self, a: [f32; 2], b: [f32; 2]) -> bool {
        if self.contains(a) || self.contains(b) {
            return true;
        }
        let n = self.points.len();
        for i in 0..n {
            if segments_intersect(a, b, self.points[i], self.points[(i + 1) % n]) {
                return true;
            }
        }
        false
    }

    /// C-061/062 "Shape of division": cut this panel with an OPEN polyline
    /// instead of a straight line, leaving `gutter` between the halves. A
    /// spline cut is the same call on a densely sampled path, which is why
    /// there is one routine and not two.
    ///
    /// Convex frames only, exactly like [`Self::split`] — and the halves it
    /// returns are usually CONCAVE, so they rasterize (round 13's exact SDF)
    /// but cannot be divided again. The path must cross the panel's outline
    /// once in and once out; a cut that leaves the panel and comes back is
    /// refused rather than guessed at.
    ///
    /// Unlike [`Self::split`] this is not half-plane clipping — the cut edge
    /// keeps every vertex of the path, so it walks the two boundaries instead
    /// (see [`Frame::halves_by_path`]).
    pub fn split_path(&self, path: &[[f32; 2]], gutter: f32) -> Option<(Frame, Frame)> {
        if !self.is_convex() || path.len() < 2 {
            return None;
        }
        // Two points IS a straight cut: keep the exact half-plane form.
        if path.len() == 2 {
            return self.split(path[0], path[1], gutter);
        }
        let bb = self.bbox();
        let reach = (bb[2] - bb[0]).abs() + (bb[3] - bb[1]).abs() + gutter + 16.0;
        let g = (gutter * 0.5).max(0.0);
        // Each half is bounded by the path pushed INTO it by half the gutter,
        // so the strip between them is exactly `gutter` wide. `offset_path`
        // moves along `+n` where `n = [e.y, -e.x]`, the same normal
        // convention `split` uses — so a 2-point path and a 3-point path
        // agree at the limit.
        let (a, _) = self.halves_by_path(&offset_path(path, -g, reach))?;
        let (_, b) = self.halves_by_path(&offset_path(path, g, reach))?;
        if a.area() < MIN_FRAME_AREA
            || b.area() < MIN_FRAME_AREA
            || !a.is_simple()
            || !b.is_simple()
        {
            return None;
        }
        Some((a, b))
    }

    /// The two pieces an OPEN polyline cuts this polygon into, as
    /// `(negative-normal side, positive-normal side)` of the crossing chord.
    ///
    /// The path must cross the outline exactly twice. Each piece is the
    /// inside run of the path plus the arc of the polygon boundary between
    /// the exit and the entry — a boundary walk, which is why the cut edge
    /// keeps every one of the path's vertices instead of being flattened to
    /// a chord. Both crossings landing on the SAME polygon edge is handled
    /// (one piece then has no polygon vertices at all).
    fn halves_by_path(&self, path: &[[f32; 2]]) -> Option<(Frame, Frame)> {
        let n = self.points.len();
        if n < 3 || path.len() < 2 {
            return None;
        }
        // Crossings in path order: (path segment, boundary parameter, point).
        // The boundary parameter is `edge index + t along that edge`, which
        // is what makes the two arcs a subtraction instead of a case split.
        let mut xs: Vec<(usize, f32, [f32; 2])> = Vec::new();
        for (si, w) in path.windows(2).enumerate() {
            let mut hits: Vec<(f32, f32, [f32; 2])> = Vec::new();
            for ei in 0..n {
                let (a, b) = (self.points[ei], self.points[(ei + 1) % n]);
                if let Some((t, s)) = segment_cross(w[0], w[1], a, b) {
                    let p = [a[0] + (b[0] - a[0]) * s, a[1] + (b[1] - a[1]) * s];
                    hits.push((t, ei as f32 + s, p));
                }
            }
            hits.sort_by(|x, y| x.0.total_cmp(&y.0));
            xs.extend(hits.into_iter().map(|(_, u, p)| (si, u, p)));
        }
        if xs.len() != 2 {
            return None;
        }
        let (s1, u1, p1) = xs[0];
        let (s2, u2, p2) = xs[1];
        // The run of the path that is inside the panel, entry -> exit.
        let mut inside = Vec::with_capacity(s2 - s1 + 2);
        inside.push(p1);
        inside.extend_from_slice(&path[s1 + 1..=s2]);
        inside.push(p2);
        inside.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4);
        if inside.len() < 2 {
            return None;
        }
        // Arc from the exit forward to the entry, and the complementary one.
        let span_a = (u1 - u2).rem_euclid(n as f32);
        let mut a = inside.clone();
        a.extend(self.boundary_arc(u2, span_a));
        let mut b: Vec<[f32; 2]> = inside.iter().rev().copied().collect();
        b.extend(self.boundary_arc(u1, n as f32 - span_a));
        let (a, b) = (Frame { points: a }, Frame { points: b });
        if a.points.len() < 3 || b.points.len() < 3 {
            return None;
        }
        // Name the halves by the side of the entry->exit chord they fall on,
        // so the caller's two offset cuts pick matching pieces. The probe is
        // the midpoint of arc A on the OUTLINE — on a convex polygon every
        // point of that open arc is strictly on A's side of the chord, which
        // a centroid is not guaranteed to be once a half goes concave.
        let e = sub(p2, p1);
        let nrm = [e[1], -e[0]];
        let probe = self.boundary_point(u2 + span_a * 0.5);
        Some(if dot(nrm, sub(probe, p1)) <= 0.0 {
            (a, b)
        } else {
            (b, a)
        })
    }

    /// The outline point at boundary parameter `u` (`edge index + t`).
    fn boundary_point(&self, u: f32) -> [f32; 2] {
        let n = self.points.len();
        let u = u.rem_euclid(n as f32);
        let i = (u.floor() as usize) % n;
        let t = u - u.floor();
        let (a, b) = (self.points[i], self.points[(i + 1) % n]);
        [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
    }

    /// The polygon's vertices strictly inside the boundary arc that starts at
    /// parameter `from` and runs `span` forward (parameters are
    /// `edge index + t`; vertex `k` sits at parameter `k`).
    fn boundary_arc(&self, from: f32, span: f32) -> Vec<[f32; 2]> {
        let n = self.points.len();
        let mut out = Vec::new();
        for step in 0..n {
            let k = (from.floor() as usize + 1 + step) % n;
            let d = (k as f32 - from).rem_euclid(n as f32);
            if d > 1e-5 && d < span - 1e-5 {
                out.push(self.points[k]);
            }
        }
        out
    }

    /// FB-023/024 "Divide frame border equally": `cols` × `rows` equal cells
    /// with `gutter_x` between columns and `gutter_y` between rows, row-major
    /// (the reading order is recomputed from geometry, never from this order).
    ///
    /// `fit_to_side` is CSP's *Fit to Side Direction of Frame*: the grid runs
    /// along the panel's own edges, so a tilted panel divides along its slant.
    /// The edge chosen is the one closest to the page's horizontal, so
    /// "columns" keeps meaning columns on a tilted rectangle.
    ///
    /// Convex frames only. `None` when any cell would come out degenerate —
    /// the caller keeps the panel it had rather than shipping slivers.
    pub fn divide_equally(
        &self,
        cols: usize,
        rows: usize,
        gutter_x: f32,
        gutter_y: f32,
        fit_to_side: bool,
    ) -> Option<Vec<Frame>> {
        if !self.is_convex() || cols == 0 || rows == 0 || cols * rows < 2 {
            return None;
        }
        let u = if fit_to_side {
            self.flattest_edge_dir()
        } else {
            [1.0, 0.0]
        };
        let v = [-u[1], u[0]];
        let proj = |axis: [f32; 2]| -> (f32, f32) {
            let mut lo = f32::INFINITY;
            let mut hi = f32::NEG_INFINITY;
            for p in &self.points {
                let d = dot(axis, *p);
                lo = lo.min(d);
                hi = hi.max(d);
            }
            (lo, hi)
        };
        let (u0, u1) = proj(u);
        let (v0, v1) = proj(v);
        let (su, sv) = ((u1 - u0) / cols as f32, (v1 - v0) / rows as f32);
        let (gx, gy) = (gutter_x.max(0.0) * 0.5, gutter_y.max(0.0) * 0.5);
        let mut out = Vec::with_capacity(cols * rows);
        for j in 0..rows {
            for i in 0..cols {
                let lo_u = u0 + su * i as f32 + if i > 0 { gx } else { 0.0 };
                let hi_u = u0 + su * (i + 1) as f32 - if i + 1 < cols { gx } else { 0.0 };
                let lo_v = v0 + sv * j as f32 + if j > 0 { gy } else { 0.0 };
                let hi_v = v0 + sv * (j + 1) as f32 - if j + 1 < rows { gy } else { 0.0 };
                if hi_u <= lo_u || hi_v <= lo_v {
                    return None;
                }
                let cell = self
                    .clip_half_plane(u, hi_u)
                    .clip_half_plane([-u[0], -u[1]], -lo_u)
                    .clip_half_plane(v, hi_v)
                    .clip_half_plane([-v[0], -v[1]], -lo_v);
                if cell.points.len() < 3 || cell.area() < MIN_FRAME_AREA {
                    return None;
                }
                out.push(cell);
            }
        }
        Some(out)
    }

    /// The edge direction closest to the page's horizontal, pointing right.
    /// This is the "fit to side" axis: on a tilted rectangle it is the long
    /// side the artist thinks of as the panel's top.
    fn flattest_edge_dir(&self) -> [f32; 2] {
        let n = self.points.len();
        let mut best = [1.0f32, 0.0];
        let mut best_flat = -1.0f32;
        for i in 0..n {
            let e = sub(self.points[(i + 1) % n], self.points[i]);
            let len = dot(e, e).sqrt();
            if len < 1e-6 {
                continue;
            }
            let mut d = [e[0] / len, e[1] / len];
            if d[0] < 0.0 {
                d = [-d[0], -d[1]];
            }
            if d[0] > best_flat {
                best_flat = d[0];
                best = d;
            }
        }
        best
    }

    /// Axis-aligned bounds `[x0, y0, x1, y1]`.
    pub fn bbox(&self) -> [f32; 4] {
        let mut r = [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ];
        for p in &self.points {
            r[0] = r[0].min(p[0]);
            r[1] = r[1].min(p[1]);
            r[2] = r[2].max(p[0]);
            r[3] = r[3].max(p[1]);
        }
        r
    }
}

fn segment_distance(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let ab = sub(b, a);
    let len2 = dot(ab, ab);
    let t = if len2 < 1e-9 {
        0.0
    } else {
        (dot(sub(p, a), ab) / len2).clamp(0.0, 1.0)
    };
    let q = [a[0] + ab[0] * t, a[1] + ab[1] * t];
    dot(sub(p, q), sub(p, q)).sqrt()
}

fn segments_intersect(a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2]) -> bool {
    let d1 = cross(sub(b, a), sub(c, a));
    let d2 = cross(sub(b, a), sub(d, a));
    let d3 = cross(sub(d, c), sub(a, c));
    let d4 = cross(sub(d, c), sub(b, c));
    (d1 > 0.0) != (d2 > 0.0) && (d3 > 0.0) != (d4 > 0.0)
}

/// Where `a`–`b` crosses `c`–`d`: `(t along a→b, s along c→d)`, both in
/// `0..1`. Parallel or non-crossing = `None`. The parameters, not the point,
/// because [`Frame::halves_by_path`] orders crossings by them.
fn segment_cross(a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2]) -> Option<(f32, f32)> {
    let (r, s) = (sub(b, a), sub(d, c));
    let den = cross(r, s);
    if den.abs() < 1e-9 {
        return None;
    }
    let t = cross(sub(c, a), s) / den;
    let u = cross(sub(c, a), r) / den;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then_some((t, u))
}

/// Perpendicular offset of an OPEN polyline by `d` along `n = [e.y, -e.x]`
/// (the normal convention [`Frame::split`] uses), with both ends extended by
/// `reach` so the offset copy still crosses whatever the original crossed.
///
/// Corners use the averaged adjacent normal WITHOUT the miter length
/// correction: a gutter offset is small and a true miter spikes to infinity
/// at a hairpin, which on a panel cut is a crash and never a nicer border.
fn offset_path(path: &[[f32; 2]], d: f32, reach: f32) -> Vec<[f32; 2]> {
    let m = path.len();
    if m < 2 {
        return path.to_vec();
    }
    let norms: Vec<[f32; 2]> = path
        .windows(2)
        .map(|w| {
            let e = sub(w[1], w[0]);
            let len = dot(e, e).sqrt().max(1e-6);
            [e[1] / len, -e[0] / len]
        })
        .collect();
    let mut out: Vec<[f32; 2]> = Vec::with_capacity(m + 2);
    for (k, p) in path.iter().enumerate() {
        let nk = if k == 0 {
            norms[0]
        } else if k == m - 1 {
            norms[m - 2]
        } else {
            let (x, y) = (norms[k - 1], norms[k]);
            let s = [x[0] + y[0], x[1] + y[1]];
            let l = dot(s, s).sqrt();
            if l < 1e-6 { y } else { [s[0] / l, s[1] / l] }
        };
        out.push([p[0] + nk[0] * d, p[1] + nk[1] * d]);
    }
    if reach > 0.0 {
        let ext = |from: [f32; 2], toward: [f32; 2]| {
            let e = sub(from, toward);
            let l = dot(e, e).sqrt().max(1e-6);
            [from[0] + e[0] / l * reach, from[1] + e[1] / l * reach]
        };
        let last = ext(out[out.len() - 1], out[out.len() - 2]);
        let first = ext(out[0], out[1]);
        out.insert(0, first);
        out.push(last);
    }
    out
}

impl FrameSet {
    /// One rectangular frame — the "from default border" starting point.
    pub fn single_rect(rect: [f32; 4], border_px: f32) -> Self {
        Self {
            frames: vec![Frame::rect(rect[0], rect[1], rect[2], rect[3])],
            border_px,
            slot: None,
            reading_pin: None,
            border_ruler: false,
        }
    }

    /// `FB-053`/`FB-054`: the panel outlines as snapping polylines, for when
    /// [`Self::border_ruler`] is on and the artist inks the border by hand.
    /// Closed loops — the first point repeats at the end, because a curve
    /// ruler snaps to its SEGMENTS and the closing edge is a segment like any
    /// other. Empty when the border is ordinary ink.
    pub fn ruler_curves(&self) -> Vec<CurveRuler> {
        if !self.border_ruler {
            return Vec::new();
        }
        self.frames
            .iter()
            .filter(|f| f.points.len() >= 3)
            .map(|f| {
                let mut pts = f.points.clone();
                pts.push(f.points[0]);
                CurveRuler { pts }
            })
            .collect()
    }

    /// `FB-030` "Extend to canvas edge": run frame `frame`'s edge `edge` out
    /// until it leaves the page — the bleed panel in one tap.
    ///
    /// The edge translates along its own outward normal, so the panel grows
    /// without changing shape. Past the page is deliberate: the mask is
    /// clipped at rasterization, and a border that stops exactly ON the page
    /// edge still prints a hairline. `bleed` is how far past to go.
    ///
    /// CSP's second behaviour is here too: if another frame in this set faces
    /// that edge, the edge stops flush against it instead of the page — the
    /// **gutter between the two panels disappears**. Returns false when the
    /// edge is degenerate or already out (nothing to do, no undo entry).
    pub fn extend_to_edge(
        &mut self,
        frame: usize,
        edge: usize,
        canvas: (f32, f32),
        bleed: f32,
    ) -> bool {
        let Some(f) = self.frames.get(frame) else {
            return false;
        };
        let n = f.points.len();
        if n < 3 || edge >= n {
            return false;
        }
        let (a, b) = (f.points[edge], f.points[(edge + 1) % n]);
        let e = sub(b, a);
        let len = dot(e, e).sqrt();
        if len < 1e-3 {
            return false;
        }
        // Outward normal: away from the panel's interior.
        let c = f.centroid();
        let mut nrm = [e[1] / len, -e[0] / len];
        if dot(nrm, sub(a, c)) < 0.0 {
            nrm = [-nrm[0], -nrm[1]];
        }
        // How far each end must travel to clear the page, whichever page
        // side it meets first.
        let page = |p: [f32; 2]| -> f32 {
            let mut t = f32::INFINITY;
            for axis in 0..2 {
                let comp = nrm[axis];
                if comp.abs() < 1e-6 {
                    continue;
                }
                let bound = if comp > 0.0 {
                    if axis == 0 { canvas.0 } else { canvas.1 }
                } else {
                    0.0
                };
                t = t.min((bound - p[axis]) / comp);
            }
            t
        };
        let mut d = page(a).max(page(b)) + bleed;
        // A facing panel wins over the page: the gutter closes.
        if let Some(meet) = self.facing_distance(frame, a, b, nrm)
            && meet < d
        {
            d = meet;
        }
        if !d.is_finite() || d <= 0.01 {
            return false;
        }
        let f = &mut self.frames[frame];
        for k in [edge, (edge + 1) % n] {
            f.points[k][0] += nrm[0] * d;
            f.points[k][1] += nrm[1] * d;
        }
        true
    }

    /// Distance the edge `a`–`b` may travel along `nrm` before it lands on
    /// another frame of this set. `None` when nothing faces it. Only frames
    /// whose projection onto the edge's own direction OVERLAPS the edge count
    /// — a panel off to the side is not "the next panel over".
    fn facing_distance(&self, skip: usize, a: [f32; 2], b: [f32; 2], nrm: [f32; 2]) -> Option<f32> {
        let mut best: Option<f32> = None;
        for (i, o) in self.frames.iter().enumerate() {
            if i == skip {
                continue;
            }
            if let Some((near, _)) = facing_vertices(o, a, b, nrm) {
                best = Some(best.map_or(near, |x: f32| x.min(near)));
            }
        }
        best
    }

    /// Rasterize only the **border ink**: AA black stroke centred on every
    /// polygon edge, transparent everywhere else. This is a frame FOLDER's own
    /// raster — drawn above its masked children (true-isolation model).
    pub fn rasterize_border(&self, size: (u32, u32)) -> HashMap<TileIdx, Arc<Tile>> {
        // Border width 0 = CSP's "Draw border" unchecked at creation, and
        // `border_ruler` = the same switch thrown afterwards (FB-053): the
        // shape stays, the ink goes, the width is remembered.
        if self.border_ruler || self.border_px <= 0.05 {
            return HashMap::new();
        }
        let sdfs: Vec<FrameSdf> = self.frames.iter().map(FrameSdf::of).collect();
        let reach = self.border_px * 0.5 + 1.0;
        let tile_r = (TILE_SIZE as f32) * 0.5 * std::f32::consts::SQRT_2;

        let tiles_x = (size.0 as usize).div_ceil(TILE_SIZE) as i32;
        let tiles_y = (size.1 as usize).div_ceil(TILE_SIZE) as i32;
        let mut out = HashMap::new();

        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                let idx = TileIdx::new(tx, ty);
                let (ox, oy) = idx.origin();
                let center = [
                    ox as f32 + TILE_SIZE as f32 * 0.5,
                    oy as f32 + TILE_SIZE as f32 * 0.5,
                ];
                // Only tiles whose pixels can be within border reach of an
                // edge matter: |d| <= tile_r + reach for some frame.
                let near_edge = sdfs
                    .iter()
                    .any(|s| !s.is_empty() && s.dist(center).abs() <= tile_r + reach);
                if !near_edge {
                    continue;
                }

                let mut tile = Tile::new_transparent();
                let data = tile.data_mut();
                let mut any = false;
                for py in 0..TILE_SIZE {
                    for px in 0..TILE_SIZE {
                        let p = [ox as f32 + px as f32 + 0.5, oy as f32 + py as f32 + 0.5];
                        let mut border = 0.0f32;
                        for s in &sdfs {
                            if s.is_empty() {
                                continue;
                            }
                            let d = s.dist(p);
                            border =
                                border.max((self.border_px * 0.5 + 0.5 - d.abs()).clamp(0.0, 1.0));
                        }
                        if border <= 0.0 {
                            continue;
                        }
                        any = true;
                        let o = Tile::offset(px, py);
                        // Premultiplied black ink.
                        data[o + 3] = (border * FIX15_ONE as f32).round() as u16;
                    }
                }
                if any {
                    out.insert(idx, Arc::new(tile));
                }
            }
        }
        out
    }

    /// Rasterize the **panel-interior coverage mask**: opaque white inside the
    /// union of the panels (AA at the edges), absent outside. Compositors
    /// multiply a frame folder's group by this — an absent tile means zero
    /// coverage. Fully-inside tiles share **one** allocation.
    pub fn rasterize_mask(&self, size: (u32, u32)) -> HashMap<TileIdx, Arc<Tile>> {
        let sdfs: Vec<FrameSdf> = self.frames.iter().map(FrameSdf::of).collect();
        let tile_r = (TILE_SIZE as f32) * 0.5 * std::f32::consts::SQRT_2;
        let one = FIX15_ONE as u16;

        let tiles_x = (size.0 as usize).div_ceil(TILE_SIZE) as i32;
        let tiles_y = (size.1 as usize).div_ceil(TILE_SIZE) as i32;
        let mut out = HashMap::new();
        let mut white: Option<Arc<Tile>> = None;

        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                let idx = TileIdx::new(tx, ty);
                let (ox, oy) = idx.origin();
                let center = [
                    ox as f32 + TILE_SIZE as f32 * 0.5,
                    oy as f32 + TILE_SIZE as f32 * 0.5,
                ];
                let mut fully_inside = false;
                let mut near = false;
                for s in &sdfs {
                    if s.is_empty() {
                        continue;
                    }
                    let d = s.dist(center);
                    if d < -(tile_r + 1.0) {
                        fully_inside = true;
                    }
                    if d.abs() <= tile_r + 1.0 {
                        near = true;
                    }
                }
                if fully_inside && !near {
                    let w = white
                        .get_or_insert_with(|| {
                            let mut t = Tile::new_transparent();
                            t.data_mut().fill(one);
                            Arc::new(t)
                        })
                        .clone();
                    out.insert(idx, w);
                    continue;
                }
                if !fully_inside && !near {
                    continue; // fully outside every panel: zero coverage
                }

                let mut tile = Tile::new_transparent();
                let data = tile.data_mut();
                let mut any = false;
                for py in 0..TILE_SIZE {
                    for px in 0..TILE_SIZE {
                        let p = [ox as f32 + px as f32 + 0.5, oy as f32 + py as f32 + 0.5];
                        let mut inside = 0.0f32;
                        for s in &sdfs {
                            if s.is_empty() {
                                continue;
                            }
                            inside = inside.max((0.5 - s.dist(p)).clamp(0.0, 1.0));
                        }
                        if inside <= 0.0 {
                            continue;
                        }
                        any = true;
                        let o = Tile::offset(px, py);
                        let v = (inside * FIX15_ONE as f32).round() as u16;
                        data[o] = v;
                        data[o + 1] = v;
                        data[o + 2] = v;
                        data[o + 3] = v;
                    }
                }
                if any {
                    out.insert(idx, Arc::new(tile));
                }
            }
        }
        out
    }

    /// Frame containing `p`, topmost in list order.
    pub fn frame_at(&self, p: [f32; 2]) -> Option<usize> {
        self.frames.iter().rposition(|f| f.contains(p))
    }

    /// Rasterize to sparse tiles: white outside the union, black AA borders,
    /// transparent inside. Tiles that come out fully transparent are absent;
    /// tiles that come out fully white all share **one** `Arc` (a page is
    /// mostly gutter/margin — sharing keeps a B4 600dpi page at a few MB).
    pub fn rasterize(&self, size: (u32, u32)) -> HashMap<TileIdx, Arc<Tile>> {
        let sdfs: Vec<FrameSdf> = self.frames.iter().map(FrameSdf::of).collect();
        let reach = self.border_px * 0.5 + 1.0;
        let tile_r = (TILE_SIZE as f32) * 0.5 * std::f32::consts::SQRT_2;
        let one = FIX15_ONE as u16;

        let tiles_x = (size.0 as usize).div_ceil(TILE_SIZE) as i32;
        let tiles_y = (size.1 as usize).div_ceil(TILE_SIZE) as i32;
        let mut out = HashMap::new();
        let mut white: Option<Arc<Tile>> = None;

        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                let idx = TileIdx::new(tx, ty);
                let (ox, oy) = idx.origin();
                let center = [
                    ox as f32 + TILE_SIZE as f32 * 0.5,
                    oy as f32 + TILE_SIZE as f32 * 0.5,
                ];

                // Conservative classification from each frame's distance at the
                // tile center: beyond (tile radius + border reach) the frame
                // cannot touch any pixel of this tile.
                let mut fully_inside_one = false;
                let mut all_far_out = true;
                let mut needs_pixels = false;
                for s in &sdfs {
                    if s.is_empty() {
                        continue;
                    }
                    let d = s.dist(center);
                    if d < -(tile_r + reach) {
                        fully_inside_one = true;
                    } else if d <= tile_r + reach {
                        needs_pixels = true;
                    }
                    if d <= tile_r + reach {
                        all_far_out = false;
                    }
                }

                if fully_inside_one && !needs_pixels {
                    continue; // deep inside a panel: transparent, no tile
                }
                if all_far_out {
                    let w = white
                        .get_or_insert_with(|| {
                            let mut t = Tile::new_transparent();
                            t.data_mut().fill(one);
                            Arc::new(t)
                        })
                        .clone();
                    out.insert(idx, w);
                    continue;
                }

                // Boundary tile: per-pixel coverage.
                let mut tile = Tile::new_transparent();
                let data = tile.data_mut();
                let mut any = false;
                for py in 0..TILE_SIZE {
                    for px in 0..TILE_SIZE {
                        let p = [ox as f32 + px as f32 + 0.5, oy as f32 + py as f32 + 0.5];
                        let mut inside = 0.0f32;
                        let mut border = 0.0f32;
                        for s in &sdfs {
                            if s.is_empty() {
                                continue;
                            }
                            let d = s.dist(p);
                            inside = inside.max((0.5 - d).clamp(0.0, 1.0));
                            if self.border_px > 0.05 {
                                border = border
                                    .max((self.border_px * 0.5 + 0.5 - d.abs()).clamp(0.0, 1.0));
                            }
                        }
                        let w = 1.0 - inside;
                        let b = border;
                        // black border over white gutter, premultiplied:
                        let alpha = b + w * (1.0 - b);
                        let rgb = w * (1.0 - b);
                        if alpha <= 0.0 {
                            continue;
                        }
                        any = true;
                        let o = Tile::offset(px, py);
                        let rv = (rgb * FIX15_ONE as f32).round() as u16;
                        let av = (alpha * FIX15_ONE as f32).round() as u16;
                        data[o] = rv;
                        data[o + 1] = rv;
                        data[o + 2] = rv;
                        data[o + 3] = av;
                    }
                }
                if any {
                    out.insert(idx, Arc::new(tile));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The geometry "Keep gutters aligned" rides on: which panel is across
    /// the gutter, and which of its corners form the border facing back.
    #[test]
    fn facing_vertices_finds_the_panel_across_the_gutter() {
        // A's right edge, points 1 -> 2 of a rect, outward normal +x.
        let a = [100.0, 0.0];
        let b = [100.0, 100.0];
        let nrm = [1.0, 0.0];

        // Straight across a 40 px gutter: both left corners, at 40.
        let across = Frame::rect(140.0, 0.0, 240.0, 100.0);
        let (d, idx) = facing_vertices(&across, a, b, nrm).expect("faces the edge");
        assert!((d - 40.0).abs() < 1e-4, "gutter width: {d}");
        assert_eq!(idx.len(), 2, "the whole facing border: {idx:?}");
        for k in &idx {
            assert!((across.points[*k][0] - 140.0).abs() < 1e-4);
        }

        // Two stacked panels share one gutter: each reports 40.
        let top = Frame::rect(140.0, 0.0, 240.0, 50.0);
        let bot = Frame::rect(140.0, 50.0, 240.0, 100.0);
        for f in [&top, &bot] {
            assert!((facing_vertices(f, a, b, nrm).unwrap().0 - 40.0).abs() < 1e-4);
        }

        // Off to the side (no projection overlap onto the edge's own
        // direction) is BESIDE this panel, not facing it.
        let beside = Frame::rect(140.0, 120.0, 240.0, 200.0);
        assert!(facing_vertices(&beside, a, b, nrm).is_none());

        // Behind the edge (the wrong side) does not face it either.
        let behind = Frame::rect(-100.0, 0.0, -20.0, 100.0);
        assert!(facing_vertices(&behind, a, b, nrm).is_none());

        // A skewed neighbour reports its NEAREST corner, and only that one
        // is the facing vertex.
        let skew = Frame {
            points: vec![[140.0, 0.0], [240.0, 0.0], [240.0, 100.0], [180.0, 100.0]],
        };
        let (d, idx) = facing_vertices(&skew, a, b, nrm).unwrap();
        assert!((d - 40.0).abs() < 1e-4);
        assert_eq!(idx, vec![0], "only the near corner: {idx:?}");
    }

    #[test]
    fn rotate_and_scale_around_anchor() {
        // Clockwise (y-down) quarter turn around the centre permutes corners.
        let mut f = Frame::rect(0.0, 0.0, 10.0, 10.0);
        f.rotate_around([5.0, 5.0], std::f32::consts::FRAC_PI_2);
        assert!((f.points[0][0] - 10.0).abs() < 1e-4 && f.points[0][1].abs() < 1e-4);
        assert!((f.centroid()[0] - 5.0).abs() < 1e-4, "centroid pinned");

        let mut g = Frame::rect(0.0, 0.0, 10.0, 10.0);
        g.scale_around([0.0, 0.0], 2.0, 3.0);
        assert!((g.points[2][0] - 20.0).abs() < 1e-4 && (g.points[2][1] - 30.0).abs() < 1e-4);
    }

    #[test]
    fn rect_is_convex_and_contains_its_center() {
        let f = Frame::rect(10.0, 10.0, 110.0, 210.0);
        assert!(f.is_convex());
        assert_eq!(f.area(), 100.0 * 200.0);
        assert!(f.contains([60.0, 110.0]));
        assert!(!f.contains([5.0, 5.0]));
        assert!((f.distance([60.0, 10.0])).abs() < 1e-3, "on the top edge");
        assert!((f.distance([60.0, 5.0]) - 5.0).abs() < 1e-3);
    }

    #[test]
    fn split_leaves_a_gutter_and_stays_convex() {
        let f = Frame::rect(0.0, 0.0, 200.0, 100.0);
        // Vertical cut at x=100, 8px gutter.
        let (a, b) = f.split([100.0, -10.0], [100.0, 110.0], 8.0).unwrap();
        assert!(a.is_convex() && b.is_convex());
        let (left, right) = if a.centroid()[0] < b.centroid()[0] {
            (a, b)
        } else {
            (b, a)
        };
        assert!((left.area() - 96.0 * 100.0).abs() < 1.0);
        assert!((right.area() - 96.0 * 100.0).abs() < 1.0);
        assert!((left.bbox()[2] - 96.0).abs() < 1e-2);
        assert!((right.bbox()[0] - 104.0).abs() < 1e-2);
    }

    #[test]
    fn diagonal_split_still_convex() {
        let f = Frame::rect(0.0, 0.0, 100.0, 100.0);
        let (a, b) = f.split([0.0, 20.0], [100.0, 80.0], 6.0).unwrap();
        assert!(a.is_convex() && b.is_convex());
        assert!((a.area() + b.area()) < f.area());
    }

    #[test]
    fn sliver_cuts_are_refused() {
        let f = Frame::rect(0.0, 0.0, 100.0, 100.0);
        assert!(
            f.split([2.0, -1.0], [2.0, 101.0], 6.0).is_none(),
            "sliver side"
        );
        assert!(
            f.split([50.0, -1.0], [50.0, 101.0], 300.0).is_none(),
            "gutter eats all"
        );
        assert!(
            f.split([50.0, 50.0], [50.0, 50.0], 6.0).is_none(),
            "zero-length line"
        );
    }

    #[test]
    fn segment_touch_tests() {
        let f = Frame::rect(0.0, 0.0, 100.0, 100.0);
        assert!(f.segment_touches([50.0, -10.0], [50.0, 110.0]), "crosses");
        assert!(
            f.segment_touches([50.0, 50.0], [200.0, 50.0]),
            "starts inside"
        );
        assert!(!f.segment_touches([150.0, -10.0], [150.0, 110.0]), "misses");
    }

    #[test]
    fn vertex_and_edge_hit_tests() {
        let f = Frame::rect(0.0, 0.0, 100.0, 100.0);
        assert_eq!(f.vertex_near([98.0, 3.0], 5.0), Some(1));
        assert_eq!(f.vertex_near([50.0, 50.0], 5.0), None);
        assert_eq!(f.edge_near([50.0, 2.0], 5.0), Some(0));
        assert_eq!(f.edge_near([98.0, 50.0], 5.0), Some(1));
    }

    /// An L-shape: the classic concave test subject.
    fn ell() -> Frame {
        Frame {
            points: vec![
                [20.0, 20.0],
                [120.0, 20.0],
                [120.0, 70.0],
                [70.0, 70.0],
                [70.0, 120.0],
                [20.0, 120.0],
            ],
        }
    }

    #[test]
    fn concave_frames_have_exact_sdf_and_refuse_splits() {
        let f = ell();
        assert!(!f.is_convex());
        assert!(f.is_simple());
        assert!(f.contains([40.0, 40.0]), "inside the L's body");
        assert!(!f.contains([100.0, 100.0]), "the notch is outside");
        assert!((f.distance([40.0, 20.0])).abs() < 1e-3, "on the top edge");
        assert!(
            (f.distance([100.0, 100.0]) - 30.0).abs() < 0.1,
            "30 px from both notch edges"
        );
        assert!(
            f.split([70.0, 0.0], [70.0, 140.0], 6.0).is_none(),
            "concave never splits"
        );

        let mut bow = f.clone();
        bow.points.swap(1, 4);
        assert!(!bow.is_simple(), "self-intersection detected");
    }

    #[test]
    fn concave_frame_rasterizes_notch_as_gutter() {
        let fs = FrameSet {
            frames: vec![ell()],
            border_px: 3.0,
            slot: None,
            reading_pin: None,
            border_ruler: false,
        };
        let tiles = fs.rasterize((192, 192));
        let px = |x: i32, y: i32| -> [u16; 4] {
            let idx = TileIdx::of_pixel(x, y);
            let (ox, oy) = idx.origin();
            tiles
                .get(&idx)
                .map(|t| t.pixel((x - ox) as usize, (y - oy) as usize))
                .unwrap_or([0, 0, 0, 0])
        };
        let one = FIX15_ONE as u16;
        assert_eq!(px(40, 40), [0, 0, 0, 0], "inside the L = transparent");
        assert_eq!(
            px(100, 100),
            [one, one, one, one],
            "the notch = white gutter"
        );
        assert_eq!(px(160, 160), [one, one, one, one], "far outside = white");
        // Coverage mask agrees: inside opaque, notch absent/zero.
        let mask = fs.rasterize_mask((192, 192));
        let midx = TileIdx::of_pixel(40, 40);
        assert!(mask.get(&midx).is_some_and(|t| t.pixel(40, 40)[3] == one));
        let nidx = TileIdx::of_pixel(100, 100);
        let ncov = mask
            .get(&nidx)
            .map(|t| t.pixel(100 - 64, 100 - 64)[3])
            .unwrap_or(0);
        assert_eq!(ncov, 0, "notch has zero panel coverage");
    }

    #[test]
    fn zero_border_means_no_ink() {
        let fs = FrameSet::single_rect([32.0, 32.0, 96.0, 96.0], 0.0);
        assert!(
            fs.rasterize_border((128, 128)).is_empty(),
            "Draw border off = no ink"
        );
        // The flat raster still masks (white gutter), just without a stroke.
        let tiles = fs.rasterize((128, 128));
        let idx = TileIdx::of_pixel(64, 32);
        let edge = tiles.get(&idx).map(|t| t.pixel(0, 32)).unwrap_or([0; 4]);
        assert!(edge[0] == edge[3], "edge pixel is white-ish, not ink");
    }

    #[test]
    fn rasterize_white_outside_transparent_inside_ink_on_edge() {
        let fs = FrameSet::single_rect([64.0, 64.0, 192.0, 192.0], 4.0);
        let tiles = fs.rasterize((256, 256));

        let px = |x: i32, y: i32| -> [u16; 4] {
            let idx = TileIdx::of_pixel(x, y);
            let (ox, oy) = idx.origin();
            tiles
                .get(&idx)
                .map(|t| t.pixel((x - ox) as usize, (y - oy) as usize))
                .unwrap_or([0, 0, 0, 0])
        };

        let one = FIX15_ONE as u16;
        assert_eq!(px(8, 8), [one, one, one, one], "far outside = opaque white");
        assert_eq!(px(128, 128), [0, 0, 0, 0], "panel interior = transparent");
        let edge = px(128, 64);
        assert_eq!(edge[3], one, "border center is opaque");
        assert!(edge[0] < one / 8, "border center is ink-black");
    }

    #[test]
    fn rasterize_shares_one_white_tile() {
        let fs = FrameSet::single_rect([300.0, 300.0, 340.0, 340.0], 4.0);
        let tiles = fs.rasterize((1024, 1024));
        let whites: Vec<_> = tiles
            .values()
            .filter(|t| {
                t.pixel(0, 0)[3] == FIX15_ONE as u16 && t.pixel(63, 63)[3] == FIX15_ONE as u16
            })
            .collect();
        assert!(
            whites.len() > 100,
            "a small frame on a big page is mostly gutter"
        );
        let first = Arc::as_ptr(whites[0]);
        assert!(
            whites.iter().filter(|t| Arc::as_ptr(t) == first).count() >= whites.len() - 8,
            "fully-white tiles share one allocation"
        );
    }

    #[test]
    fn empty_frameset_rasterizes_to_all_white() {
        let fs = FrameSet {
            frames: vec![],
            border_px: 4.0,
            slot: None,
            reading_pin: None,
            border_ruler: false,
        };
        let tiles = fs.rasterize((128, 128));
        assert_eq!(tiles.len(), 4);
        assert!(
            tiles
                .values()
                .all(|t| t.pixel(32, 32) == [FIX15_ONE as u16; 4])
        );
    }

    // --- C-061/062: the shape of the division ---------------------------

    /// A 2-point path is a straight cut and must land on `split`'s exact
    /// half-plane answer — otherwise the polyline tool and the line tool
    /// would disagree by a pixel at the limit and artists would notice.
    #[test]
    fn a_two_point_path_is_the_straight_cut() {
        let f = Frame::rect(0.0, 0.0, 200.0, 100.0);
        let straight = f.split([100.0, -10.0], [100.0, 110.0], 8.0).unwrap();
        let path = f
            .split_path(&[[100.0, -10.0], [100.0, 110.0]], 8.0)
            .unwrap();
        assert_eq!(straight, path);
    }

    /// A dog-leg cut: the halves keep every vertex of the path (so the cut
    /// edge is the drawn shape, not its chord), they tile the panel minus
    /// the gutter, and the gutter is really there.
    #[test]
    fn a_bent_path_cuts_a_bent_edge() {
        let f = Frame::rect(0.0, 0.0, 200.0, 100.0);
        let path = [[100.0, -10.0], [60.0, 50.0], [100.0, 110.0]];
        let (a, b) = f.split_path(&path, 8.0).unwrap();
        // The elbow is on both cut edges, pushed apart by the gutter.
        let near_elbow = |g: &Frame| {
            g.points
                .iter()
                .filter(|p| (p[1] - 50.0).abs() < 12.0 && p[0] > 30.0 && p[0] < 90.0)
                .count()
        };
        assert!(near_elbow(&a) >= 1 && near_elbow(&b) >= 1, "elbow survived");
        assert!(a.is_simple() && b.is_simple());
        // Bent cut, so at least one half must be concave — that is the
        // whole point of the feature.
        assert!(!a.is_convex() || !b.is_convex(), "a bent cut bends a panel");
        let lost = f.area() - a.area() - b.area();
        assert!(lost > 0.0, "the gutter costs area");
        // The gutter is ~8 px over a ~120 px long cut, plus the elbow.
        assert!(
            lost < 8.0 * 260.0,
            "and not much more than the gutter: {lost}"
        );
        // Each half keeps two of the panel's own corners.
        assert!(a.area() > 4000.0 && b.area() > 4000.0);
    }

    /// The refusals: a path that never reaches the panel, one that leaves
    /// and comes back (two cuts, no single answer), and a concave subject.
    #[test]
    fn path_cuts_that_have_no_honest_answer_are_refused() {
        let f = Frame::rect(0.0, 0.0, 200.0, 100.0);
        assert!(
            f.split_path(&[[300.0, -10.0], [320.0, 50.0], [300.0, 110.0]], 8.0)
                .is_none(),
            "never touches the panel"
        );
        assert!(
            f.split_path(
                &[
                    [100.0, -10.0],
                    [100.0, 30.0],
                    [-20.0, 30.0],
                    [-20.0, 70.0],
                    [100.0, 70.0],
                    [100.0, 110.0],
                ],
                8.0
            )
            .is_none(),
            "leaves the panel and comes back: four crossings, no two halves"
        );
        assert!(
            ell()
                .split_path(&[[70.0, 0.0], [60.0, 60.0], [70.0, 140.0]], 6.0)
                .is_none(),
            "concave subjects are refused, exactly like split"
        );
    }

    // --- FB-023/024: divide the border equally --------------------------

    #[test]
    fn equal_division_makes_a_grid_with_gutters() {
        let f = Frame::rect(0.0, 0.0, 300.0, 200.0);
        let cells = f.divide_equally(3, 2, 10.0, 20.0, false).unwrap();
        assert_eq!(cells.len(), 6);
        // 3 columns of 100 px lose 10 px of gutter each except at the page
        // edges: the outer two are 95 wide, the middle one 90.
        let mut widths: Vec<f32> = cells[..3]
            .iter()
            .map(|c| c.bbox()[2] - c.bbox()[0])
            .collect();
        widths.sort_by(f32::total_cmp);
        assert!((widths[0] - 90.0).abs() < 0.01, "middle column: {widths:?}");
        assert!((widths[2] - 95.0).abs() < 0.01, "outer column: {widths:?}");
        // 2 rows of 100 with a 20 px gutter = 90 each.
        for c in &cells {
            assert!((c.bbox()[3] - c.bbox()[1] - 90.0).abs() < 0.01);
        }
        // Every cell is inside the original and they never overlap.
        assert!(cells.iter().all(|c| c.is_convex()));
        let total: f32 = cells.iter().map(|c| c.area()).sum();
        assert!(total < f.area(), "the gutters cost area");
    }

    /// *Fit to Side Direction of Frame*: a tilted panel divides along its
    /// own slant. The cut edges come out parallel to the panel's long side,
    /// not to the page — which is the only reason the option exists.
    #[test]
    fn fit_to_side_divides_along_the_panels_own_slant() {
        // A square rotated 20°.
        let mut f = Frame::rect(0.0, 0.0, 200.0, 200.0);
        f.rotate_around([100.0, 100.0], 20f32.to_radians());
        let square = f.divide_equally(2, 1, 0.0, 0.0, false).unwrap();
        let slanted = f.divide_equally(2, 1, 0.0, 0.0, true).unwrap();
        assert_eq!((square.len(), slanted.len()), (2, 2));
        // True-vertical division: the two halves have equal bboxes width-wise.
        let bw = |c: &Frame| c.bbox()[2] - c.bbox()[0];
        assert!((bw(&square[0]) - bw(&square[1])).abs() < 0.5);
        // Slant division: each half is the tilted rectangle's half, so its
        // bbox is WIDER than half the panel's bbox (the slant sticks out).
        assert!(
            bw(&slanted[0]) > bw(&square[0]) + 1.0,
            "the slanted halves are not axis-aligned: {} vs {}",
            bw(&slanted[0]),
            bw(&square[0])
        );
        // Area is conserved either way with no gutter.
        for cells in [&square, &slanted] {
            let total: f32 = cells.iter().map(|c| c.area()).sum();
            assert!((total - f.area()).abs() < f.area() * 0.01);
        }
    }

    #[test]
    fn equal_division_refuses_slivers_and_nonsense() {
        let f = Frame::rect(0.0, 0.0, 300.0, 200.0);
        assert!(f.divide_equally(0, 2, 0.0, 0.0, false).is_none(), "no cols");
        assert!(
            f.divide_equally(1, 1, 0.0, 0.0, false).is_none(),
            "1x1 is not a division"
        );
        assert!(
            f.divide_equally(4, 4, 200.0, 200.0, false).is_none(),
            "gutters wider than the cells"
        );
        assert!(
            ell().divide_equally(2, 2, 4.0, 4.0, false).is_none(),
            "concave subjects are refused"
        );
    }

    // --- FB-030: extend to the canvas edge -------------------------------

    #[test]
    fn extend_to_edge_runs_the_panel_off_the_page() {
        let mut fs = FrameSet::single_rect([50.0, 60.0, 250.0, 160.0], 4.0);
        // Edge 0 is the top (y=60) — it runs up past y=0 by the bleed.
        assert!(fs.extend_to_edge(0, 0, (400.0, 300.0), 6.0));
        let bb = fs.frames[0].bbox();
        assert!(
            (bb[1] + 6.0).abs() < 0.01,
            "top is 6 px past the page: {bb:?}"
        );
        assert!(
            (bb[0] - 50.0).abs() < 0.01 && (bb[2] - 250.0).abs() < 0.01,
            "sides unmoved"
        );
        assert!((bb[3] - 160.0).abs() < 0.01, "bottom unmoved");
    }

    /// The second half of FB-030: tapped between two panels, the edge stops
    /// on the neighbour instead of the page — the gutter closes.
    #[test]
    fn extend_to_edge_closes_the_gutter_against_a_neighbour() {
        let mut fs = FrameSet::single_rect([20.0, 20.0, 180.0, 100.0], 4.0);
        fs.frames.push(Frame::rect(20.0, 120.0, 180.0, 260.0));
        // Bottom edge of the top panel (y=100) toward the panel at y=120.
        assert!(fs.extend_to_edge(0, 2, (400.0, 300.0), 6.0));
        assert!(
            (fs.frames[0].bbox()[3] - 120.0).abs() < 0.01,
            "stopped flush on the neighbour, not at the page: {:?}",
            fs.frames[0].bbox()
        );
        // A panel off to the SIDE is not "the next panel over": nothing
        // faces the top edge, so it goes to the page.
        assert!(fs.extend_to_edge(0, 0, (400.0, 300.0), 6.0));
        assert!((fs.frames[0].bbox()[1] + 6.0).abs() < 0.01);
    }

    // --- FB-053/054: the border as a ruler -------------------------------

    #[test]
    fn border_as_ruler_drops_the_ink_and_keeps_the_width() {
        let mut fs = FrameSet::single_rect([16.0, 16.0, 112.0, 112.0], 6.0);
        assert!(
            !fs.rasterize_border((128, 128)).is_empty(),
            "ink by default"
        );
        assert!(fs.ruler_curves().is_empty(), "and no ruler while it inks");

        fs.border_ruler = true;
        assert!(fs.rasterize_border((128, 128)).is_empty(), "no ink at all");
        assert_eq!(fs.border_px, 6.0, "the width is remembered, not zeroed");
        let curves = fs.ruler_curves();
        assert_eq!(curves.len(), 1);
        assert_eq!(curves[0].pts.len(), 5, "closed loop: 4 corners + the first");
        assert_eq!(curves[0].pts[0], curves[0].pts[4]);

        // The panel still masks its children — only the ink went away.
        assert!(!fs.rasterize_mask((128, 128)).is_empty());

        fs.border_ruler = false;
        assert!(!fs.rasterize_border((128, 128)).is_empty(), "ink came back");
    }
}
