//! Row 156 / `FG-020` — Smart Shape recognition: what was that wobbly
//! stroke *meant* to be?
//!
//! Pure geometry, no app state, no clock, no RNG. The app hands over a
//! freehand path in canvas px and gets back either a clean figure to ink in
//! its place, or [`None`] — and `None` is the important half. A hatch mark,
//! a scribble, a bit of cross-hatching and a considered wobble that is
//! *supposed* to be wobbly must all come back `None`, because the failure
//! mode of a shape recognizer is not "it missed one", it is "it ate the
//! drawing". Every threshold here is therefore tuned to REFUSE first.
//!
//! ## The pipeline
//!
//! 1. **Floor** — drop the stroke if it is too short or too small to be a
//!    figure at all ([`MIN_STROKE_PX`], [`MIN_SPAN_PX`]). This is the
//!    hatch-mark gate and it runs before any fitting.
//! 2. **Resample** — [`RESAMPLE_N`] points spaced evenly along the arc.
//!    Even spacing is what makes every later measure scale-free, and the
//!    spacing (a big fraction of the stroke) is what makes hand jitter stop
//!    mattering without a smoothing pass that would also erase real corners.
//! 3. **Closedness** — endpoint gap against arc length ([`CLOSE_GAP_FRAC`]),
//!    plus an ENCLOSURE test ([`MIN_ENCLOSURE`]): the isoperimetric ratio
//!    `4π|A|/L²`, which is 1 for a circle, 0.79 for a square and ~0 for a
//!    there-and-back scrub that ends where it started without enclosing
//!    anything. Endpoint proximity alone would call that scrub a loop.
//! 4. **Complexity gates** — arc length against bbox diagonal
//!    ([`MAX_LEN_RATIO_OPEN`] / [`MAX_LEN_RATIO_CLOSED`]) and SIGNED total
//!    turning ([`MAX_TURN_CLOSED`] / [`MAX_TURN_OPEN`]). Together these are
//!    the scribble gate: a zigzag and a scrubbed-out mistake blow the length
//!    ratio, a spiral or a thrice-traced loop blows the turning.
//!
//!    Turning is measured SIGNED on purpose. Absolute turning is dominated
//!    by hand tremor — at any sane resample spacing the noise alone sums
//!    past a full turn, so an absolute-turning gate refuses every real
//!    stroke (it did, first time). Signed turning is a topological count of
//!    the loops the hand actually made and the tremor cancels out of it.
//! 5. **Fit candidates** and take the best RESIDUAL, where residual is
//!    always the RMS distance from the resampled points to the candidate's
//!    outline divided by the bbox diagonal — one comparable number for
//!    every shape. Under [`FIT_TOL`] it wins; over it, nothing does.
//!
//! Ties break toward the SIMPLER shape (circle over ellipse, round over
//! polygon unless the polygon is clearly better — [`POLY_MARGIN`]), because
//! "the circle you meant" is the whole promise of the row.

use std::f32::consts::{PI, TAU};

/// Points on the resampled path. Big enough that a 6-gon's corners survive
/// and small enough that a 2 px hand tremor is far below the spacing.
pub const RESAMPLE_N: usize = 64;
/// Arc-length floor, canvas px. Below this the mark is a hatch, a tick or a
/// dot — never a figure. The single most important constant in the file.
pub const MIN_STROKE_PX: f32 = 48.0;
/// Bounding-box diagonal floor, canvas px. Catches the long thin scrub that
/// clears [`MIN_STROKE_PX`] by going back and forth in one place.
pub const MIN_SPAN_PX: f32 = 24.0;
/// Endpoint gap / arc length, at or under which the stroke reads as closed.
pub const CLOSE_GAP_FRAC: f32 = 0.25;
/// Isoperimetric ratio `4π|A|/L²` a closed loop must reach: 1.0 for a
/// circle, 0.79 a square, 0.6 a triangle, 0.18 a 15:1 sliver — and ~0 for a
/// stroke that came back to its start without going round anything.
pub const MIN_ENCLOSURE: f32 = 0.10;
/// Arc length / bbox diagonal a closed figure may reach. Circle 3.14,
/// hexagon 3.0, thin ellipse 2.1 — a flower or a star runs well past it.
pub const MAX_LEN_RATIO_CLOSED: f32 = 5.0;
/// The same for an open stroke. A line is 1.0, a big sweep 1.4, a 270° "C"
/// 2.8; a zigzag or a scrubbed-out mistake is 6 and up.
pub const MAX_LEN_RATIO_OPEN: f32 = 3.2;
/// SIGNED total turning a closed figure may have, in turns. One loop is
/// exactly 1.0 whatever its shape, so this catches spirals and the loop
/// traced round three times, which every residual test would happily accept.
pub const MAX_TURN_CLOSED: f32 = 1.6;
/// The same for an open stroke.
pub const MAX_TURN_OPEN: f32 = 1.1;
/// The sharpest single step an open smooth curve may contain, radians. A
/// cusp this hard means the artist drew a corner, not a curve.
pub const MAX_CURVE_CUSP: f32 = 1.1;
/// Residual (RMS distance to the candidate outline / bbox diagonal) at or
/// under which a fit is accepted. Above it the stroke is left as drawn.
pub const FIT_TOL: f32 = 0.045;
/// Douglas–Peucker epsilon as a fraction of the bbox diagonal, for the
/// corner hunt. Wide enough that wobble adds no vertices.
pub const DP_EPS_FRAC: f32 = 0.055;
/// A polygon fit has to beat the round fit by this factor to win. Without
/// it a jittered circle would happily come back as a regular octagon.
pub const POLY_MARGIN: f32 = 0.7;
/// Most corners a recognized polygon may have. Past this every convex blob
/// fits something, so "polygon" stops meaning anything.
pub const MAX_POLY_SIDES: usize = 8;
/// Axis ratio inside which an ellipse is reported as a circle instead.
pub const CIRCLE_RATIO: f32 = 0.88;
/// Side-length coefficient of variation a regular polygon may have.
pub const POLY_SIDE_CV: f32 = 0.22;

/// What the stroke turned out to be.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShapeKind {
    /// A straight line between the two ends.
    Line,
    /// A rectangle, possibly rotated — `FG-020`'s "true rectangle".
    Rect,
    /// An ellipse with its own axes and rotation.
    Ellipse,
    /// An ellipse whose axes matched: reported separately because "the
    /// circle you meant" is the row's own headline.
    Circle,
    /// A regular polygon with this many sides (3..=[`MAX_POLY_SIDES`]).
    Polygon(usize),
    /// An open smooth curve — the wobble taken out of a swoosh.
    Curve,
}

impl ShapeKind {
    /// The status-line / overlay name, lowercase for mid-sentence use.
    pub fn label(self) -> &'static str {
        match self {
            ShapeKind::Line => "line",
            ShapeKind::Rect => "rectangle",
            ShapeKind::Ellipse => "ellipse",
            ShapeKind::Circle => "circle",
            ShapeKind::Polygon(3) => "triangle",
            ShapeKind::Polygon(5) => "pentagon",
            ShapeKind::Polygon(6) => "hexagon",
            ShapeKind::Polygon(_) => "polygon",
            ShapeKind::Curve => "curve",
        }
    }

    /// Does the path close back on itself? The ink path and the overlay both
    /// need it and must not disagree.
    pub fn closed(self) -> bool {
        matches!(
            self,
            ShapeKind::Rect | ShapeKind::Ellipse | ShapeKind::Circle | ShapeKind::Polygon(_)
        )
    }
}

/// One accepted recognition: what it is, the path to ink, and how well it
/// fitted (smaller is better; see [`FIT_TOL`] for the scale).
#[derive(Clone, Debug)]
pub struct Recognized {
    pub kind: ShapeKind,
    /// Canvas-px outline. Closed shapes do NOT repeat the first point — the
    /// inker closes them (see `ShapeKind::closed`).
    pub path: Vec<[f32; 2]>,
    pub residual: f32,
}

impl Recognized {
    pub fn closed(&self) -> bool {
        self.kind.closed()
    }

    /// The path's bounding box, `[x0, y0, x1, y1]`.
    pub fn bbox(&self) -> [f32; 4] {
        bbox(&self.path)
    }
}

// --- the entry point -----------------------------------------------------

/// Classify a freehand stroke. `None` means "leave it exactly as drawn",
/// which is the answer for anything the fits do not clearly explain.
pub fn recognize(raw: &[[f32; 2]]) -> Option<Recognized> {
    let pts = dedupe(raw);
    if pts.len() < 4 {
        return None;
    }
    let len = arc_len(&pts);
    let diag = bbox_diag(&pts);
    // Gate 1: too small to be a figure. Runs before every fit so a 12 px
    // hatch mark can never cost anything but this compare.
    if len < MIN_STROKE_PX || diag < MIN_SPAN_PX {
        return None;
    }
    let s = resample(&pts, RESAMPLE_N);
    // Closed = ends together AND something actually enclosed. The area test
    // is what separates a loop from a stroke scrubbed back over itself.
    let gap = dist(s[0], s[s.len() - 1]);
    let enclosure = 4.0 * PI * shoelace(&s).abs() / (len * len).max(1e-6);
    let closed = gap <= CLOSE_GAP_FRAC * len && enclosure >= MIN_ENCLOSURE;

    // Gate 2: the scribble gate, in two independent halves — see the module
    // header for why the turning half is SIGNED.
    let (max_ratio, max_turn) = if closed {
        (MAX_LEN_RATIO_CLOSED, MAX_TURN_CLOSED)
    } else {
        (MAX_LEN_RATIO_OPEN, MAX_TURN_OPEN)
    };
    if len > max_ratio * diag {
        return None;
    }
    if signed_turning(&s, closed).abs() / TAU > max_turn {
        return None;
    }

    let best = if closed {
        fit_closed(&s, diag)
    } else {
        fit_open(&s, diag)
    }?;
    (best.residual <= FIT_TOL).then_some(best)
}

// --- closed shapes -------------------------------------------------------

fn fit_closed(s: &[[f32; 2]], diag: f32) -> Option<Recognized> {
    let round = fit_round(s, diag);
    let poly = fit_poly(s, diag);
    match (round, poly) {
        (Some(r), Some(p)) => {
            // The polygon has to be CLEARLY better, not merely better: a
            // jittered circle fits an octagon well enough to steal it
            // otherwise, and a circle turned into an octagon is exactly the
            // "it ate my drawing" failure this row must not have.
            Some(if p.residual < r.residual * POLY_MARGIN {
                p
            } else {
                r
            })
        }
        (r, None) => r,
        (None, p) => p,
    }
}

/// Circle first, ellipse only if the circle is not nearly as good — the
/// simpler shape wins ties, because a hand-drawn circle is never perfectly
/// round and an ellipse fit will always shave a little off.
fn fit_round(s: &[[f32; 2]], diag: f32) -> Option<Recognized> {
    // The centre. For a full loop sampled evenly BY ARC LENGTH the centroid
    // is the centre exactly (both a circle and an ellipse are symmetric
    // under a half turn about it); the hand's open gap is the only bias.
    let c = centroid(s);
    // Circle: the radius that minimizes RMS radial error is the mean radius.
    let r = s.iter().map(|p| dist(*p, c)).sum::<f32>() / s.len() as f32;
    if !(r.is_finite() && r > 0.5) {
        return None;
    }
    let circle = ellipse_path(c, r, r, 0.0);
    let circle_res = rms_to_path(s, &circle, true) / diag;

    // Ellipse: principal axes from the second moments, then a 2x2 linear
    // least squares for A x² + B y² = 1 in that frame.
    let theta = principal_angle(s);
    let (sin, cos) = theta.sin_cos();
    let local: Vec<[f32; 2]> = s
        .iter()
        .map(|p| {
            let (dx, dy) = (p[0] - c[0], p[1] - c[1]);
            [dx * cos + dy * sin, -dx * sin + dy * cos]
        })
        .collect();
    let ell = solve_axes(&local).map(|(rx, ry)| {
        let path = ellipse_path(c, rx, ry, theta);
        let res = rms_to_path(s, &path, true) / diag;
        let ratio = rx.min(ry) / rx.max(ry);
        let kind = if ratio >= CIRCLE_RATIO {
            ShapeKind::Circle
        } else {
            ShapeKind::Ellipse
        };
        Recognized {
            kind,
            path,
            residual: res,
        }
    });

    match ell {
        // Prefer the circle unless the ellipse earns its extra freedom.
        Some(e) if e.residual < circle_res * POLY_MARGIN && e.kind == ShapeKind::Ellipse => Some(e),
        _ => Some(Recognized {
            kind: ShapeKind::Circle,
            path: circle,
            residual: circle_res,
        }),
    }
}

/// Corner hunt: simplify the loop, then build the IDEAL shape those corners
/// describe — a true (possibly rotated) rectangle for four, a regular n-gon
/// otherwise. The ideal is what gets scored, so a wonky quadrilateral scores
/// badly and is refused rather than silently squared up.
fn fit_poly(s: &[[f32; 2]], diag: f32) -> Option<Recognized> {
    let corners = simplify_closed(s, DP_EPS_FRAC * diag);
    let k = corners.len();
    if !(3..=MAX_POLY_SIDES).contains(&k) {
        return None;
    }
    if k == 4 {
        return fit_rect(s, &corners, diag);
    }
    let c = centroid(s);
    // Regular n-gon: mean radius, and the rotation that best explains where
    // the corners actually landed (circular mean of angle_i - i·τ/k).
    let r = corners.iter().map(|p| dist(*p, c)).sum::<f32>() / k as f32;
    if !(r.is_finite() && r > 0.5) {
        return None;
    }
    // Refuse an irregular polygon outright — this row promises REGULAR ones,
    // and squaring up a deliberate irregular shape is a data-loss bug.
    let sides: Vec<f32> = (0..k).map(|i| dist(corners[i], corners[(i + 1) % k])).collect();
    let mean = sides.iter().sum::<f32>() / k as f32;
    if mean <= 0.0 {
        return None;
    }
    let cv = (sides.iter().map(|d| (d - mean).powi(2)).sum::<f32>() / k as f32).sqrt() / mean;
    if cv > POLY_SIDE_CV {
        return None;
    }
    let step = TAU / k as f32;
    let (mut sx, mut sy) = (0.0f32, 0.0f32);
    for (i, p) in corners.iter().enumerate() {
        let a = (p[1] - c[1]).atan2(p[0] - c[0]) - i as f32 * step;
        sx += a.cos();
        sy += a.sin();
    }
    let rot = sy.atan2(sx);
    let path: Vec<[f32; 2]> = (0..k)
        .map(|i| {
            let a = rot + i as f32 * step;
            [c[0] + r * a.cos(), c[1] + r * a.sin()]
        })
        .collect();
    let residual = rms_to_path(s, &path, true) / diag;
    Some(Recognized {
        kind: ShapeKind::Polygon(k),
        path,
        residual,
    })
}

/// A true rectangle through four corners, rotation included. The angle is
/// chosen by minimum enclosing area over the corner-edge directions — the
/// rotating-calipers rule, restricted to the four candidates that matter.
fn fit_rect(s: &[[f32; 2]], corners: &[[f32; 2]], diag: f32) -> Option<Recognized> {
    let mut best: Option<(f32, [f32; 4], f32)> = None; // (area, local bbox, theta)
    for i in 0..corners.len() {
        let a = corners[i];
        let b = corners[(i + 1) % corners.len()];
        let theta = (b[1] - a[1]).atan2(b[0] - a[0]);
        let (sin, cos) = theta.sin_cos();
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in s {
            let (u, v) = (p[0] * cos + p[1] * sin, -p[0] * sin + p[1] * cos);
            x0 = x0.min(u);
            y0 = y0.min(v);
            x1 = x1.max(u);
            y1 = y1.max(v);
        }
        let area = (x1 - x0) * (y1 - y0);
        if area.is_finite() && best.as_ref().is_none_or(|(a0, _, _)| area < *a0) {
            best = Some((area, [x0, y0, x1, y1], theta));
        }
    }
    let (_, r, theta) = best?;
    let (sin, cos) = theta.sin_cos();
    let back = |u: f32, v: f32| [u * cos - v * sin, u * sin + v * cos];
    let path = vec![
        back(r[0], r[1]),
        back(r[2], r[1]),
        back(r[2], r[3]),
        back(r[0], r[3]),
    ];
    let residual = rms_to_path(s, &path, true) / diag;
    Some(Recognized {
        kind: ShapeKind::Rect,
        path,
        residual,
    })
}

// --- open shapes ---------------------------------------------------------

fn fit_open(s: &[[f32; 2]], diag: f32) -> Option<Recognized> {
    // Straight line: the ends, scored against every sample between them.
    let line = vec![s[0], s[s.len() - 1]];
    let line_res = rms_to_path(s, &line, false) / diag;
    if line_res <= FIT_TOL {
        return Some(Recognized {
            kind: ShapeKind::Line,
            path: line,
            residual: line_res,
        });
    }
    // A smooth open curve — but only if it really is smooth. A corner this
    // sharp means the hand drew a corner and cleaning it into a sweep would
    // be inventing something the artist did not draw.
    let cusp = (1..s.len() - 1)
        .map(|i| turn_at(s[i - 1], s[i], s[i + 1]))
        .fold(0.0f32, f32::max);
    if cusp > MAX_CURVE_CUSP {
        return None;
    }
    // The clean version: a Catmull-Rom-style sweep through nine evenly
    // spaced anchors off the resampled path. Nine is enough for an S and
    // far too few to reproduce a wobble, which is the point.
    const ANCHORS: usize = 9;
    let anchors: Vec<[f32; 2]> = resample(s, ANCHORS);
    let path = spline_through(&anchors);
    let residual = rms_to_path(s, &path, false) / diag;
    Some(Recognized {
        kind: ShapeKind::Curve,
        path,
        residual,
    })
}

/// Open Catmull-Rom through the anchors, tessellated. Kept local rather than
/// borrowed from `balloon::tessellate_open` so this module stays pure
/// geometry with no dependency on the balloon subsystem.
fn spline_through(a: &[[f32; 2]]) -> Vec<[f32; 2]> {
    if a.len() < 3 {
        return a.to_vec();
    }
    let n = a.len();
    let at = |i: isize| -> [f32; 2] { a[i.clamp(0, n as isize - 1) as usize] };
    let mut out = Vec::with_capacity((n - 1) * 8 + 1);
    for i in 0..n - 1 {
        let (p0, p1, p2, p3) = (
            at(i as isize - 1),
            at(i as isize),
            at(i as isize + 1),
            at(i as isize + 2),
        );
        for k in 0..8 {
            let t = k as f32 / 8.0;
            let (t2, t3) = (t * t, t * t * t);
            let mut p = [0.0f32; 2];
            for d in 0..2 {
                p[d] = 0.5
                    * ((2.0 * p1[d])
                        + (-p0[d] + p2[d]) * t
                        + (2.0 * p0[d] - 5.0 * p1[d] + 4.0 * p2[d] - p3[d]) * t2
                        + (-p0[d] + 3.0 * p1[d] - 3.0 * p2[d] + p3[d]) * t3);
            }
            out.push(p);
        }
    }
    out.push(a[n - 1]);
    out
}

// --- geometry helpers ----------------------------------------------------

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    (b[0] - a[0]).hypot(b[1] - a[1])
}

fn dedupe(pts: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let mut out: Vec<[f32; 2]> = Vec::with_capacity(pts.len());
    for p in pts {
        if !p[0].is_finite() || !p[1].is_finite() {
            continue;
        }
        if out.last().is_none_or(|q| dist(*q, *p) > 1e-3) {
            out.push(*p);
        }
    }
    out
}

fn arc_len(pts: &[[f32; 2]]) -> f32 {
    pts.windows(2).map(|w| dist(w[0], w[1])).sum()
}

/// `[x0, y0, x1, y1]`.
pub fn bbox(pts: &[[f32; 2]]) -> [f32; 4] {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in pts {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    [x0, y0, x1, y1]
}

fn bbox_diag(pts: &[[f32; 2]]) -> f32 {
    let b = bbox(pts);
    (b[2] - b[0]).hypot(b[3] - b[1])
}

fn centroid(pts: &[[f32; 2]]) -> [f32; 2] {
    let n = pts.len().max(1) as f32;
    [
        pts.iter().map(|p| p[0]).sum::<f32>() / n,
        pts.iter().map(|p| p[1]).sum::<f32>() / n,
    ]
}

/// `n` points spaced evenly along the arc, first and last preserved. Even
/// spacing is what makes every measure downstream scale-free.
fn resample(pts: &[[f32; 2]], n: usize) -> Vec<[f32; 2]> {
    let total = arc_len(pts);
    if n < 2 || pts.len() < 2 || total <= 0.0 {
        return pts.to_vec();
    }
    let step = total / (n - 1) as f32;
    let mut out = Vec::with_capacity(n);
    out.push(pts[0]);
    let (mut i, mut carried) = (1usize, 0.0f32);
    let mut cur = pts[0];
    while out.len() < n - 1 && i < pts.len() {
        let seg = dist(cur, pts[i]);
        if carried + seg >= step {
            let t = (step - carried) / seg.max(1e-6);
            let p = [
                cur[0] + (pts[i][0] - cur[0]) * t,
                cur[1] + (pts[i][1] - cur[1]) * t,
            ];
            out.push(p);
            cur = p;
            carried = 0.0;
        } else {
            carried += seg;
            cur = pts[i];
            i += 1;
        }
    }
    while out.len() < n {
        out.push(pts[pts.len() - 1]);
    }
    out
}

/// SIGNED turn at `b`, radians in `-PI..=PI`. Left is positive.
fn signed_turn_at(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    let (u, v) = ([b[0] - a[0], b[1] - a[1]], [c[0] - b[0], c[1] - b[1]]);
    if u[0].hypot(u[1]) < 1e-6 || v[0].hypot(v[1]) < 1e-6 {
        return 0.0;
    }
    (u[0] * v[1] - u[1] * v[0]).atan2(u[0] * v[0] + u[1] * v[1])
}

/// Absolute turn at `b`, radians in `0..=PI`. Only the CUSP test uses this:
/// one sharp corner is a real signal even though the SUM of these is not.
fn turn_at(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    signed_turn_at(a, b, c).abs().min(PI)
}

/// Net turning along the path — the number of loops the hand made, times τ.
/// Tremor contributes a zero-mean term that cancels, which is exactly why
/// this and not the absolute sum is the gate (module header).
fn signed_turning(s: &[[f32; 2]], closed: bool) -> f32 {
    let n = s.len();
    if n < 3 {
        return 0.0;
    }
    let mut t: f32 = (1..n - 1)
        .map(|i| signed_turn_at(s[i - 1], s[i], s[i + 1]))
        .sum();
    if closed {
        t += signed_turn_at(s[n - 2], s[n - 1], s[0]) + signed_turn_at(s[n - 1], s[0], s[1]);
    }
    t
}

/// Twice the signed area the path encloses, halved — the shoelace formula,
/// closing the loop implicitly.
fn shoelace(s: &[[f32; 2]]) -> f32 {
    let n = s.len();
    let mut a = 0.0;
    for i in 0..n {
        let (p, q) = (s[i], s[(i + 1) % n]);
        a += p[0] * q[1] - q[0] * p[1];
    }
    a * 0.5
}

/// Orientation of the long axis, from the second moments.
fn principal_angle(s: &[[f32; 2]]) -> f32 {
    let c = centroid(s);
    let (mut xx, mut yy, mut xy) = (0.0f32, 0.0f32, 0.0f32);
    for p in s {
        let (dx, dy) = (p[0] - c[0], p[1] - c[1]);
        xx += dx * dx;
        yy += dy * dy;
        xy += dx * dy;
    }
    0.5 * (2.0 * xy).atan2(xx - yy)
}

/// Least squares for `A u² + B v² = 1` over centred, axis-aligned points;
/// returns the semi-axes. `None` when the normal equations are degenerate
/// (a straight run of points, everything at the centre).
fn solve_axes(local: &[[f32; 2]]) -> Option<(f32, f32)> {
    let (mut a11, mut a12, mut a22, mut b1, mut b2) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for p in local {
        let (u2, v2) = ((p[0] * p[0]) as f64, (p[1] * p[1]) as f64);
        a11 += u2 * u2;
        a12 += u2 * v2;
        a22 += v2 * v2;
        b1 += u2;
        b2 += v2;
    }
    let det = a11 * a22 - a12 * a12;
    if det.abs() < 1e-9 {
        return None;
    }
    let a = (b1 * a22 - b2 * a12) / det;
    let b = (a11 * b2 - a12 * b1) / det;
    if !(a > 1e-9 && b > 1e-9) {
        return None;
    }
    let (rx, ry) = ((1.0 / a).sqrt() as f32, (1.0 / b).sqrt() as f32);
    (rx.is_finite() && ry.is_finite() && rx > 0.5 && ry > 0.5).then_some((rx, ry))
}

fn ellipse_path(c: [f32; 2], rx: f32, ry: f32, theta: f32) -> Vec<[f32; 2]> {
    let (sin, cos) = theta.sin_cos();
    const N: usize = 96;
    (0..N)
        .map(|k| {
            let t = k as f32 / N as f32 * TAU;
            let (u, v) = (rx * t.cos(), ry * t.sin());
            [c[0] + u * cos - v * sin, c[1] + u * sin + v * cos]
        })
        .collect()
}

fn dist_to_seg(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (vx, vy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = vx * vx + vy * vy;
    if l2 < 1e-9 {
        return dist(p, a);
    }
    let t = (((p[0] - a[0]) * vx + (p[1] - a[1]) * vy) / l2).clamp(0.0, 1.0);
    dist(p, [a[0] + vx * t, a[1] + vy * t])
}

fn dist_to_path(p: [f32; 2], path: &[[f32; 2]], closed: bool) -> f32 {
    let n = path.len();
    if n == 1 {
        return dist(p, path[0]);
    }
    let segs = if closed { n } else { n - 1 };
    (0..segs)
        .map(|i| dist_to_seg(p, path[i], path[(i + 1) % n]))
        .fold(f32::MAX, f32::min)
}

/// The ONE score every candidate is judged by: RMS distance from the
/// resampled stroke to the candidate outline. Callers divide by the bbox
/// diagonal so the number is scale-free and comparable across shapes.
fn rms_to_path(s: &[[f32; 2]], path: &[[f32; 2]], closed: bool) -> f32 {
    if path.is_empty() {
        return f32::MAX;
    }
    let sum: f32 = s.iter().map(|p| dist_to_path(*p, path, closed).powi(2)).sum();
    (sum / s.len() as f32).sqrt()
}

/// Douglas–Peucker over a CLOSED loop. Anchored at the point farthest from
/// the centroid, which on any polygon is a corner — starting mid-edge would
/// pin a false vertex there and turn a triangle into a quadrilateral.
fn simplify_closed(s: &[[f32; 2]], eps: f32) -> Vec<[f32; 2]> {
    let n = s.len();
    if n < 4 {
        return s.to_vec();
    }
    let c = centroid(s);
    let start = (0..n)
        .max_by(|a, b| {
            dist(s[*a], c)
                .partial_cmp(&dist(s[*b], c))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(0);
    let mut loop_pts: Vec<[f32; 2]> = (0..=n).map(|i| s[(start + i) % n]).collect();
    let mut keep = vec![false; loop_pts.len()];
    keep[0] = true;
    keep[loop_pts.len() - 1] = true;
    dp(&loop_pts, 0, loop_pts.len() - 1, eps, &mut keep);
    loop_pts.pop(); // the repeated anchor
    keep.pop();
    let mut out: Vec<[f32; 2]> = loop_pts
        .iter()
        .zip(&keep)
        .filter_map(|(p, k)| k.then_some(*p))
        .collect();
    // Wobble can leave two "corners" a few px apart on one real corner.
    // Merge them, or a square comes back as a hexagon and gets refused.
    let min_gap = eps * 1.2;
    let mut i = 0;
    while out.len() > 3 && i < out.len() {
        let j = (i + 1) % out.len();
        if dist(out[i], out[j]) < min_gap {
            out[i] = [
                (out[i][0] + out[j][0]) * 0.5,
                (out[i][1] + out[j][1]) * 0.5,
            ];
            out.remove(j);
        } else {
            i += 1;
        }
    }
    out
}

fn dp(pts: &[[f32; 2]], lo: usize, hi: usize, eps: f32, keep: &mut [bool]) {
    if hi <= lo + 1 {
        return;
    }
    let (mut far, mut best) = (lo, 0.0f32);
    for (i, p) in pts.iter().enumerate().take(hi).skip(lo + 1) {
        let d = dist_to_seg(*p, pts[lo], pts[hi]);
        if d > best {
            best = d;
            far = i;
        }
    }
    if best > eps {
        keep[far] = true;
        dp(pts, lo, far, eps, keep);
        dp(pts, far, hi, eps, keep);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic "hand wobble": a repeating, index-derived offset in
    /// `-1..=1`. No RNG anywhere in this crate's tests — a replayed failure
    /// has to be the same failure, and a shape recognizer that only passes
    /// on some seeds is not passing.
    fn jitter(i: usize, salt: usize) -> f32 {
        let h = (i.wrapping_mul(2654435761).wrapping_add(salt.wrapping_mul(40503))) >> 8;
        ((h % 2000) as f32 / 1000.0) - 1.0
    }

    /// A closed loop traced round an outline with `amp` px of wobble.
    fn wobble(path: &[[f32; 2]], n: usize, amp: f32, salt: usize) -> Vec<[f32; 2]> {
        let dense = super::resample(
            &path
                .iter()
                .copied()
                .chain(std::iter::once(path[0]))
                .collect::<Vec<_>>(),
            n,
        );
        dense
            .iter()
            .enumerate()
            .map(|(i, p)| {
                [
                    p[0] + jitter(i, salt) * amp,
                    p[1] + jitter(i, salt + 7) * amp,
                ]
            })
            .collect()
    }

    fn circle_pts(cx: f32, cy: f32, r: f32, n: usize, amp: f32) -> Vec<[f32; 2]> {
        (0..=n)
            .map(|i| {
                let t = i as f32 / n as f32 * TAU;
                let rr = r + jitter(i, 3) * amp;
                [cx + rr * t.cos(), cy + rr * t.sin()]
            })
            .collect()
    }

    #[test]
    fn a_wobbly_circle_is_the_circle_you_meant() {
        let got = recognize(&circle_pts(200.0, 200.0, 90.0, 120, 4.0))
            .expect("a hand-drawn circle is recognized");
        assert_eq!(got.kind, ShapeKind::Circle, "residual {}", got.residual);
        let c = centroid(&got.path);
        assert!(
            (c[0] - 200.0).abs() < 4.0 && (c[1] - 200.0).abs() < 4.0,
            "centred on what was drawn, got {c:?}"
        );
        let r = got.path.iter().map(|p| dist(*p, c)).sum::<f32>() / got.path.len() as f32;
        assert!((r - 90.0).abs() < 5.0, "radius {r}");
        assert!(got.closed(), "a circle closes");
    }

    /// The other half of the promise: an ellipse must NOT be rounded up into
    /// a circle. Squaring an oval off is the same class of bug as eating a
    /// scribble — it throws away what the hand actually said.
    #[test]
    fn a_flattened_loop_stays_an_ellipse() {
        let pts: Vec<[f32; 2]> = (0..=140)
            .map(|i| {
                let t = i as f32 / 140.0 * TAU;
                [
                    240.0 + (150.0 + jitter(i, 5) * 3.0) * t.cos(),
                    200.0 + (60.0 + jitter(i, 9) * 3.0) * t.sin(),
                ]
            })
            .collect();
        let got = recognize(&pts).expect("an oval is recognized");
        assert_eq!(got.kind, ShapeKind::Ellipse, "residual {}", got.residual);
        let b = got.bbox();
        assert!((b[2] - b[0] - 300.0).abs() < 20.0, "width from {b:?}");
        assert!((b[3] - b[1] - 120.0).abs() < 20.0, "height from {b:?}");
    }

    #[test]
    fn a_wobbly_rectangle_is_a_true_rectangle() {
        let r = [
            [80.0, 60.0],
            [320.0, 60.0],
            [320.0, 240.0],
            [80.0, 240.0],
        ];
        let got = recognize(&wobble(&r, 160, 3.5, 11)).expect("a hand-drawn box is recognized");
        assert_eq!(got.kind, ShapeKind::Rect, "residual {}", got.residual);
        assert_eq!(got.path.len(), 4, "four corners, no more");
        let b = got.bbox();
        assert!((b[0] - 80.0).abs() < 8.0 && (b[1] - 60.0).abs() < 8.0, "{b:?}");
        assert!((b[2] - 320.0).abs() < 8.0 && (b[3] - 240.0).abs() < 8.0, "{b:?}");
    }

    /// `FG-021`'s "true rectangle" has to survive being drawn at an angle —
    /// the axis-aligned assumption is the classic recognizer bug.
    #[test]
    fn a_rotated_rectangle_keeps_its_angle() {
        let th = 0.5f32;
        let (sin, cos) = th.sin_cos();
        let c = [220.0f32, 210.0];
        let spin = |u: f32, v: f32| [c[0] + u * cos - v * sin, c[1] + u * sin + v * cos];
        let r = [
            spin(-120.0, -70.0),
            spin(120.0, -70.0),
            spin(120.0, 70.0),
            spin(-120.0, 70.0),
        ];
        let got = recognize(&wobble(&r, 170, 3.0, 23)).expect("a tilted box is recognized");
        assert_eq!(got.kind, ShapeKind::Rect, "residual {}", got.residual);
        // Every drawn corner has a recognized corner near it.
        for want in r {
            assert!(
                got.path.iter().any(|p| dist(*p, want) < 16.0),
                "corner {want:?} missing from {:?}",
                got.path
            );
        }
        // And it is NOT the axis-aligned bounding box (which would be much
        // bigger than the true rectangle at this angle).
        let b = got.bbox();
        assert!(
            (b[2] - b[0]) < 290.0,
            "an axis-aligned fit would be wider: {b:?}"
        );
    }

    #[test]
    fn a_wobbly_triangle_is_a_triangle() {
        let t = [[200.0, 60.0], [340.0, 300.0], [60.0, 300.0]];
        let got = recognize(&wobble(&t, 150, 3.0, 31)).expect("a hand-drawn triangle");
        assert_eq!(got.kind, ShapeKind::Polygon(3), "residual {}", got.residual);
        assert_eq!(got.path.len(), 3);
    }

    #[test]
    fn a_straightish_drag_is_a_line() {
        let pts: Vec<[f32; 2]> = (0..=90)
            .map(|i| {
                let t = i as f32 / 90.0;
                [60.0 + t * 280.0, 200.0 + jitter(i, 13) * 2.5]
            })
            .collect();
        let got = recognize(&pts).expect("a straightish stroke is a line");
        assert_eq!(got.kind, ShapeKind::Line, "residual {}", got.residual);
        assert_eq!(got.path.len(), 2, "two ends");
        assert!(!got.closed(), "a line does not close");
        assert!(dist(got.path[0], [60.0, 200.0]) < 8.0, "{:?}", got.path);
        assert!(dist(got.path[1], [340.0, 200.0]) < 8.0, "{:?}", got.path);
    }

    #[test]
    fn a_smooth_sweep_is_an_open_curve() {
        let pts: Vec<[f32; 2]> = (0..=100)
            .map(|i| {
                let t = i as f32 / 100.0;
                [
                    60.0 + t * 300.0,
                    220.0 - (t * PI).sin() * 110.0 + jitter(i, 17) * 2.0,
                ]
            })
            .collect();
        let got = recognize(&pts).expect("a smooth arc is recognized");
        assert_eq!(got.kind, ShapeKind::Curve, "residual {}", got.residual);
        assert!(!got.closed());
        // It still goes where the hand went — and the peak survived.
        assert!(dist(got.path[0], [60.0, 220.0]) < 10.0);
        let top = got
            .path
            .iter()
            .fold(f32::MAX, |m, p| m.min(p[1]));
        assert!((top - 110.0).abs() < 16.0, "peak at y={top}");
    }

    // --- the refusals: the half that matters more ------------------------

    #[test]
    fn a_genuine_scribble_is_left_alone() {
        // Eight overlapping loops — the thing you scrub out a mistake with.
        let pts: Vec<[f32; 2]> = (0..=400)
            .map(|i| {
                let t = i as f32 / 400.0;
                let a = t * TAU * 8.0;
                [
                    120.0 + t * 200.0 + a.cos() * 40.0,
                    200.0 + a.sin() * 40.0,
                ]
            })
            .collect();
        assert!(
            recognize(&pts).is_none(),
            "a scribble must never become a shape"
        );
    }

    #[test]
    fn a_zigzag_is_left_alone() {
        let pts: Vec<[f32; 2]> = (0..=120)
            .map(|i| {
                let t = i as f32 / 120.0;
                [
                    60.0 + t * 280.0,
                    200.0 + if (i / 6) % 2 == 0 { 45.0 } else { -45.0 },
                ]
            })
            .collect();
        assert!(recognize(&pts).is_none(), "a zigzag is not a curve");
    }

    #[test]
    fn a_small_hatch_mark_is_left_alone() {
        // The killer case: cross-hatching is hundreds of short strokes, and
        // ANY of them turning into a figure would be unusable.
        let pts: Vec<[f32; 2]> = (0..=10)
            .map(|i| {
                let t = i as f32 / 10.0;
                [100.0 + t * 18.0, 100.0 + t * 14.0]
            })
            .collect();
        assert!(
            recognize(&pts).is_none(),
            "below the size floor nothing is recognized"
        );
        // And it is the FLOOR doing it, not the fit: the same mark scaled up
        // is a perfectly good line.
        let big: Vec<[f32; 2]> = pts
            .iter()
            .map(|p| [(p[0] - 100.0) * 12.0 + 100.0, (p[1] - 100.0) * 12.0 + 100.0])
            .collect();
        assert_eq!(
            recognize(&big).map(|r| r.kind),
            Some(ShapeKind::Line),
            "the same shape above the floor is a line"
        );
    }

    #[test]
    fn a_lumpy_blob_is_left_alone() {
        // Closed, one loop, low turning — passes every gate and still must
        // not be squared up: the fits simply do not explain it.
        let pts: Vec<[f32; 2]> = (0..=160)
            .map(|i| {
                let t = i as f32 / 160.0 * TAU;
                let r = 100.0 + (t * 5.0).sin() * 34.0 + (t * 3.0).cos() * 22.0;
                [220.0 + r * t.cos(), 220.0 + r * t.sin()]
            })
            .collect();
        assert!(
            recognize(&pts).is_none(),
            "a lumpy closed shape stays as drawn"
        );
    }

    #[test]
    fn an_irregular_quadrilateral_is_not_squared_up() {
        // A deliberate wonky four-sider. Four corners, so the rect fitter
        // runs — and the ideal rectangle it proposes must score too badly
        // to accept, or the tool would silently redraw the artist's shape.
        let q = [
            [80.0, 70.0],
            [330.0, 110.0],
            [270.0, 300.0],
            [110.0, 190.0],
        ];
        assert!(
            recognize(&wobble(&q, 150, 2.0, 41)).is_none(),
            "a wonky quad is not a rectangle"
        );
    }

    #[test]
    fn a_there_and_back_scrub_is_not_a_closed_loop() {
        // Ends where it started, but the perimeter never gets away from the
        // diagonal — CLOSED_LEN_RATIO is the gate, and the open fits must
        // not rescue it either.
        let mut pts: Vec<[f32; 2]> = (0..=40)
            .map(|i| [80.0 + i as f32 * 6.0, 200.0 + jitter(i, 19) * 3.0])
            .collect();
        let back: Vec<[f32; 2]> = pts.iter().rev().copied().collect();
        pts.extend(back);
        assert!(recognize(&pts).is_none(), "a scrub is not a shape");
    }

    #[test]
    fn degenerate_input_never_panics() {
        assert!(recognize(&[]).is_none());
        assert!(recognize(&[[1.0, 1.0]]).is_none());
        assert!(recognize(&[[1.0, 1.0], [1.0, 1.0], [1.0, 1.0]]).is_none());
        assert!(recognize(&[[f32::NAN, 0.0], [0.0, f32::NAN]]).is_none());
        // A long run of identical points dedupes to one.
        let same = vec![[50.0, 50.0]; 400];
        assert!(recognize(&same).is_none());
    }

    /// Recognition must not depend on where on the page the stroke was
    /// drawn or how big it is — the residual is normalized by the diagonal
    /// precisely so that it does not.
    #[test]
    fn recognition_is_translation_and_scale_invariant() {
        let base = circle_pts(0.0, 0.0, 60.0, 110, 2.5);
        for (dx, dy, k) in [(0.0, 0.0, 1.0), (900.0, 400.0, 1.0), (120.0, 60.0, 3.0)] {
            let moved: Vec<[f32; 2]> = base
                .iter()
                .map(|p| [p[0] * k + dx, p[1] * k + dy])
                .collect();
            assert_eq!(
                recognize(&moved).map(|r| r.kind),
                Some(ShapeKind::Circle),
                "at offset ({dx}, {dy}) scale {k}"
            );
        }
    }
}
