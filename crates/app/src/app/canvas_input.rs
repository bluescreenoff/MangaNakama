//! Canvas pointer input: `canvas_down/move/up` route every press to the
//! active tool's gesture, plus the gesture state types and finishers for
//! frame/balloon Object drags and the Transform handles. One place decides
//! what a press MEANS; the check order in `canvas_down`/`canvas_move` is
//! the gesture priority (pan → transform → alt-eyedropper → tools; then
//! the if-chain: rotating → panning → drawing → transform → text →
//! object/balloon drags → frame → select) — do not reorder it.

use super::{App, PointerKind, TransformGesture};
use crate::cmd::{
    AppCmd, BalloonMode, FigureAnchor, FigureMode, FigureStage2, FigureStage2Kind, FillMode,
    FrameMode, GradMode, ObjectMode, PanMode, RulerKind, SelectMode, Tool,
};
use mn_core::{
    Balloon, BalloonHandle, BalloonSet, BalloonShape, Frame, PenSample, Selection, Tail, selected,
};

/// What part of a frame an Object-tool drag grabbed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ObjectDragMode {
    MoveWhole,
    Vertex(usize),
    /// Edge i → i+1: both endpoints move together.
    Edge(usize),
    /// Rotation lollipop: spin the polygon around its centroid.
    Rotate,
    /// Bbox corner i (0=TL, 1=TR, 2=BR, 3=BL): uniform scale around the
    /// opposite corner (CSP's panel-scale handles).
    ScaleCorner(usize),
    /// Bbox edge i (0=top, 1=right, 2=bottom, 3=left): stretch one axis
    /// around the opposite edge.
    ScaleEdge(usize),
}

/// Bbox corner i's opposite corner index.
const fn opposite_corner(i: usize) -> usize {
    (i + 2) % 4
}

/// SF-004/005 (TRIAGE 140): which driver handle of a generated
/// effect-line layer a drag grabbed. The BLUE reference (position and
/// extent of the run) vs the shape drivers (radii / angle / lengths).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum GenDragMode {
    /// Focus: the convergence point (moves the whole run).
    Center,
    /// Focus: inner radius (where rays start).
    RIn,
    /// Focus: outer radius (where rays end).
    ROut,
    /// Speed: the direction line's heading.
    Angle,
    /// Speed: shortest line length.
    LenMin,
    /// Speed: longest line length.
    LenMax,
}

/// An in-progress driver drag on a generated layer. Release re-applies
/// the spec and regenerates in place (one rasterization, no per-move).
pub struct GenLinesDrag {
    pub layer: usize,
    pub mode: GenDragMode,
    pub start: (f32, f32),
    pub cur: (f32, f32),
    pub orig: mn_core::genlines::GenLinesSpec,
}

/// An in-progress ruler MOVE (Object tool). `ruler` is the combined index
/// [`mn_core::Rulers::grab_near`] returns (items first, curves offset by
/// `items.len()` — the [`mn_core::SnapLock`] convention), `last` the
/// previous canvas point: every move applies the STEP, so the grab offset
/// survives and the live ruler is already in its new place for the next
/// snap. The whole drag is ONE undo step: `before` is the set as it was at
/// the grab, and `canvas_up` records it once at release.
#[derive(Clone, Debug)]
pub struct RulerMove {
    pub ruler: usize,
    pub grab: mn_core::RulerGrab,
    pub last: [f32; 2],
    /// Did the pointer actually travel? A press that only grabs is not a
    /// move, and must not claim to be one in the status line.
    pub moved: bool,
    /// The ruler set at the moment of the grab — the undo step's pre-image.
    pub before: mn_core::Rulers,
}

/// A point clamped inside the page, with a margin so a handle drawn on
/// the very edge is still grabbable.
fn on_page(p: [f32; 2], size: (u32, u32)) -> [f32; 2] {
    let m = 6.0;
    [
        p[0].clamp(m, (size.0 as f32 - m).max(m)),
        p[1].clamp(m, (size.1 as f32 - m).max(m)),
    ]
}

/// Is `p` inside the page (with the same margin `on_page` clamps to)?
fn is_on_page(p: [f32; 2], size: (u32, u32)) -> bool {
    on_page(p, size) == p
}

/// A radial driver handle at radius `r`: on the ray the placing drag was
/// made along if that lands on the page, else swept around the ring to
/// the nearest angle that does.
///
/// A handle off the page cannot be clicked and is not drawn — which is
/// how a burst placed near an edge ended up with no grabbable radius at
/// all. Sweeping keeps the RADIUS exact (the drag math reads distance
/// from the centre, so a straight clamp would make the grab jump); only
/// the angle we show it at moves.
fn radial_handle(c: [f32; 2], r: f32, base_deg: f32, size: (u32, u32)) -> [f32; 2] {
    let at = |deg: f32| {
        let (s, co) = deg.to_radians().sin_cos();
        [c[0] + co * r, c[1] + s * r]
    };
    let p = at(base_deg);
    if is_on_page(p, size) {
        return p;
    }
    for k in 1..=12 {
        for sign in [1.0f32, -1.0] {
            let q = at(base_deg + sign * k as f32 * 15.0);
            if is_on_page(q, size) {
                return q;
            }
        }
    }
    on_page(p, size)
}

/// Where each driver handle sits, canvas px — the single source shared
/// by the hit-test and the overlay so they can never disagree.
pub fn gen_handle_points(
    spec: &mn_core::genlines::GenLinesSpec,
    size: (u32, u32),
) -> Vec<(GenDragMode, [f32; 2])> {
    if spec.focus {
        let c = [spec.a, spec.b];
        vec![
            (GenDragMode::Center, on_page(c, size)),
            (
                GenDragMode::RIn,
                radial_handle(c, spec.c, spec.hand_deg, size),
            ),
            (
                GenDragMode::ROut,
                radial_handle(c, spec.d, spec.hand_deg, size),
            ),
        ]
    } else {
        // Speed lines are canvas-wide parallels; the reference line runs
        // along the angle through the run's ANCHOR — the midpoint of the
        // drag that placed it, so the handles are where the gesture was.
        // (`None` = the canvas centre, where every pre-2026-08-23 run's
        // handles sat.)
        let a = gen_anchor(spec, size);
        let (s, co) = spec.a.to_radians().sin_cos();
        let dir = [co, s];
        let at = |l: f32| on_page([a[0] + dir[0] * l, a[1] + dir[1] * l], size);
        vec![
            (GenDragMode::Angle, at(spec.c * 0.5)),
            (GenDragMode::LenMin, at(spec.b)),
            (GenDragMode::LenMax, at(spec.c)),
        ]
    }
}

/// Does `layer` carry ink within `tol` px of (cx, cy)?
///
/// A generated run is HAIRLINES with paper between them, and the hit test
/// used to read the ONE pixel under the cursor: to select a set you had
/// to land on a line, at zero tolerance, which at any real zoom is not a
/// thing a hand can do — the owner's "I cannot re-select them to edit
/// properties". Every other object here has a tolerance; this one now
/// does too, over the same `tol` disc the handles use.
pub(crate) fn layer_ink_near(layer: &mn_core::Layer, cx: f32, cy: f32, tol: f32) -> bool {
    let r = tol.clamp(1.0, 64.0);
    let (x0, x1) = ((cx - r).floor() as i32, (cx + r).ceil() as i32);
    let (y0, y1) = ((cy - r).floor() as i32, (cy + r).ceil() as i32);
    for y in y0..=y1 {
        for x in x0..=x1 {
            if x < 0 || y < 0 {
                continue;
            }
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            if dx * dx + dy * dy > r * r {
                continue;
            }
            let idx = mn_core::TileIdx::of_pixel(x, y);
            let hit = layer.tile(idx).is_some_and(|t| {
                let (ox, oy) = idx.origin();
                t.pixel((x - ox) as usize, (y - oy) as usize)[3] > 0
            });
            if hit {
                return true;
            }
        }
    }
    false
}

/// [`layer_ink_near`] over the tiles the compositor DISPLAYS — for a
/// live fill layer the derived tone raster lives in `fill_tiles`, not
/// the layer's own pixel map, so a plain `tile()` read sees nothing.
pub(crate) fn display_ink_near(layer: &mn_core::Layer, cx: f32, cy: f32, tol: f32) -> bool {
    let r = tol.clamp(1.0, 64.0);
    let (x0, x1) = ((cx - r).floor() as i32, (cx + r).ceil() as i32);
    let (y0, y1) = ((cy - r).floor() as i32, (cy + r).ceil() as i32);
    let tiles = layer.display_tiles();
    for y in y0..=y1 {
        for x in x0..=x1 {
            if x < 0 || y < 0 {
                continue;
            }
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            if dx * dx + dy * dy > r * r {
                continue;
            }
            let idx = mn_core::TileIdx::of_pixel(x, y);
            let hit = tiles.get(&idx).is_some_and(|t| {
                let (ox, oy) = idx.origin();
                t.pixel((x - ox) as usize, (y - oy) as usize)[3] > 0
            });
            if hit {
                return true;
            }
        }
    }
    false
}

/// A speed run's reference anchor: its own, or the canvas centre.
pub fn gen_anchor(spec: &mn_core::genlines::GenLinesSpec, size: (u32, u32)) -> [f32; 2] {
    spec.anchor
        .unwrap_or([size.0 as f32 * 0.5, size.1 as f32 * 0.5])
}

/// How far the ray from `a` along `dir` runs before it leaves the page —
/// the ceiling the LenMax handle clamps to, so the handle stays on the
/// paper the run is drawn on.
fn page_reach(a: [f32; 2], dir: [f32; 2], size: (u32, u32)) -> f32 {
    let mut best = f32::INFINITY;
    for (p, d, hi) in [(a[0], dir[0], size.0 as f32), (a[1], dir[1], size.1 as f32)] {
        if d.abs() < 1e-6 {
            continue;
        }
        let t = if d > 0.0 { (hi - p) / d } else { -p / d };
        if t > 0.0 {
            best = best.min(t);
        }
    }
    if best.is_finite() { best } else { 0.0 }
}

/// Even-odd containment for a frame polygon (any winding; the frame SDF
/// signs concave frames by the same rule).
fn poly_contains(pts: &[[f32; 2]], p: [f32; 2]) -> bool {
    let n = pts.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let t = (p[1] - a[1]) / (b[1] - a[1]);
            if a[0] + t * (b[0] - a[0]) > p[0] {
                inside = !inside;
            }
        }
    }
    inside
}

/// The panel a placement gesture starts in: the SMALLEST frame folder
/// whose polygon contains the point (nesting = innermost wins), with
/// that polygon's AABB. `None` in the gutter or on a bare page.
pub(crate) fn panel_at(doc: &mn_core::Document, p: [f32; 2]) -> Option<(usize, [f32; 4])> {
    let mut best: Option<(f32, usize, [f32; 4])> = None; // (|2·area|, folder, aabb)
    for (i, l) in doc.layers.iter().enumerate() {
        if !(l.folder && l.is_frame()) {
            continue;
        }
        let Some(fs) = l.frames() else { continue };
        for f in &fs.frames {
            if !poly_contains(&f.points, p) {
                continue;
            }
            let mut twice_area = 0.0;
            let mut aabb = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
            for k in 0..f.points.len() {
                let (q, r) = (f.points[k], f.points[(k + 1) % f.points.len()]);
                twice_area += q[0] * r[1] - r[0] * q[1];
                aabb[0] = aabb[0].min(q[0]);
                aabb[1] = aabb[1].min(q[1]);
                aabb[2] = aabb[2].max(q[0]);
                aabb[3] = aabb[3].max(q[1]);
            }
            if best
                .as_ref()
                .map_or(true, |(ba, _, _)| twice_area.abs() < *ba)
            {
                best = Some((twice_area.abs(), i, aabb));
            }
        }
    }
    best.map(|(_, i, aabb)| (i, aabb))
}

/// The spec as a drag would leave it (shared by the live overlay and
/// the release commit).
pub fn gen_drag_spec(d: &GenLinesDrag, size: (u32, u32)) -> mn_core::genlines::GenLinesSpec {
    let mut s = d.orig;
    let (dx, dy) = (d.cur.0 - d.start.0, d.cur.1 - d.start.1);
    // Length/radius ceilings come from the CANVAS, not literals — a
    // 600 dpi B4 is wider than 6000 px — and every clamp's bounds are
    // built non-invertible: f32::clamp PANICS on min > max, the dialog
    // sets inner/outer and min/max as independent values with nothing
    // enforcing order, and a panic here unwinds through wndproc = an
    // abort (audit B, 2026-08-19).
    let diag = ((size.0 as f32).powi(2) + (size.1 as f32).powi(2)).sqrt();
    let ceil = 2.0 * diag;
    match d.mode {
        GenDragMode::Center => {
            s.a += dx;
            s.b += dy;
        }
        GenDragMode::RIn => {
            let r = ((d.cur.0 - s.a).powi(2) + (d.cur.1 - s.b).powi(2)).sqrt();
            s.c = r.clamp(4.0, (s.d - 4.0).max(4.0));
        }
        GenDragMode::ROut => {
            let r = ((d.cur.0 - s.a).powi(2) + (d.cur.1 - s.b).powi(2)).sqrt();
            let lo = s.c + 4.0;
            if lo < ceil {
                s.d = r.clamp(lo, ceil);
            }
        }
        GenDragMode::Angle => {
            let a = gen_anchor(&s, size);
            s.a = (d.cur.1 - a[1]).atan2(d.cur.0 - a[0]).to_degrees();
        }
        GenDragMode::LenMin => {
            let a = gen_anchor(&s, size);
            let (sin, cos) = s.a.to_radians().sin_cos();
            let l = (d.cur.0 - a[0]) * cos + (d.cur.1 - a[1]) * sin;
            s.b = l.clamp(8.0, (s.c - 8.0).max(8.0));
        }
        GenDragMode::LenMax => {
            let a = gen_anchor(&s, size);
            let (sin, cos) = s.a.to_radians().sin_cos();
            let l = (d.cur.0 - a[0]) * cos + (d.cur.1 - a[1]) * sin;
            let lo = s.b + 8.0;
            // The ceiling is the PAGE, not twice the diagonal: past the
            // paper's edge the handle is invisible and ungrabbable, so
            // the run could be lengthened and never shortened again. The
            // reference line runs BOTH ways from the anchor, so the cap
            // is whichever way reaches further — an anchor near the right
            // edge must not cap a leftward block at 20 px.
            let reach = page_reach(a, [cos, sin], size)
                .max(page_reach(a, [-cos, -sin], size))
                .min(ceil);
            if lo < reach {
                s.c = l.clamp(lo, reach);
            }
        }
    }
    s
}

/// An in-progress Object-tool drag. The document keeps the original frame
/// until release; the overlay draws `preview()` live.
pub struct ObjectDrag {
    pub layer: usize,
    pub frame: usize,
    pub mode: ObjectDragMode,
    pub start: (f32, f32),
    pub cur: (f32, f32),
    pub orig: Frame,
    /// Live Shift state, refreshed on every pointer move: rotate snaps
    /// to 45° increments while held (CSP).
    pub shift_snap: bool,
}

/// Rows 76/78's group half: a drag armed by pressing an object that is
/// already a member of a multi-selection of more than one — the whole
/// set translates together on release. Translation only, by design:
/// handles, lattice and edge drags edit ONE object, so they stay on the
/// single-object paths (press a solo object, or one outside the set).
pub struct GroupObjDrag {
    pub start: (f32, f32),
    pub cur: (f32, f32),
}

impl GroupObjDrag {
    /// The whole-pixel delta the release will commit.
    pub fn delta(&self) -> (i32, i32) {
        (
            (self.cur.0 - self.start.0).round() as i32,
            (self.cur.1 - self.start.1).round() as i32,
        )
    }
}

impl ObjectDrag {
    /// The dragged frame as it would land right now.
    pub fn preview(&self) -> Frame {
        let (dx, dy) = (self.cur.0 - self.start.0, self.cur.1 - self.start.1);
        let mut f = self.orig.clone();
        match self.mode {
            ObjectDragMode::MoveWhole => f.translate(dx, dy),
            ObjectDragMode::Vertex(i) => {
                f.points[i][0] += dx;
                f.points[i][1] += dy;
            }
            ObjectDragMode::Edge(i) => {
                let n = f.points.len();
                let j = (i + 1) % n;
                f.points[i][0] += dx;
                f.points[i][1] += dy;
                f.points[j][0] += dx;
                f.points[j][1] += dy;
            }
            ObjectDragMode::Rotate => {
                let c = self.orig.centroid();
                let a0 = (self.start.1 - c[1]).atan2(self.start.0 - c[0]);
                let a1 = (self.cur.1 - c[1]).atan2(self.cur.0 - c[0]);
                // Shift = 45° INCREMENTS from the original orientation
                // (CSP; same rule as the figure tools' Shift constraint —
                // quantize the delta, not the absolute, so a frame drawn
                // at 10° still rotates in 45° steps of its own).
                let d = if self.shift_snap {
                    ((a1 - a0) / std::f32::consts::FRAC_PI_4).round() * std::f32::consts::FRAC_PI_4
                } else {
                    a1 - a0
                };
                f.rotate_around(c, d);
            }
            ObjectDragMode::ScaleCorner(i) => {
                let b = self.orig.bbox();
                let corners = [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]];
                let a = corners[opposite_corner(i)];
                let l0 = (self.start.0 - a[0]).hypot(self.start.1 - a[1]);
                let l1 = (self.cur.0 - a[0]).hypot(self.cur.1 - a[1]);
                if l0 > 1e-3 {
                    f.scale_around(a, l1 / l0, l1 / l0);
                }
            }
            ObjectDragMode::ScaleEdge(i) => {
                let b = self.orig.bbox();
                // Top/bottom edges scale Y around the far edge; left/right
                // scale X — anchor is the opposite side of the ORIGINAL box.
                let (anchor, span0, span1, vert) = match i {
                    0 => (b[3], self.start.1, self.cur.1, true),
                    2 => (b[1], self.start.1, self.cur.1, true),
                    1 => (b[0], self.start.0, self.cur.0, false),
                    _ => (b[2], self.start.0, self.cur.0, false),
                };
                let d0 = span0 - anchor;
                let d1 = span1 - anchor;
                if d0.abs() > 1e-3 {
                    let s = d1 / d0;
                    if vert {
                        f.scale_around([b[0], anchor], 1.0, s);
                    } else {
                        f.scale_around([anchor, b[1]], s, 1.0);
                    }
                }
            }
        }
        f
    }

    fn moved(&self) -> bool {
        (self.cur.0 - self.start.0).abs() + (self.cur.1 - self.start.1).abs() > 0.5
    }
}

/// An in-progress drag of a live TONE layer's lattice (CSP "Move tone
/// pattern"): the dots shift under the art while the window stays put.
/// The document keeps the original until release — one SetFillParams.
pub struct FillLatticeDrag {
    pub layer: usize,
    pub start: (f32, f32),
    pub cur: (f32, f32),
}

/// An in-progress Liquify drag (row 55): the whole gesture is one op
/// bracket, so one undo. `last_step` drives the hold-accumulation
/// frame tick for Expand/Pinch/Twirl.
pub struct LiquifyDrag {
    pub last: (f32, f32),
    pub last_step: std::time::Instant,
}

/// What part of a balloon an Object-tool drag grabbed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BalloonDragMode {
    MoveWhole,
    Handle(BalloonHandle),
    /// The Operation tool's blue box (CSP applies it to every object):
    /// bbox corner i uniform scale about the opposite corner.
    BoxCorner(usize),
    /// Bbox edge-mid i axis stretch.
    BoxEdge(usize),
    /// Rotation lollipop above the box.
    BoxRotate,
}

pub struct BalloonObjDrag {
    pub layer: usize,
    pub balloon: usize,
    pub mode: BalloonDragMode,
    pub start: (f32, f32),
    pub cur: (f32, f32),
    pub orig: Balloon,
    /// Live Shift state, refreshed on every pointer move: moves constrain
    /// to H/V/45° and rotation snaps to 45° increments (CSP).
    pub shift_snap: bool,
}

impl BalloonObjDrag {
    /// The dragged balloon as it would land right now.
    pub fn preview(&self) -> Balloon {
        let (dx, dy) = (self.cur.0 - self.start.0, self.cur.1 - self.start.1);
        let mut b = self.orig.clone();
        match self.mode {
            BalloonDragMode::MoveWhole => {
                // Shift constrains the move to horizontal/vertical/45°
                // diagonals (CSP manual, moving balloons) — the nearest
                // of the 8 octants at the drag's own length.
                let (mx, my) = if self.shift_snap {
                    let len = dx.hypot(dy);
                    let oct = (dy.atan2(dx) / std::f32::consts::FRAC_PI_4).round()
                        * std::f32::consts::FRAC_PI_4;
                    (oct.cos() * len, oct.sin() * len)
                } else {
                    (dx, dy)
                };
                b.translate(mx, my)
            }
            BalloonDragMode::Handle(h) => b.apply_handle(h, [self.cur.0, self.cur.1]),
            BalloonDragMode::BoxCorner(i) => {
                let bb = self.orig.bbox();
                let corners = [
                    [bb[0], bb[1]],
                    [bb[2], bb[1]],
                    [bb[2], bb[3]],
                    [bb[0], bb[3]],
                ];
                let a = corners[opposite_corner(i)];
                let l0 = (self.start.0 - a[0]).hypot(self.start.1 - a[1]);
                let l1 = (self.cur.0 - a[0]).hypot(self.cur.1 - a[1]);
                if l0 > 1e-3 {
                    b.transform_around(a, l1 / l0, l1 / l0, 0.0);
                }
            }
            BalloonDragMode::BoxEdge(i) => {
                let bb = self.orig.bbox();
                let (anchor, span0, span1, vert) = match i {
                    0 => (bb[3], self.start.1, self.cur.1, true),
                    2 => (bb[1], self.start.1, self.cur.1, true),
                    1 => (bb[0], self.start.0, self.cur.0, false),
                    _ => (bb[2], self.start.0, self.cur.0, false),
                };
                let d0 = span0 - anchor;
                let d1 = span1 - anchor;
                if d0.abs() > 1e-3 {
                    let s = d1 / d0;
                    if vert {
                        b.transform_around([bb[0], anchor], 1.0, s, 0.0);
                    } else {
                        b.transform_around([anchor, bb[1]], s, 1.0, 0.0);
                    }
                }
            }
            BalloonDragMode::BoxRotate => {
                let bb = self.orig.bbox();
                let c = [(bb[0] + bb[2]) * 0.5, (bb[1] + bb[3]) * 0.5];
                let a0 = (self.start.1 - c[1]).atan2(self.start.0 - c[0]);
                let a1 = (self.cur.1 - c[1]).atan2(self.cur.0 - c[0]);
                // Shift = 45° INCREMENTS from the original orientation
                // (CSP manual, rotating balloons; same rule as frames).
                let d = if self.shift_snap {
                    ((a1 - a0) / std::f32::consts::FRAC_PI_4).round() * std::f32::consts::FRAC_PI_4
                } else {
                    a1 - a0
                };
                b.transform_around(c, 1.0, 1.0, d);
            }
        }
        b
    }

    fn moved(&self) -> bool {
        (self.cur.0 - self.start.0).abs() + (self.cur.1 - self.start.1).abs() > 0.5
    }

    /// The rigid turn this drag applies as `(pivot, radians)`, or `None`.
    ///
    /// Only the lollipop is rigid: a handle drag reshapes the bubble and a
    /// box scale changes its proportions, and neither is a motion you could
    /// carry lettering by. TRIAGE 134 uses this to turn the text INSIDE the
    /// balloon with it — see [`mn_core::balloon::rotate_texts_in`].
    ///
    /// An ellipse or rounded-rect body is refused: those are axis-aligned and
    /// a rotate drag does not tilt them at all, so carrying the lettering
    /// would spin the text inside a bubble that never moved.
    pub fn rotation(&self) -> Option<([f32; 2], f32)> {
        if !matches!(self.mode, BalloonDragMode::BoxRotate) || !self.orig.shape.rotates_exactly() {
            return None;
        }
        let bb = self.orig.bbox();
        let c = [(bb[0] + bb[2]) * 0.5, (bb[1] + bb[3]) * 0.5];
        let a0 = (self.start.1 - c[1]).atan2(self.start.0 - c[0]);
        let a1 = (self.cur.1 - c[1]).atan2(self.cur.0 - c[0]);
        Some((c, a1 - a0))
    }
}

/// An in-progress Object-tool drag on a balloon; same lifecycle as
/// [`ObjectDrag`] — the document keeps the original until release.
// (`point_in_quad` moved to `app::transform`, next to the hit test that is
// its only caller.)

impl App {
    // --- canvas input: one place where a tool decides what a press means ---

    /// How deep into a click run this press is: 1 plain, 2 double, 3 triple,
    /// and on up. The window class has no `CS_DBLCLKS`, so `WM_LBUTTONDBLCLK`
    /// never arrives and the run has to be timed here — same 4 px / 400 ms
    /// test the curve ruler has always used.
    fn click_run(&mut self, x: f32, y: f32) -> u8 {
        let n = match self.last_click {
            Some((lx, ly, t, n))
                if (lx - x).abs() < 4.0
                    && (ly - y).abs() < 4.0
                    && t.elapsed().as_millis() < 400 =>
            {
                n.saturating_add(1)
            }
            _ => 1,
        };
        self.last_click = Some((x, y, std::time::Instant::now(), n));
        n
    }

    pub fn canvas_down(&mut self, x: f32, y: f32, kind: PointerKind, batch: &[PenSample]) {
        // `IO-060`: the document is being rebuilt at another resolution one
        // page at a time. A stroke started now would be resampled into the
        // result and would have been drawn against pixels that are about to
        // stop existing. The pointer half of the `cmd::dispatch` refusal.
        if self.resample_job.is_some() {
            return;
        }
        // The user is touching the canvas: the deferred startup fit stands
        // down and never adjusts their view again (see App::render).
        self.startup_fit_pending = false;
        let clicks = self.click_run(x, y);
        // Leak repair (app/fill_repair.rs): the armed gesture owns the
        // next stroke. VIRTUAL mode captures it as points — nothing lands
        // anywhere; REAL-INK mode lets the normal stroke run below and
        // the release refills.
        if self
            .fill_repair
            .as_ref()
            .is_some_and(|r| r.virtual_barrier)
        {
            let radius = self.brush_radius();
            let (cx, cy) = self.viewport.to_canvas(x, y);
            if let Some(r) = &mut self.fill_repair {
                r.pts = vec![(cx, cy)];
                r.radius = radius;
            }
            self.needs_redraw = true;
            return;
        }
        // TODO #3: an armed ruler creation owns the next drag — CSP's
        // Layer ▸ Ruler ▸ …-then-draw flow. No tool switch, no painting.
        if self.ruler_pending.is_some() {
            let (cx, cy) = self.viewport.to_canvas(x, y);
            // Curve (part 2): click vertices; a double-click closes.
            if self.ruler_pending == Some(RulerKind::Curve) {
                if clicks >= 2 {
                    self.finish_curve_ruler();
                    return;
                }
                self.curve_pending
                    .get_or_insert_with(Vec::new)
                    .push([cx, cy]);
                self.set_status("curve vertex added — double-click (or Enter) to finish");
                self.needs_redraw = true;
                return;
            }
            self.ruler_drag = Some([cx, cy]);
            return;
        }
        if self.space_down || self.tool == Tool::Pan {
            // CSP's canvas modifiers, the three that live on Space
            // ("Shortcuts usable during operation"): Space+drag pans,
            // Shift+Space+drag ROTATES, Ctrl+Space+click zooms in and
            // Alt+Space+click zooms out — both about the point clicked,
            // like the wheel. Only the first of the three was here, so
            // rotating the page mid-stroke meant letting go of the pen
            // to press R.
            let m = self.shell.sync_modifiers();
            if self.space_down && (m.ctrl || m.alt) {
                let step = self.prefs.wheel_step.max(1.02);
                self.zoom_at(x, y, if m.ctrl { step } else { 1.0 / step });
                return;
            }
            let rotate = if self.space_down {
                m.shift
            } else {
                self.pan_mode == PanMode::Rotate
            };
            if rotate {
                self.begin_rotate(x, y);
            } else {
                self.begin_pan(x, y);
            }
            return;
        }
        // An active Transform owns the canvas: presses move/scale/rotate the
        // float instead of reaching any tool.
        if self.transform_drag.is_some() {
            self.transform_down(x, y);
            self.needs_redraw = true;
            return;
        }
        // KB-020/KB-022 (TRIAGE 172, owner HIGH): Ctrl+Alt+drag resizes
        // the brush live; Ctrl+drag is a temporary Object grab. Checked
        // BEFORE the Alt-eyedropper arm so ctrl+alt never falls into it.
        if matches!(
            self.tool,
            Tool::Pen | Tool::Eraser | Tool::SelPen | Tool::SelEraser
        ) && self.shell.sync_modifiers().ctrl
        {
            let m = self.shell.sync_modifiers();
            if m.alt {
                self.size_drag_begin(x);
                return;
            }
            if self.temp_object_try(x, y) {
                return;
            }
            // Nothing under the pen: fall through and draw.
        }
        // CSP modifier default: Alt turns any drawing tool into the eyedropper.
        if matches!(
            self.tool,
            Tool::Pen | Tool::Eraser | Tool::SelPen | Tool::SelEraser | Tool::Fill
        ) && self.shell.sync_modifiers().alt
        {
            let (cx, cy) = self.viewport.to_canvas(x, y);
            self.push_cmd(AppCmd::PickColor(cx, cy));
            self.needs_redraw = true;
            return;
        }
        match self.tool {
            Tool::Pen | Tool::Eraser | Tool::SelPen | Tool::SelEraser => {
                // The frame-layer guard is about INK landing on frame
                // layers — selection strokes paint the doc's scratch, so
                // it does not apply to them.
                if !matches!(self.tool, Tool::SelPen | Tool::SelEraser) && self.guard_frame_layer()
                {
                    return;
                }
                self.begin_stroke(kind);
                self.push_batch(batch);
                self.doc.mask_stroke_to_selection();
            }
            Tool::Fill => {
                if self.guard_frame_layer() {
                    return;
                }
                let (cx, cy) = self.viewport.to_canvas(x, y);
                match self.fill_mode {
                    // FI-003 / FI-004 / FI-005 are drags, not clicks:
                    // collect the freehand path and let the release decide
                    // what it meant (see canvas_up).
                    FillMode::Click => self.push_cmd(AppCmd::Fill(cx, cy)),
                    _ => self.fill_drag = Some(vec![(cx, cy)]),
                }
            }
            Tool::Tone => {
                // One click, one live tone layer — no drag state to keep.
                let (cx, cy) = self.viewport.to_canvas(x, y);
                self.push_cmd(AppCmd::ToneRegion(cx, cy));
            }
            Tool::Wand => {
                let (cx, cy) = self.viewport.to_canvas(x, y);
                // The combine op is decided AT THE CLICK (held modifiers
                // override the persistent 4-way mode).
                let m = self.shell.sync_modifiers();
                let op = crate::cmd::effective_sel_op(m.shift, m.alt, self.sel_op);
                self.push_cmd(AppCmd::MagicSelect(cx, cy, op));
            }
            Tool::Eyedrop => {
                let (cx, cy) = self.viewport.to_canvas(x, y);
                self.push_cmd(AppCmd::PickColor(cx, cy));
            }
            Tool::Select if self.select_mode == SelectMode::Magnetic => {
                let (cx, cy) = self.viewport.to_canvas(x, y);
                self.magnetic_down(cx, cy);
            }
            Tool::Select => {
                let (cx, cy) = self.viewport.to_canvas(x, y);
                // A press that is COMBINING (Shift/Alt held, or the Tool
                // Property's own Add/Subtract/Intersect mode) always draws
                // a new shape, even from inside the existing ants: adding a
                // second chunk to a selection is the everyday gesture, and
                // SE-039's move-the-ants grab used to swallow it whole —
                // Shift+drag inside a selection MOVED it instead of adding.
                let m = self.shell.sync_modifiers();
                let combining = crate::cmd::effective_sel_op(m.shift, m.alt, self.sel_op)
                    != mn_core::SelectionOp::Replace;
                let inside = !combining
                    && self
                        .doc
                        .selection
                        .as_ref()
                        .is_some_and(|s| selected(s.coverage(cx as i32, cy as i32)))
                    && !self.doc.active_layer().is_vector();
                if inside {
                    // Dragging from inside the selection moves the ANTS
                    // (SE-039); contents move via the launcher Transform.
                    self.select_moving = Some(((cx, cy), (cx, cy)));
                } else {
                    self.select_drag = Some(vec![(cx, cy)]);
                }
            }
            Tool::Figure => {
                let (cx, cy) = self.viewport.to_canvas(x, y);
                // Row 157: a figure already in its second stage is waiting
                // for exactly this — the click that says "there". It fires
                // before any sub-tool arm because at this moment the press
                // means "commit", not "start another shape".
                if let Some(s) = &mut self.figure_stage2 {
                    // Take the aim from the PRESS, not from the last move:
                    // a tap that arrives without a move before it (touch, or
                    // any synthetic caller) must still mean where it landed.
                    s.cur = (cx, cy);
                    self.finish_figure_stage2();
                    self.needs_redraw = true;
                    return;
                }
                // `FG-013`/`FG-014`/`FG-016`: Ctrl and Alt turn this press
                // from "place another point" into "edit the list I already
                // have". Sampled ONCE here, at the caller, and handed down as
                // plain booleans — the `smart_shape_adjust` rule. A helper
                // that read the keyboard itself would make the whole editing
                // grammar untestable and would tie the suite to whatever the
                // developer's hands were doing while it ran.
                //
                // The Figure tool is safe ground for both: the Ctrl
                // temporary-Object grab and the Alt eyedropper above are
                // gated to Pen/Eraser/Sel*/Fill, so neither modifier already
                // means something here.
                let m = self.shell.sync_modifiers();
                let (ctrl, alt) = (m.ctrl, m.alt);
                match self.figure_mode {
                    FigureMode::Polygon => {
                        // Click = place a vertex; a click back on the first
                        // vertex (or Enter) closes the shape.
                        let close_tol = (10.0 / self.viewport.zoom.max(0.01)).max(3.0);
                        match &mut self.figure_poly {
                            Some(pts) => {
                                let first = pts[0].p;
                                // Closing is a PLAIN tap only. With a
                                // modifier down the artist is reaching for
                                // the anchor itself (move it, or ask for a
                                // corner that Polygon does not have), and
                                // committing the shape out from under that
                                // reach is unrecoverable in one keystroke.
                                let close = pts.len() >= 3
                                    && !ctrl
                                    && !alt
                                    && (first.0 - cx).abs() + (first.1 - cy).abs() < close_tol;
                                if close {
                                    self.finish_figure_poly();
                                } else if !self.figure_poly_edit(cx, cy, ctrl, alt) {
                                    self.figure_poly
                                        .get_or_insert_with(Vec::new)
                                        .push(FigureAnchor::new(cx, cy));
                                }
                            }
                            None => {
                                if self.guard_frame_layer() {
                                    return;
                                }
                                self.figure_poly = Some(vec![FigureAnchor::new(cx, cy)]);
                                self.set_status(
                                    "click vertices; tap one to delete it, tap an edge to insert, Ctrl+drag moves; Enter closes",
                                );
                            }
                        }
                    }
                    FigureMode::Curve => {
                        // Rows 84/85, the same two-stage gesture as Polygon
                        // and for the same reason: the mark is longer than
                        // one press. Click points; a click back on the LAST
                        // one (the natural "that's it" motion, and what a
                        // double-click reads as — the second click lands on
                        // the point the first placed) ends the curve, as do
                        // Enter and Esc. Nothing closes the loop: an open
                        // sweep is the whole difference from Polygon.
                        let tol = (10.0 / self.viewport.zoom.max(0.01)).max(3.0);
                        match &mut self.figure_poly {
                            Some(pts) => {
                                let last = pts.last().expect("non-empty").p;
                                // Plain taps only, same reasoning as the
                                // Polygon close — and one more that is
                                // specific to `FG-016`: Alt+tapping the point
                                // you JUST placed to make it a corner is the
                                // actual CSP workflow, so Alt must never be
                                // read as "that's it, ink it".
                                let done = pts.len() >= 2
                                    && !ctrl
                                    && !alt
                                    && (last.0 - cx).abs() + (last.1 - cy).abs() < tol;
                                if done {
                                    self.finish_figure_poly();
                                } else if !self.figure_poly_edit(cx, cy, ctrl, alt) {
                                    self.figure_poly
                                        .get_or_insert_with(Vec::new)
                                        .push(FigureAnchor::new(cx, cy));
                                }
                            }
                            None => {
                                if self.guard_frame_layer() {
                                    return;
                                }
                                self.figure_poly = Some(vec![FigureAnchor::new(cx, cy)]);
                                self.set_status(
                                    "click along the curve; Alt+tap a point for a corner, Ctrl+drag moves; Enter inks it",
                                );
                            }
                        }
                    }
                    // Row 156 / `FG-020`: the one Figure gesture that is a
                    // freehand STROKE. It begins exactly like a pen stroke
                    // — same guard, same `begin_stroke`, same live ink —
                    // and `smart_shape` rides alongside recording the path
                    // so the hold has something to recognize. Releasing
                    // without holding therefore costs nothing and changes
                    // nothing: the stroke is simply already drawn.
                    FigureMode::Smart => {
                        if self.guard_frame_layer() {
                            return;
                        }
                        let ops_at_start = self.doc.op_count();
                        self.begin_stroke(kind);
                        // `begin_stroke` REFUSES on a file object, a maskless
                        // live layer and mid-correction — arming the gesture
                        // regardless would leave state behind that no release
                        // ever clears (canvas_up only finishes while drawing).
                        if self.drawing() {
                            self.push_batch(batch);
                            self.doc.mask_stroke_to_selection();
                            // `FG-020`'s switch, and it gates only the
                            // GESTURE: off, this sub tool is a plain
                            // freehand pen — the stroke above still inks,
                            // there is simply no hold, no recognition and
                            // no swap behind it.
                            if self.prefs.smart_shape {
                                let refused =
                                    self.shell.sync_modifiers().shift || self.rulers_deciding();
                                self.smart_shape = Some(crate::app::SmartShape {
                                    pts: vec![[cx, cy]],
                                    anchor: (x, y),
                                    still_since: std::time::Instant::now(),
                                    ripe: false,
                                    hit: None,
                                    refused,
                                    ops_at_start,
                                    adjust: None,
                                });
                            }
                        }
                    }
                    m if m.generates() => {
                        // No frame-layer guard: these never ink the active
                        // layer — the release generates a fresh effect-line
                        // layer of their own.
                        self.figure_drag = Some(((cx, cy), (cx, cy)));
                    }
                    _ => {
                        if self.guard_frame_layer() {
                            return;
                        }
                        self.figure_drag = Some(((cx, cy), (cx, cy)));
                    }
                }
                self.needs_redraw = true;
            }
            Tool::Gradient => {
                // Re-guards on EVERY press, which is what covers the
                // freeform gesture's second stroke: the Layers palette is
                // live between the two lines, so the layer under the first
                // need not be the layer under the second.
                if self.guard_frame_layer() {
                    return;
                }
                let (cx, cy) = self.viewport.to_canvas(x, y);
                if self.grad_mode.is_freeform() {
                    let g = self.grad_free.get_or_insert_with(Default::default);
                    g.cur = vec![[cx, cy]];
                    g.drawing = true;
                } else {
                    self.grad_drag = Some(((cx, cy), (cx, cy)));
                }
                self.needs_redraw = true;
            }
            Tool::Liquify => {
                if self.guard_frame_layer() {
                    return;
                }
                let (cx, cy) = self.viewport.to_canvas(x, y);
                self.doc.begin_op();
                self.liquify_drag = Some(LiquifyDrag {
                    last: (cx, cy),
                    last_step: std::time::Instant::now(),
                });
                self.set_status(format!(
                    "{} — drag to warp; hold to keep working (Alt inverts)",
                    self.liquify_mode.label()
                ));
                self.needs_redraw = true;
            }
            Tool::Frame => {
                let (cx, cy) = self.viewport.to_canvas(x, y);
                match self.frame_mode {
                    FrameMode::Polyline => {
                        // Click = place a vertex; a click back on the first
                        // vertex (or Enter) closes the panel.
                        let close_tol = (10.0 / self.viewport.zoom.max(0.01)).max(3.0);
                        match &mut self.frame_poly {
                            Some(pts) => {
                                let first = pts[0];
                                let close = pts.len() >= 3
                                    && (first.0 - cx).abs() + (first.1 - cy).abs() < close_tol;
                                if close {
                                    self.finish_frame_poly();
                                } else {
                                    pts.push((cx, cy));
                                }
                            }
                            None => {
                                self.frame_poly = Some(vec![(cx, cy)]);
                                self.set_status(
                                    "click corners; click the first one (or Enter) to close, Esc cancels",
                                );
                            }
                        }
                    }
                    FrameMode::Pen => {
                        self.frame_pen = Some(vec![(cx, cy)]);
                    }
                    _ => self.frame_drag = Some(((cx, cy), (cx, cy))),
                }
            }
            Tool::Balloon => {
                let (cx, cy) = self.viewport.to_canvas(x, y);
                let pr = batch.last().map(|s| s.pressure).unwrap_or(0.5);
                self.balloon_drag = Some(vec![[cx, cy, pr], [cx, cy, pr]]);
            }
            Tool::Object if self.object_mode == ObjectMode::PickLayer => {
                // S-001: no drag, no hit-test against objects — the click
                // just answers "which layer drew this pixel".
                let (cx, cy) = self.viewport.to_canvas(x, y);
                self.pick_layer_at(cx, cy);
            }
            Tool::Object => {
                let (cx, cy) = self.viewport.to_canvas(x, y);
                // Modifier clicks EDIT the selected balloon's anchor list
                // before any drag machinery can grab the press.
                let m = self.shell.sync_modifiers();
                if self.balloon_anchor_edit(cx, cy, m.ctrl, m.alt) {
                    self.needs_redraw = true;
                    return;
                }
                // Rulers are drawn OVER the art and are grabbed first —
                // but only within the handle tolerance, so a press that is
                // not on a ruler still reaches the panel/balloon under it.
                if self.ruler_grab(cx, cy) {
                    self.needs_redraw = true;
                    return;
                }
                // Vector strokes are LAYER-SCOPED: only when the active
                // layer records do its strokes take the press (you selected
                // the layer to edit it — the mask-editing scoping). Alt
                // turns the drag into the WIDTH edit (phase 4).
                if self.doc.active_layer().strokes.is_some() && self.vector_hit(cx, cy, m.alt) {
                    self.needs_redraw = true;
                    return;
                }
                // Row 78: the four-way click combine runs BEFORE the
                // kind-specific grabs — Remove/Toggle on an already-
                // selected object never re-selects (and never arms a drag).
                let combine = if m.shift {
                    crate::cmd::SelectCombine::Add
                } else {
                    self.object_combine
                };
                if combine != crate::cmd::SelectCombine::New {
                    let cands = self.object_candidates_at(cx, cy);
                    if let Some(top) = cands.first().copied() {
                        let primary = self.object_selection();
                        let in_multi = self.object_multi.iter().any(|r| *r == top);
                        let is_primary = primary == Some(top);
                        match combine {
                            crate::cmd::SelectCombine::Remove => {
                                if in_multi {
                                    self.object_multi.retain(|r| *r != top);
                                    self.set_status("removed from the selection");
                                    if is_primary {
                                        self.clear_object_selection();
                                    }
                                    self.needs_redraw = true;
                                    return;
                                }
                                if is_primary {
                                    self.clear_object_selection();
                                    self.set_status("deselected");
                                    self.needs_redraw = true;
                                    return;
                                }
                                // Remove on an unselected object: nothing.
                                self.needs_redraw = true;
                                return;
                            }
                            crate::cmd::SelectCombine::Toggle => {
                                if in_multi || is_primary {
                                    self.object_multi.retain(|r| *r != top);
                                    if is_primary {
                                        self.clear_object_selection();
                                    }
                                    self.set_status("toggled off");
                                    self.needs_redraw = true;
                                    return;
                                }
                                self.object_multi.push(top);
                                // Falls through: the normal path makes it
                                // the primary too, so its handles appear.
                            }
                            crate::cmd::SelectCombine::Add => {
                                if !in_multi && !is_primary {
                                    self.object_multi.push(top);
                                }
                                // Falls through to select/drag as usual.
                            }
                            crate::cmd::SelectCombine::New => unreachable!(),
                        }
                    }
                } else {
                    // New: the fresh selection replaces the set.
                    self.object_multi.clear();
                }
                // Group-move: pressing a member of a multi-selection of
                // more than one arms a WHOLE-SET translate. After the
                // combine block so Remove/Toggle keep their early
                // returns; before the kind grabs so the set takes the
                // press. The pressed member still becomes the PRIMARY
                // (the Add/Toggle fall-through used to do that further
                // down — its handles follow the click), and a set of one
                // (or a press outside the set) falls through to the
                // ordinary single-object behaviour.
                if let Some(top) = self.object_candidates_at(cx, cy).first().copied()
                    && self.object_multi.len() > 1
                    && self.object_multi.iter().any(|r| *r == top)
                {
                    self.object_select_ref(top);
                    self.group_drag = Some(GroupObjDrag {
                        start: (cx, cy),
                        cur: (cx, cy),
                    });
                    self.needs_redraw = true;
                    return;
                }
                if !self.text_object_press(cx, cy, clicks) {
                    self.object_hit(cx, cy);
                }
            }
            Tool::Text => {
                let (cx, cy) = self.viewport.to_canvas(x, y);
                let shift = self.shell.sync_modifiers().shift;
                self.text_tool_down(cx, cy, shift, clicks);
            }
            Tool::Pan => unreachable!("handled above"),
        }
        self.needs_redraw = true;
    }

    /// L-001 press: start a trace, place an anchor, or close the loop by
    /// clicking back on the first anchor (the ringed one in the overlay).
    fn magnetic_down(&mut self, cx: f32, cy: f32) {
        let at = (cx.round() as i32, cy.round() as i32);
        // Same close tolerance the polyline frame and polygon figure use, so
        // "click the first point to close" means one thing across the app.
        let close_tol = (10.0 / self.viewport.zoom.max(0.01)).max(3.0);
        let closing = self
            .magnetic
            .as_ref()
            .is_some_and(|l| l.anchors().len() >= 3 && l.near_start(at, close_tol));
        if closing {
            self.magnetic_close();
            return;
        }
        if let Some(l) = self.magnetic.as_mut() {
            l.anchor(&self.doc, at);
        } else {
            let reach = self.magnetic_reach;
            self.magnetic = Some(mn_core::magnetic::Lasso::start(&self.doc, at, reach));
            self.set_status(
                "magnetic lasso: trace along the line — Backspace undoes an anchor, Enter closes",
            );
        }
    }

    /// L-001 drag: extend the wire to the cursor, freezing an anchor once the
    /// cursor has drifted far enough. The auto-anchor is not cosmetic — it is
    /// what keeps each shortest-path search inside a small window, and what
    /// stops the path behind you re-routing as you move on.
    fn magnetic_track(&mut self, cx: f32, cy: f32) {
        let at = (cx.round() as i32, cy.round() as i32);
        if let Some(l) = self.magnetic.as_mut() {
            if l.drift(at) >= mn_core::magnetic::AUTO_ANCHOR_PX {
                l.anchor(&self.doc, at);
            } else {
                l.track(&self.doc, at);
            }
        }
        self.needs_redraw = true;
    }

    /// Painting on a vector layer (frames, balloons) would be overwritten by
    /// the next re-raster; tell the user instead of silently eating ink.
    /// Returns true when blocked.
    fn guard_frame_layer(&mut self) -> bool {
        let l = self.doc.active_layer();
        if l.lock {
            self.set_status("layer is locked — unlock it in the Layers palette to edit");
            true
        } else if l.is_frame() {
            self.set_status(if l.folder {
                "this is the frame folder itself — pick a layer inside it to draw"
            } else {
                "frame layers hold panel borders — pick a raster layer to draw"
            });
            true
        } else if l.folder {
            self.set_status("folders organise layers — pick a layer inside to draw");
            true
        } else if l.is_balloon() {
            self.set_status("balloon layers hold speech bubbles — pick a raster layer to draw");
            true
        } else if l.is_text() {
            self.set_status("text layers hold text boxes — T edits, pick a raster layer to draw");
            true
        } else {
            false
        }
    }

    /// Snap a divide drag to horizontal/vertical when within ~8° of the axis.
    fn snap_axis(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let t = 8f32.to_radians().tan();
        if dy.abs() <= dx.abs() * t {
            (b.0, a.1)
        } else if dx.abs() <= dy.abs() * t {
            (a.0, b.1)
        } else {
            b
        }
    }

    /// Balloon-tool release: turn the drag into a balloon (or a tail) and
    /// push the command. Each sample is `[x, y, pressure]`.
    fn finish_balloon_drag(&mut self, pts: Vec<[f32; 3]>) {
        let Some(a) = pts.first().copied() else {
            return;
        };
        match self.balloon_mode {
            BalloonMode::Ellipse | BalloonMode::Round => {
                let Some(&b) = pts.last() else { return };
                let (w, h) = ((b[0] - a[0]).abs(), (b[1] - a[1]).abs());
                if w < mn_core::balloon::MIN_BALLOON_EXTENT
                    || h < mn_core::balloon::MIN_BALLOON_EXTENT
                {
                    self.set_status("drag out the balloon's size");
                    return;
                }
                let shape = if self.balloon_mode == BalloonMode::Ellipse {
                    BalloonShape::Ellipse {
                        center: [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5],
                        radii: [w * 0.5, h * 0.5],
                    }
                } else {
                    BalloonShape::RoundRect {
                        rect: [
                            a[0].min(b[0]),
                            a[1].min(b[1]),
                            a[0].max(b[0]),
                            a[1].max(b[1]),
                        ],
                        // CSP-ish soft corner: a quarter of the short side.
                        corner: w.min(h) * 0.25,
                    }
                };
                let mut balloon = Balloon {
                    shape,
                    tails: Vec::new(),
                    ..Default::default()
                };
                balloon.set_ink(self.balloon_ink);
                self.push_cmd(AppCmd::BalloonAdd { balloon });
            }
            BalloonMode::Draw => {
                // Simplify at ~2 screen px so zoomed-in drawing stays faithful;
                // each kept anchor carries its pen pressure. The raster runs a
                // smooth closed spline through the anchors (CSP's drawn
                // bubbles — not a hard polygon), inking the outline thinner
                // where the pen was light when the layer's pressure toggle is
                // on (Tool Property).
                let eps = (2.0 / self.viewport.zoom.max(0.01)).max(1.0);
                let raw: Vec<[f32; 2]> = pts.iter().map(|p| [p[0], p[1]]).collect();
                let prs: Vec<f32> = pts.iter().map(|p| p[2]).collect();
                let (mut simple, mut widths) = mn_core::balloon::simplify_anchors(&raw, &prs, eps);
                // Drawn shape closes itself: drop a last point that landed on
                // top of the first.
                if simple.len() >= 2 {
                    let (f, l) = (simple[0], simple[simple.len() - 1]);
                    if (f[0] - l[0]).abs() + (f[1] - l[1]).abs() < eps * 2.0 {
                        simple.pop();
                        widths.truncate(simple.len());
                    }
                }
                let mut balloon = Balloon {
                    shape: BalloonShape::Polygon {
                        points: simple,
                        widths,
                        corners: Vec::new(),
                    },
                    tails: Vec::new(),

                    ..Default::default()
                };
                balloon.set_ink(self.balloon_ink);
                if !balloon.is_valid() {
                    self.set_status("draw a closed bubble shape");
                    return;
                }
                self.push_cmd(AppCmd::BalloonAdd { balloon });
            }
            BalloonMode::Tail => {
                // Press inside a balloon, release at the tip.
                let Some(b) = pts.last().copied() else { return };
                let mut target = None;
                'outer: for li in (0..self.doc.layers.len()).rev() {
                    let layer = &self.doc.layers[li];
                    if !layer.visible {
                        continue;
                    }
                    if let Some(bs) = layer.balloons() {
                        if let Some(bi) = bs.balloon_at([a[0], a[1]]) {
                            target = Some((li, bi));
                            break 'outer;
                        }
                    }
                }
                let Some((layer, balloon)) = target else {
                    self.set_status("start the tail drag inside a balloon");
                    return;
                };
                if (b[0] - a[0]).abs() + (b[1] - a[1]).abs() < 8.0 {
                    self.set_status("drag out to where the tail should point");
                    return;
                }
                let tail = Tail {
                    base: [a[0], a[1]],
                    tip: [b[0], b[1]],
                    width: self.mm_to_px(self.balloon_tail_mm).max(6.0),
                    // `B-005`/`B-006`: the Tail section's current shape and
                    // bend, so the tail lands looking the way the panel says.
                    kind: self.balloon_tail_kind,
                    bend: self.balloon_tail_bend,
                };
                self.push_cmd(AppCmd::BalloonTailAdd {
                    layer,
                    balloon,
                    tail,
                });
            }
        }
    }

    /// TRIAGE 134 — the lettering turns with the bubble, and stays lettering.
    ///
    /// `orig` is the balloon as it stood BEFORE the drag: a text is carried
    /// when its centre was inside that shape. Every VISIBLE text layer is
    /// walked, because a page's balloons and its lettering are different
    /// layers and there is no stored link between them (the Object tool's
    /// stack cycling works the same way — geometry, not bookkeeping).
    ///
    /// Each carried item gets its angle re-shaped through DirectWrite, which
    /// is the whole point of the row: no glyph is ever flattened into pixels,
    /// so the text is still a text box afterwards — click it and keep typing.
    /// A hidden text layer is left alone rather than silently edited.
    pub(crate) fn carry_texts_with_balloon(
        &mut self,
        orig: &Balloon,
        pivot: [f32; 2],
        rad: f32,
    ) -> usize {
        if rad == 0.0 {
            return 0;
        }
        let dpi = self.doc_dpi();
        let mut carried = 0usize;
        let mut layers = 0usize;
        for li in 0..self.doc.layers.len() {
            let Some(layer) = self.doc.layers.get(li) else {
                continue;
            };
            if !layer.visible || layer.lock {
                continue;
            }
            let Some(ts) = layer.texts() else { continue };
            let mut ts = ts.clone();
            let moved = mn_core::balloon::rotate_texts_in(orig, &mut ts, pivot, rad);
            if moved.is_empty() {
                continue;
            }
            for &i in &moved {
                let shaped = self
                    .text_engine
                    .as_ref()
                    .and_then(|e| e.render(&ts.texts[i], dpi).ok().flatten());
                ts.texts[i].cache = shaped;
            }
            carried += moved.len();
            layers += 1;
            self.push_cmd(AppCmd::TextCommit {
                layer: li,
                texts: ts,
            });
        }
        if carried > 0 {
            // One gesture, one undo: the HistoryWrapLast the caller queues
            // right behind these commits bundles them with the balloon's.
            self.set_status(format!(
                "balloon turned — {carried} text{} came with it (still editable; one undo takes the turn and its lettering together)",
                if carried == 1 { "" } else { "s" }
            ));
        }
        layers
    }

    /// The move's half of the lettering carry (CSP manual, moving
    /// balloons): a moved bubble takes the texts inside it along, by the
    /// same geometric pairing as the turn — no stored link, hidden and
    /// locked layers untouched. Translation never reshapes a glyph, so
    /// unlike the turn the shaped caches stay valid and no re-render is
    /// needed.
    pub(crate) fn translate_texts_with_balloon(&mut self, orig: &Balloon, d: [f32; 2]) -> usize {
        if d[0] == 0.0 && d[1] == 0.0 {
            return 0;
        }
        let mut carried = 0usize;
        let mut layers = 0usize;
        for li in 0..self.doc.layers.len() {
            let Some(layer) = self.doc.layers.get(li) else {
                continue;
            };
            if !layer.visible || layer.lock {
                continue;
            }
            let Some(ts) = layer.texts() else { continue };
            let mut ts = ts.clone();
            let moved = mn_core::balloon::translate_texts_in(orig, &mut ts, d);
            if moved.is_empty() {
                continue;
            }
            carried += moved.len();
            layers += 1;
            self.push_cmd(AppCmd::TextCommit {
                layer: li,
                texts: ts,
            });
        }
        if carried > 0 {
            self.set_status(format!(
                "balloon moved — {carried} text{} came with it (still editable; one undo takes the move and its lettering together)",
                if carried == 1 { "" } else { "s" }
            ));
        }
        layers
    }

    /// The resize's half of the lettering carry (owner, 2026-08-25): a
    /// resized bubble keeps its lettering at the same relative position —
    /// centre-fraction of the old box, same fraction of the new one — by
    /// the same geometric pairing as the turn and the move. No stored
    /// link, hidden and locked layers untouched, type size untouched.
    pub(crate) fn scale_texts_with_balloon(&mut self, orig: &Balloon, new_bbox: [f32; 4]) -> usize {
        let mut carried = 0usize;
        let mut layers = 0usize;
        for li in 0..self.doc.layers.len() {
            let Some(layer) = self.doc.layers.get(li) else {
                continue;
            };
            if !layer.visible || layer.lock {
                continue;
            }
            let Some(ts) = layer.texts() else { continue };
            let mut ts = ts.clone();
            let moved = mn_core::balloon::scale_texts_in(orig, &mut ts, new_bbox);
            if moved.is_empty() {
                continue;
            }
            carried += moved.len();
            layers += 1;
            self.push_cmd(AppCmd::TextCommit {
                layer: li,
                texts: ts,
            });
        }
        if carried > 0 {
            self.set_status(format!(
                "balloon resized — {carried} text{} kept its place in it (still editable; one undo takes the resize and its lettering together)",
                if carried == 1 { "" } else { "s" }
            ));
        }
        layers
    }

    /// ROADMAP good-first-issue #1 — **fit a balloon to its text**.
    ///
    /// Which text? The same geometric pairing the rest of the app uses: there
    /// is no stored balloon→text link (see `carry_texts_with_balloon` above)
    /// and this does not invent one. Every VISIBLE text layer is walked bottom
    /// to top and the last hit wins, so "topmost" means the same thing here as
    /// it does to a click. A hidden layer is not a candidate — you cannot see
    /// what it would size the bubble against.
    ///
    /// A LOCKED text layer still counts: the fit reads the lettering and
    /// writes only the balloon, so nothing locked is edited.
    ///
    /// The reshape goes out as one `BalloonCommit`, which is the single
    /// `set_balloons` step every other Tool Property balloon edit records —
    /// one press, one undo.
    pub(crate) fn fit_balloon_to_text(&mut self, layer: usize, balloon: usize) {
        let Some(bs) = self
            .doc
            .layers
            .get(layer)
            .and_then(|l| l.balloons())
            .cloned()
        else {
            return;
        };
        let Some(body) = bs.balloons.get(balloon) else {
            return;
        };
        let mut found: Option<(usize, usize)> = None;
        for li in 0..self.doc.layers.len() {
            let Some(l) = self.doc.layers.get(li) else {
                continue;
            };
            if !l.visible {
                continue;
            }
            let Some(ts) = l.texts() else { continue };
            if let Some(i) = mn_core::balloon::text_in(body, ts) {
                found = Some((li, i));
            }
        }
        let Some((li, ti)) = found else {
            self.set_status("no lettering in this balloon — type the text into it first");
            return;
        };

        let dpi = self.doc_dpi();
        let mut bs2 = bs.clone();
        let changed = self
            .doc
            .layers
            .get(li)
            .and_then(|l| l.texts())
            .and_then(|ts| ts.texts.get(ti))
            .is_some_and(|item| {
                bs2.balloons[balloon].fit_to_text(item, mn_text::font_px(item, dpi))
            });
        if !changed {
            self.set_status("the balloon already fits its lettering");
            return;
        }
        self.push_cmd(AppCmd::BalloonCommit {
            layer,
            balloons: bs2,
        });
        self.set_status("balloon fitted to its lettering — the tail came with it");
    }

    // --- figure + gradient (round 24) ----------------------------------------

    /// The closed path of the current figure shape, canvas px.
    pub fn figure_path(&self, a: (f32, f32), b: (f32, f32)) -> Vec<[f32; 2]> {
        match self.figure_mode {
            FigureMode::Line => vec![[a.0, a.1], [b.0, b.1]],
            FigureMode::Rect => vec![[a.0, a.1], [b.0, a.1], [b.0, b.1], [a.0, b.1]],
            FigureMode::Ellipse => {
                let cx = (a.0 + b.0) * 0.5;
                let cy = (a.1 + b.1) * 0.5;
                let rx = ((b.0 - a.0).abs() * 0.5).max(0.5);
                let ry = ((b.1 - a.1).abs() * 0.5).max(0.5);
                let n = 96;
                (0..n)
                    .map(|k| {
                        let t = k as f32 / n as f32 * std::f32::consts::TAU;
                        [cx + rx * t.cos(), cy + ry * t.sin()]
                    })
                    .collect()
            }
            // Row 157 / FG-002: stage one of the Curve is a straight line,
            // and shows as one. The bend is stage two's business.
            FigureMode::Arc => vec![[a.0, a.1], [b.0, b.1]],
            // Both click-list gestures: no drag geometry to preview, the
            // commit builds their path (see finish_figure_poly). Smart
            // shape joins them for the opposite reason — its gesture is a
            // freehand stroke that inks itself, so there is no a→b drag
            // here to turn into geometry (the overlay previews the
            // RECOGNIZED shape instead; see `SmartShape::preview`).
            FigureMode::Polygon | FigureMode::Curve | FigureMode::Smart => vec![],
            // Stream line: the drag reads as the motion arrow it sets.
            FigureMode::Stream => vec![[a.0, a.1], [b.0, b.1]],
            // Saturated line and the two flashes: a circle around the
            // centre at the dragged radius — the ring the generated lines
            // (or spikes) will reach.
            FigureMode::Focus | FigureMode::Urchin | FigureMode::SolidFlash => {
                let r = (b.0 - a.0).hypot(b.1 - a.1).max(0.5);
                let n = 96;
                (0..=n)
                    .map(|k| {
                        let t = k as f32 / n as f32 * std::f32::consts::TAU;
                        [a.0 + r * t.cos(), a.1 + r * t.sin()]
                    })
                    .collect()
            }
        }
    }

    /// Ink one figure path through the active brush (CSP figures stroke with
    /// the drawing brush), optionally filling it first — one undo step: the
    /// fill joins the stroke's open op, and the selection mask applies to
    /// both at once.
    fn ink_figure(&mut self, path: &[[f32; 2]], close: bool) {
        if path.len() < 2 {
            return;
        }
        // Fill FIRST, inside the stroke's op bracket.
        let fill = close && self.figure_fill && path.len() >= 3;
        if fill {
            let c = self.active_color();
            self.doc.fill_polygon(path, c, 1.0);
            // fill_polygon brackets its own op — accept two steps for v1
            // unless we inline it; CSP merges them, noted in RESUME.
        }
        self.begin_stroke(PointerKind::Pen); // synthetic — no mouse floor
        let radius = self.brush_radius().max(0.5);
        let spacing = (radius * 0.25).max(1.0);
        let now_ms = std::time::Instant::now();
        let t0 = now_ms.elapsed().as_secs_f64() * 1000.0;
        let mut samples: Vec<PenSample> = Vec::new();
        let n = path.len();
        let segs = if close { n } else { n - 1 };
        for i in 0..segs {
            let p = path[i];
            let q = path[(i + 1) % n];
            let d = ((q[0] - p[0]).hypot(q[1] - p[1]) / spacing).ceil().max(1.0) as usize;
            for k in 0..d {
                let t = k as f32 / d as f32;
                samples.push(PenSample {
                    x: p[0] + (q[0] - p[0]) * t,
                    y: p[1] + (q[1] - p[1]) * t,
                    pressure: 1.0,
                    tilt_x: 0.0,
                    tilt_y: 0.0,
                    t_ms: t0 + (i * 16 + k) as f64,
                });
            }
        }
        let last = *path.last().expect("path");
        samples.push(PenSample {
            x: last[0],
            y: last[1],
            pressure: 1.0,
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_ms: t0 + samples.len() as f64 * 4.0 + 16.0,
        });
        // `push_batch` takes CLIENT-space samples — it runs every one of
        // them through `viewport.to_canvas` itself, because that is the
        // space a tablet reports in. Every path that reaches here is in
        // CANVAS space (the drag corners, the polygon's vertices, the
        // recognized Smart shape), so it has to be mapped back on the way
        // in. Without this the figure is laid down at whatever canvas
        // point its own canvas coordinates happen to name when read as
        // screen ones: right only at the identity viewport, which is
        // exactly what every figure test pinned, so the suite never saw
        // it. At a fitted page the shape lands off the paper entirely.
        let samples: Vec<PenSample> = samples
            .into_iter()
            .map(|s| {
                let (x, y) = self.viewport.to_screen(s.x, s.y);
                PenSample { x, y, ..s }
            })
            .collect();
        self.push_batch(&samples);
        self.end_stroke();
        self.set_status(match self.figure_mode {
            FigureMode::Line => "line inked",
            FigureMode::Rect => "rectangle inked",
            FigureMode::Ellipse => "ellipse inked",
            FigureMode::Polygon => "polygon inked",
            FigureMode::Curve | FigureMode::Arc => "curve inked",
            // Row 156: the swap writes its own, richer line right after
            // this one (it names the shape it recognized).
            FigureMode::Smart => "shape inked",
            // A generating release never reaches ink_figure (it makes a
            // layer instead) — arms exist for exhaustiveness only.
            FigureMode::Stream | FigureMode::Focus => "lines generated",
            FigureMode::Urchin | FigureMode::SolidFlash => "flash generated",
        });
    }

    /// Row 157: what a figure drag's RELEASE means. For most sub tools it
    /// still means "ink it" — but `FG-002`'s Curve and `FG-011`'s "Adjust
    /// angle after fixed" turn the release into a hand-off: the size is
    /// fixed, and a second stage steers one more parameter until a click
    /// commits. The whole state machine is here so there is one place that
    /// decides, and `finish_figure_drag` keeps its old meaning for callers
    /// (tests, screenshots) that want the one-stage behaviour outright.
    pub fn release_figure_drag(&mut self, a: (f32, f32), b: (f32, f32)) {
        // A tap, not a drag: there is no shape to adjust, and entering a
        // second stage on an accidental click would strand the user in a
        // mode with nothing on screen. Falls through to the old message.
        let tiny = (b.0 - a.0).abs() + (b.1 - a.1).abs() < 2.0;
        let kind = if tiny || self.figure_mode.generates() {
            None
        } else if self.figure_mode == FigureMode::Arc {
            Some(FigureStage2Kind::Bend)
        } else if self.figure_adjust_angle && self.figure_mode.can_adjust_angle() {
            Some(FigureStage2Kind::Angle)
        } else {
            None
        };
        let Some(kind) = kind else {
            self.finish_figure_drag(a, b);
            return;
        };
        // Seed `cur` at the no-op: the baseline's midpoint bends nothing,
        // and `b` itself turns nothing. Committing without moving therefore
        // inks exactly the shape stage one already showed.
        let cur = match kind {
            FigureStage2Kind::Bend => ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5),
            FigureStage2Kind::Angle => b,
        };
        self.figure_stage2 = Some(FigureStage2 {
            a,
            b,
            cur,
            kind,
            shift: false,
        });
        self.set_status(match kind {
            FigureStage2Kind::Bend => {
                "now bend it — the curve follows the pointer; click inks it, Esc cancels"
            }
            FigureStage2Kind::Angle => {
                "now set the angle — click or Enter inks it, Shift snaps 15°, Esc cancels"
            }
        });
    }

    /// Row 157: the pointer moved with no button down. Only the second
    /// stage cares — it is the one gesture here that steers on hover.
    /// Called from the pointer plumbing beside `last_pointer`, because
    /// `canvas_move` only runs while the canvas owns a held button.
    pub fn figure_hover(&mut self, x: i32, y: i32) {
        if self.figure_stage2.is_none() || self.shell.owns_pointer(x, y) {
            // Over a palette the pointer is not aiming at the page — freeze
            // the preview where it was rather than swinging the curve at
            // whatever panel the hand crossed on its way somewhere.
            return;
        }
        let (cx, cy) = self.viewport.to_canvas(x as f32, y as f32);
        let shift = self.shell.sync_modifiers().shift;
        if let Some(s) = &mut self.figure_stage2 {
            s.cur = (cx, cy);
            s.shift = shift;
        }
        self.needs_redraw = true;
    }

    /// Row 157: the path a second stage would ink right now — the bent
    /// curve, or the shape spun to the pointer. ONE function, so the
    /// overlay preview and the commit cannot disagree about what you are
    /// about to get.
    pub fn figure_stage2_path(&self, s: &FigureStage2) -> Vec<[f32; 2]> {
        match s.kind {
            FigureStage2Kind::Bend => {
                mn_core::balloon::quad_through([s.a.0, s.a.1], [s.b.0, s.b.1], [s.cur.0, s.cur.1])
            }
            FigureStage2Kind::Angle => {
                let (sin, cos) = s.angle().sin_cos();
                let c = [(s.a.0 + s.b.0) * 0.5, (s.a.1 + s.b.1) * 0.5];
                self.figure_path(s.a, s.b)
                    .into_iter()
                    .map(|p| {
                        let (dx, dy) = (p[0] - c[0], p[1] - c[1]);
                        [c[0] + dx * cos - dy * sin, c[1] + dx * sin + dy * cos]
                    })
                    .collect()
            }
        }
    }

    /// Row 157: the click (or Enter) that ends a second stage. One undo
    /// press, like every other figure — the bend and the spin are gesture
    /// state, not history.
    pub fn finish_figure_stage2(&mut self) {
        let Some(s) = self.figure_stage2.take() else {
            return;
        };
        // Re-guard at the COMMIT, for the same reason `finish_figure_poly`
        // does: this gesture spans a release and a second click with the
        // Layers palette live throughout, so the layer that was legal under
        // the press need not be the layer under the commit.
        if self.guard_frame_layer() {
            self.needs_redraw = true;
            return;
        }
        let path = self.figure_stage2_path(&s);
        // Bend inks an OPEN curve (never closed, never filled); Angle inks
        // the same closed shape stage one built, turned.
        self.ink_figure(&path, s.kind == FigureStage2Kind::Angle);
        self.needs_redraw = true;
    }

    /// Row 157: Esc during a second stage throws the whole figure away —
    /// the size drag included. Nothing was inked yet, so there is nothing
    /// to undo and no half-shape left behind.
    pub fn cancel_figure_stage2(&mut self) {
        if self.figure_stage2.take().is_some() {
            self.set_status("figure cancelled");
            self.needs_redraw = true;
        }
    }

    // --- row 156 / `FG-020`: Smart shape -------------------------------------

    /// The pointer moved during a Smart shape stroke: record the path, and
    /// restart the hold if the hand has actually gone somewhere. The slop is
    /// in SCREEN px because that is where the tremor is — a canvas-px
    /// threshold would be four times as strict at 400% zoom.
    pub fn smart_shape_move(&mut self, x: f32, y: f32) {
        let (cx, cy) = self.viewport.to_canvas(x, y);
        // Sampled out here, before the borrow — and sticky: Shift held for
        // any part of the stroke means the artist asked for the straight
        // line, so the recognizer keeps out of it (`FG-024`).
        let shift = self.shell.sync_modifiers().shift;
        // `FG-021`: past the hold the pen stops drawing and starts
        // ADJUSTING. Nothing more is recorded into the stroke's path, and
        // Shift stops meaning the `FG-024` refusal here — that decision was
        // made and spent, and from now on Shift means "make it regular".
        if self.smart_shape.as_ref().is_some_and(|g| g.armed()) {
            let begin = self.smart_shape.as_ref().is_some_and(|g| {
                g.adjust.is_some()
                    || (x - g.anchor.0).hypot(y - g.anchor.1) > crate::app::SMART_HOLD_SLOP_PX
            });
            if begin {
                self.smart_shape_adjust(cx, cy, shift);
            }
            return;
        }
        let Some(g) = &mut self.smart_shape else {
            return;
        };
        g.pts.push([cx, cy]);
        g.refused |= shift;
        if (x - g.anchor.0).hypot(y - g.anchor.1) <= crate::app::SMART_HOLD_SLOP_PX {
            return;
        }
        // Moved on: the armed preview is stale, and the clock restarts here.
        let was_ripe = g.ripe;
        g.anchor = (x, y);
        g.still_since = std::time::Instant::now();
        g.ripe = false;
        g.hit = None;
        if was_ripe {
            self.needs_redraw = true;
        }
    }

    /// `FG-021`: the drag that follows a matured hold. The figure is sized
    /// and turned about its pivot by the similarity that carries the handle
    /// to the pointer; Shift forces the regular form (a perfect circle, a
    /// square, a 45° line). Nothing is committed until the release.
    ///
    /// `shift` is sampled by the CALLER, for the same reason `refused` is
    /// (see `SmartShape`): reading `GetKeyState` in here would make the
    /// gesture untestable and make the suite depend on whether a developer
    /// happened to be holding Shift while it ran.
    pub(crate) fn smart_shape_adjust(&mut self, cx: f32, cy: f32, shift: bool) {
        let Some(g) = self.smart_shape.as_mut() else {
            return;
        };
        if g.adjust.is_none() {
            let Some(base) = g.hit.clone() else {
                return;
            };
            // The pivot and the handle. A CLOSED figure turns about its own
            // centre and is dragged by the pen where it came to rest, which
            // is where the hand already is — no jump on the first frame. An
            // OPEN one is pinned at its start and dragged by its far END,
            // which is what makes the manual's "adjust the position of the
            // end point" land exactly under the pointer.
            let (pivot, from) = if base.closed() {
                let n = base.path.len().max(1) as f32;
                let c = [
                    base.path.iter().map(|p| p[0]).sum::<f32>() / n,
                    base.path.iter().map(|p| p[1]).sum::<f32>() / n,
                ];
                (c, *g.pts.last().unwrap_or(&c))
            } else {
                let a = *base.path.first().unwrap_or(&[cx, cy]);
                (a, *base.path.last().unwrap_or(&a))
            };
            let shape = base.clone();
            g.adjust = Some(crate::app::SmartAdjust {
                base,
                pivot,
                from,
                shape,
            });
        }
        let Some(a) = g.adjust.as_mut() else {
            return;
        };
        // Transform first, regularize second: the 45° snap is about the
        // line's final direction, so a rotation applied after it would take
        // it straight back off 45°.
        let moved = a.base.dragged(a.pivot, a.from, [cx, cy]);
        a.shape = if shift { moved.regularized() } else { moved };
        let label = a.shape.kind.label();
        self.set_status(if shift {
            format!("smart shape: {label} — regular while Shift is held, release inks it")
        } else {
            format!("smart shape: {label} — drag to size and turn it, Shift makes it regular")
        });
        self.needs_redraw = true;
    }

    /// Has the pointer stood still long enough? Called from the frame head
    /// (a still pointer sends no events) and once more at the release.
    /// Idempotent: `ripe` latches, so the recognizer runs ONCE per hold
    /// however many frames the artist holds for.
    pub fn smart_shape_tick(&mut self) {
        let hold = std::time::Duration::from_millis(self.prefs.smart_hold_ms);
        let tol = self.prefs.smart_fit_tol;
        let Some(g) = self.smart_shape.as_ref() else {
            return;
        };
        if g.ripe || g.still_since.elapsed() < hold {
            return;
        }
        // `FG-024`, where the row refuses — decided at the pointer events
        // (see `SmartShape::refused`), never re-read here.
        let refuse = g.refused;
        let pts = g.pts.clone();
        let hit = if refuse {
            None
        } else {
            mn_core::shape_fit::recognize_with(&pts, tol)
        };
        let msg = match &hit {
            Some(r) => format!(
                "hold: release to make it a {} — keep moving to keep the stroke",
                r.kind.label()
            ),
            None if refuse => "held, but this stroke is snapped — it stays as drawn".to_string(),
            None => "held, but that is not a shape — it stays as drawn".to_string(),
        };
        if let Some(g) = self.smart_shape.as_mut() {
            g.ripe = true;
            g.hit = hit;
        }
        self.set_status(msg);
        self.needs_redraw = true;
    }

    /// `FG-024`: is a ruler or guide currently deciding where ink may land?
    /// Snapped strokes are the one family Smart shape must keep its hands
    /// off — the artist already said what shape they wanted.
    fn rulers_deciding(&self) -> bool {
        let r = &self.doc.rulers;
        r.on && !(r.items.is_empty() && r.curves.is_empty())
    }

    /// The release. Either the hold matured on something recognizable — in
    /// which case the freehand ink comes back off the page and the figure
    /// goes down in its place — or nothing happens at all and the stroke
    /// the artist drew is the stroke they keep.
    ///
    /// ONE undo press for the pair. The freehand stroke is a finished undo
    /// group by now, so the swap is: undo it (through `dispatch`, which
    /// carries the cache-invalidation doors a raw `Document::undo` would
    /// skip), then ink the figure — and a push CLEARS the redo branch, so
    /// the group we just undid is gone rather than sitting one Ctrl+Y away.
    /// Net history movement: +1.
    pub fn finish_smart_shape(&mut self) {
        let Some(g) = self.smart_shape.take() else {
            return;
        };
        let Some(hit) = g.preview() else {
            return; // no hold, or no shape: the stroke stands, always.
        };
        // The safety interlock. We are about to press Undo on the user's
        // behalf, so we must be certain the newest history entry is OUR
        // stroke and nothing else. `op_count` is the monotonic tally — NOT
        // `undo_len`, which stops moving once the depth cap is reached and
        // would make this check fail silently for anyone with a deep
        // session. Exactly one op since the press = that op is the stroke.
        if self.doc.op_count() != g.ops_at_start + 1 || self.doc.undo_len() == 0 {
            self.set_status(
                "smart shape: the stroke could not be swapped cleanly — kept it as drawn",
            );
            self.needs_redraw = true;
            return;
        }
        let kind = hit.kind;
        let path = hit.path.clone();
        let closed = hit.closed();
        let bbox = hit.bbox();
        crate::cmd::dispatch(self, AppCmd::Undo);
        self.ink_figure(&path, closed);
        let armed = self.arm_smart_transform(bbox);
        let mut msg = format!("smart shape: your stroke became a {}", kind.label());
        if armed {
            msg.push_str(" — drag to adjust it, Enter commits, Esc cancels");
        }
        self.set_status(msg);
        self.needs_redraw = true;
    }

    /// `FG-022`/`FG-023`, this round's edit mode: open the Transform float
    /// over the figure that just landed, so scale and rotate are one drag
    /// away (the import-as-layer precedent — art lands, handles arm).
    ///
    /// The lift is the figure's BOUNDING BOX, so any art already inside that
    /// box comes along for the ride. That is the honest v1 consequence of
    /// inking into the raster instead of keeping a shape object, and it is
    /// the same bargain the Object tool's raster-ink grab already makes.
    fn arm_smart_transform(&mut self, b: [f32; 4]) -> bool {
        let l = self.doc.active_layer();
        if l.lock || l.is_vector() || l.records_strokes() || l.folder {
            return false;
        }
        let pad = self.brush_radius().max(1.0) + 2.0;
        let r = [
            (b[0] - pad).floor().max(0.0) as i32,
            (b[1] - pad).floor().max(0.0) as i32,
            (b[2] + pad).ceil().min(self.doc.size.0 as f32) as i32,
            (b[3] + pad).ceil().min(self.doc.size.1 as f32) as i32,
        ];
        r[0] < r[2] && r[1] < r[3] && crate::cmd::open_layer_transform(self, self.doc.active, r)
    }

    /// Row 157 / `FG-012`: Backspace (or a right-click) takes back the LAST
    /// placed point of a multi-point figure WITHOUT abandoning the figure —
    /// the row people miss most, because the alternative is Esc and starting
    /// a twelve-click polygon over for one bad vertex.
    ///
    /// At the first point there is nothing left to keep, so it ends the
    /// gesture rather than being a dead key — the same rule the magnetic
    /// lasso's anchor walk-back already uses.
    pub fn figure_undo_point(&mut self) {
        let Some(pts) = &mut self.figure_poly else {
            return;
        };
        pts.pop();
        let left = pts.len();
        if left == 0 {
            self.figure_poly = None;
            self.set_status("figure cancelled — nothing left to take back");
        } else {
            self.set_status(format!(
                "last point removed — {left} left, Backspace again to keep walking back"
            ));
        }
        self.needs_redraw = true;
    }

    /// The dense path an in-progress click list would ink, and — for each of
    /// its points — which SOURCE segment it came from.
    ///
    /// The segment map exists so `FG-013`'s "tap the line to add a point"
    /// can hit-test what the artist can actually SEE. For Polygon the drawn
    /// line is the chords and the two agree; for the Curve the drawn line is
    /// the spline, which at a hard bend runs a long way off the chord it
    /// spans, and hit-testing chords there means clicking the visible curve
    /// quietly appends a point at the end instead of inserting one. The map
    /// is exact rather than nearest-anchor because `tessellate_open` emits
    /// every control point verbatim (the Hermite basis at `t = 0` is the
    /// identity), so a forward scan for those points partitions the dense
    /// path at precisely the right places.
    fn figure_poly_dense(&self, pts: &[FigureAnchor]) -> (Vec<[f32; 2]>, Vec<usize>) {
        let path: Vec<[f32; 2]> = pts.iter().map(|a| [a.p.0, a.p.1]).collect();
        if path.len() < 2 {
            return (path, Vec::new());
        }
        if self.figure_mode != FigureMode::Curve {
            let seg = (0..path.len()).map(|i| i.min(path.len() - 2)).collect();
            return (path, seg);
        }
        let corners: Vec<bool> = pts.iter().map(|a| a.corner).collect();
        let dense = mn_core::balloon::tessellate_open_corners(&path, &corners);
        let last = path.len() - 2;
        let mut seg = Vec::with_capacity(dense.len());
        let mut si = 0usize;
        for d in &dense {
            if si + 1 < path.len() && *d == path[si + 1] {
                si += 1;
            }
            seg.push(si.min(last));
        }
        (dense, seg)
    }

    /// `FG-013`/`FG-014`/`FG-016`: a press on an in-progress Polygon or
    /// Continuous-curve click list that means EDIT rather than "one more
    /// point". Returns whether it was consumed — `false` sends the press
    /// back to the caller's ordinary append.
    ///
    /// `ctrl`/`alt` come from the caller (see `canvas_down`'s Figure arm) so
    /// the whole grammar is drivable from a test without a keyboard.
    ///
    /// Four outcomes, in the order they are tried:
    ///
    /// * **Ctrl + an anchor** → arm the `FG-014` move drag.
    /// * **Alt + an anchor** → flip `FG-016`'s corner bit. On Polygon this is
    ///   a no-op with an explanation, because a polygon is corners already;
    ///   it is still CONSUMED, so Alt+tapping a vertex never silently drops a
    ///   duplicate vertex on top of the one you were aiming at.
    /// * **A plain tap on an anchor** → delete it. Down to nothing, the
    ///   figure ends exactly as `FG-012`'s Backspace ends it.
    /// * **A plain tap on the drawn line** → insert an anchor there, in the
    ///   list position that span belongs to.
    ///
    /// DEVIATION from `FG-016`, deliberate. CSP also toggles the LAST
    /// anchor's corner on a plain tap. We cannot have that: a plain tap on an
    /// anchor is `FG-013`'s delete, and on the Curve a plain tap on the last
    /// anchor is already the shipped commit gesture (rows 84/85 — it is what
    /// a double-click reads as). Both of those are reachable by accident far
    /// more often than a corner toggle is wanted, so the corner toggle keeps
    /// Alt and only Alt, on every anchor including the last.
    ///
    /// REFUSED: `FG-015` (Ctrl+drag a direction point, Space to switch to its
    /// anchor). There is nothing here to drag. Our Continuous curve is a
    /// Catmull-Rom spline THROUGH the clicked points — `tessellate_open`
    /// derives every tangent from the neighbouring anchors — so it has no
    /// direction points, no Bezier handles, and no off-curve control polygon
    /// for Space to toggle away from. `FG-015` is not deferred work; it is a
    /// row that only exists for the Bezier sub tools (`FG-005`/`FG-006`),
    /// which are themselves absent. Implementing it would mean replacing the
    /// curve model first, and that is a different row.
    pub(crate) fn figure_poly_edit(&mut self, cx: f32, cy: f32, ctrl: bool, alt: bool) -> bool {
        // The house handle-hit radius (`vector_edit`, the text and frame
        // handles): 10 SCREEN px, so the grab stays the same size under the
        // hand at every zoom, with a canvas-px floor so it never collapses.
        let tol = (10.0 / self.viewport.zoom.max(0.01)).max(2.0);
        let Some(pts) = &self.figure_poly else {
            return false;
        };
        if pts.is_empty() {
            return false;
        }
        // Nearest anchor within reach; ties go to the LATER one, because
        // that is the one drawn on top (`vector_edit`'s picking order).
        let hit = pts
            .iter()
            .enumerate()
            .rev()
            .map(|(i, a)| (i, (a.p.0 - cx).hypot(a.p.1 - cy)))
            .filter(|(_, d)| *d <= tol)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i);

        if let Some(i) = hit {
            if ctrl {
                self.figure_anchor_drag = Some(i);
                self.set_status("moving that point — release to drop it");
                self.needs_redraw = true;
                return true;
            }
            if alt {
                if self.figure_mode != FigureMode::Curve {
                    self.set_status("corner/curve only applies to the Continuous curve");
                    return true;
                }
                let a = &mut self.figure_poly.as_mut().expect("checked")[i];
                a.corner = !a.corner;
                let corner = a.corner;
                self.set_status(if corner {
                    "corner point — the curve creases here"
                } else {
                    "smooth point — the curve sweeps through"
                });
                self.needs_redraw = true;
                return true;
            }
            let pts = self.figure_poly.as_mut().expect("checked");
            pts.remove(i);
            let left = pts.len();
            if left == 0 {
                // Same landing as `FG-012` walked to the end: an empty list
                // would keep the overlay alive with nothing in it.
                self.figure_poly = None;
                self.set_status("figure cancelled — no points left");
            } else {
                self.set_status(format!("point deleted — {left} left"));
            }
            self.needs_redraw = true;
            return true;
        }
        if ctrl || alt {
            // Nothing under the modifier: fall through and place a point, so
            // a held key never turns the press into a dead click.
            return false;
        }
        let (dense, seg) = self.figure_poly_dense(pts);
        let at = dense
            .windows(2)
            .enumerate()
            .map(|(j, w)| (j, crate::app::vector_edit::dist_to_segment([cx, cy], w[0], w[1])))
            .filter(|(_, d)| *d <= tol)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(j, _)| seg[j] + 1);
        let Some(at) = at else {
            return false;
        };
        let pts = self.figure_poly.as_mut().expect("checked");
        // The new anchor lands where the artist CLICKED, not on the line
        // they clicked at — the tap says "a point about here", and snapping
        // it to the old path would make the first drag of a correction do
        // nothing visible. Smooth, like any freshly placed point.
        pts.insert(at, FigureAnchor::new(cx, cy));
        let n = pts.len();
        self.set_status(format!("point inserted — {n} now"));
        self.needs_redraw = true;
        true
    }

    /// `FG-014`: put the dragged anchor under the pointer. Self-healing —
    /// the drag index is cleared the moment the list it points into is gone
    /// or has shrunk past it, because the many places that abandon a figure
    /// (Esc, a sub-tool switch, the tool cycle) all just drop `figure_poly`
    /// and must not have to know this field exists.
    fn figure_anchor_drag_move(&mut self, cx: f32, cy: f32) {
        let Some(i) = self.figure_anchor_drag else {
            return;
        };
        match self.figure_poly.as_mut().and_then(|p| p.get_mut(i)) {
            Some(a) => a.p = (cx, cy),
            None => self.figure_anchor_drag = None,
        }
        self.needs_redraw = true;
    }

    /// Figure release (line/rect/ellipse): ink the dragged shape.
    /// Stream/Saturated/flash release: generate an effect-line layer instead.
    pub fn finish_figure_drag(&mut self, a: (f32, f32), b: (f32, f32)) {
        if self.figure_mode.generates() {
            self.finish_figure_lines(a, b);
            return;
        }
        let path = self.figure_path(a, b);
        if path.len() < 2 {
            return;
        }
        let tiny = (b.0 - a.0).abs() + (b.1 - a.1).abs() < 2.0;
        if tiny {
            self.set_status("drag the shape out (line: from start to end)");
            return;
        }
        self.ink_figure(&path, true);
    }

    /// Figure ▸ Stream/Saturated line release: turn the drag's geometry into
    /// a `GenLinesSpec` and place it as a fresh layer (`GenLinesPlace` —
    /// never the dialog's in-place regen; see the AppCmd doc). The tool
    /// knobs (count/width/jitter/inner radius) ride on `figure_stream` /
    /// `figure_focus`; the seed bumps per placement so re-drags reroll.
    fn finish_figure_lines(&mut self, a: (f32, f32), b: (f32, f32)) {
        let radial = self.figure_mode.radial();
        let len = (b.0 - a.0).hypot(b.1 - a.1);
        if len < 8.0 {
            self.set_status(if radial {
                "drag from the centre out to size the inner hole — the lines reach the border on their own"
            } else {
                "drag along the motion to set the angle — the lines cross the panel on their own"
            });
            return;
        }
        // The flashes share Saturated line's knobs — same centre-out
        // gesture, same four values (see FigureLineOpts).
        let opts = if radial {
            let o = self.figure_focus;
            self.figure_focus.seed = o.seed.wrapping_add(1);
            o
        } else {
            let o = self.figure_stream;
            self.figure_stream.seed = o.seed.wrapping_add(1);
            o
        };
        // CSP default lengths (owner, 2026-08-24): the lines CROSS the
        // panel — from the ring past the border, protrusions hidden by
        // the frame folder's coverage. The gesture keeps centre, hole
        // and angle; the drag distance no longer caps the length.
        let panel = panel_at(&self.doc, [a.0, a.1]);
        let bounds = panel.map_or(
            [0.0, 0.0, self.doc.size.0 as f32, self.doc.size.1 as f32],
            |(_, b)| b,
        );
        let (pa, pb, pc, pd) = if radial {
            // Centre from the press, hole from the drag (the fraction
            // knob); the reach runs to the panel's/page's farthest
            // corner plus a border-crossing margin, never shorter than
            // the drag.
            let far = [
                [bounds[0], bounds[1]],
                [bounds[2], bounds[1]],
                [bounds[0], bounds[3]],
                [bounds[2], bounds[3]],
            ]
            .iter()
            .map(|c| (c[0] - a.0).hypot(c[1] - a.1))
            .fold(0.0f32, f32::max);
            let r_out = (far + (far * 0.05).max(32.0)).max(len);
            (a.0, a.1, len * opts.r_in_frac.clamp(0.0, 0.95), r_out)
        } else {
            // Angle from the drag direction; the runs cross the whole
            // panel edge to edge (the AABB diagonal outruns any crossing
            // at any angle), protruding past both sides until the panel
            // clips them.
            let angle = (b.1 - a.1).atan2(b.0 - a.0).to_degrees();
            let cross = ((bounds[2] - bounds[0]).hypot(bounds[3] - bounds[1]) * 1.05).max(len);
            (angle, cross, cross, 0.0)
        };
        let kind = self.figure_mode.gen_kind();
        self.push_cmd(AppCmd::GenLinesPlace(
            mn_core::genlines::GenLinesSpec {
                // Kinds 1/2 keep focus = true: the Object tool's driver
                // handles and their clamps key on it (GenLinesSpec's doc).
                focus: radial,
                kind,
                a: pa,
                b: pb,
                c: pc,
                d: pd,
                count: opts.count,
                width: opts.width,
                jitter: opts.jitter,
                // Focus rays taper toward the convergence like Stream tails
                // (the renderer swaps endpoints for that); the flash kinds'
                // teeth carry their own shape and ignore it.
                taper: match self.figure_mode {
                    FigureMode::Focus | FigureMode::Stream => opts.taper,
                    _ => 0.0,
                },
                // Density: the radial kinds are gap-driven in DEGREES, the
                // stream in px — a flash counts its teeth and takes neither
                // (its `width` is a spike base, and gapping it would fight
                // the renderer's own neighbour clamp).
                gap_deg: if radial && kind == 0 {
                    opts.gap_deg
                } else {
                    0.0
                },
                gap_px: if radial { 0.0 } else { opts.gap_px },
                group: if radial { 0 } else { opts.group },
                group_gap: if radial { 0.0 } else { opts.group_gap },
                jit_gap: opts.jit_gap,
                jit_len: opts.jit_len,
                jit_width: opts.jit_width,
                // WHERE THE DRIVER HANDLES GO. Screen-side only — no
                // renderer reads either — but without them a burst placed
                // near the right edge put its radius handles off the page and
                // a stream's reference line sat at the canvas centre instead
                // of on the run you just drew: nothing to aim at, which is
                // half of "I cannot re-select them" (owner, 2026-08-23).
                hand_deg: if radial {
                    (b.1 - a.1).atan2(b.0 - a.0).to_degrees()
                } else {
                    0.0
                },
                anchor: (!radial).then_some([(a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5]),
                converge: None,
                color: [0, 0, 0],
                seed: opts.seed,
            },
            panel.map(|(i, _)| i),
        ));
    }

    /// Figure ▸ Polygon / Curve commit: ink the clicked point list — the
    /// loop for Polygon, the spline through them for Curve (rows 84/85).
    /// One entry point because they are one gesture: same click list, same
    /// Enter/Esc, same re-guard, same brush.
    pub fn finish_figure_poly(&mut self) {
        let curve = self.figure_mode == FigureMode::Curve;
        let Some(pts) = self.figure_poly.take() else {
            return;
        };
        if pts.len() < if curve { 2 } else { 3 } {
            self.set_status(if curve {
                "a curve needs at least 2 points"
            } else {
                "a polygon needs at least 3 vertices"
            });
            return;
        }
        // RE-GUARD AT THE CLOSE, not just at the first vertex. A polygon
        // spans many clicks with the Layers palette live throughout, so the
        // layer under the press that started it need not be the layer under
        // the press that ends it. Select the frame folder's header (or any
        // locked layer) mid-polygon and the stroke lands on something whose
        // raster is re-derived from vectors — the ink is discarded on the
        // next regeneration, with nothing on screen to say so.
        //
        // Found by the round-127 investigation into whether that discard is
        // reachable at all: for the pen it is not, because every press is
        // guarded. This was the one route past.
        if self.guard_frame_layer() {
            self.needs_redraw = true;
            return;
        }
        self.figure_anchor_drag = None;
        let path: Vec<[f32; 2]> = pts.iter().map(|a| [a.p.0, a.p.1]).collect();
        if curve {
            // The spline is inked through `ink_figure` — the SAME door
            // Figure ▸ Direct draw already uses — so the curve gets the
            // active brush, its pressure/taper, the selection clip, the
            // ruler-free synthetic pen and one undo step, and lands as a
            // recorded vector stroke on a vector layer. Open, never closed.
            //
            // `FG-016`: the corner bits ride into the tessellation, which is
            // the only place they can act — a crease is a property of how the
            // path is BUILT, not of how it is inked, so by the time the brush
            // sees the dense polyline the creases are already in it.
            let corners: Vec<bool> = pts.iter().map(|a| a.corner).collect();
            let dense = mn_core::balloon::tessellate_open_corners(&path, &corners);
            self.ink_figure(&dense, false);
        } else {
            self.ink_figure(&path, true);
        }
        self.needs_redraw = true;
    }

    /// Gradient release: paint the ramp between the drag endpoints.
    pub fn finish_gradient(&mut self, a: (f32, f32), b: (f32, f32)) {
        if (b.0 - a.0).abs() + (b.1 - a.1).abs() < 3.0 {
            self.set_status("drag the ramp direction — colour follows the line");
            return;
        }
        let fg = self.active_color();
        let bg = self.sub_color;
        let (from, to) = match self.grad_mode {
            GradMode::FgToBg => ([fg[0], fg[1], fg[2], 1.0], [bg[0], bg[1], bg[2], 1.0]),
            GradMode::FgToTransparent => ([fg[0], fg[1], fg[2], 1.0], [fg[0], fg[1], fg[2], 0.0]),
            GradMode::TransparentToFg => ([fg[0], fg[1], fg[2], 0.0], [fg[0], fg[1], fg[2], 1.0]),
            // FI-050 never arrives here — the press routes a freeform
            // gesture to `grad_free`, not `grad_drag`. The arm exists so
            // that a caller who reaches `finish_gradient` directly (a test,
            // a screenshot script) gets the straight Main → Sub drag rather
            // than a panic.
            GradMode::Freeform => ([fg[0], fg[1], fg[2], 1.0], [bg[0], bg[1], bg[2], 1.0]),
        };
        if self.fill_live {
            // I-016 / NL-006's live switch (TRIAGE 137): the drag defines the
            // ramp, the colours the endpoints — as parameters, not pixels.
            // The authored stops and options ride along, so a gradient LAYER
            // and a baked gradient are the same ramp either way.
            let kind = mn_core::FillKind::Gradient {
                a: [a.0, a.1],
                b: [b.0, b.1],
                from,
                to,
                mid: self.grad_mid,
                opts: self.grad_opts,
            };
            // …but only onto a layer that is ALREADY a ramp. CSP's gradient
            // tool in layer mode makes a NEW gradient layer; it never turns
            // the layer you are standing on into one, and a tone layer you
            // tuned an hour ago is exactly the thing you do not want a drag
            // to overwrite. The stack (a gradient above the tone) is also
            // the honest picture: two objects, both still editable.
            let li = self.doc.active;
            if matches!(
                self.doc.layers[li].kind,
                mn_core::LayerKind::Fill(mn_core::FillKind::Gradient { .. })
            ) {
                self.push_cmd(crate::cmd::AppCmd::SetFillParams(li, kind));
            } else {
                self.push_cmd(crate::cmd::AppCmd::NewLiveFill(kind));
            }
            return;
        }
        let ramp = mn_core::Ramp::new(from, to, self.grad_mid, self.grad_opts);
        if self.doc.paint_gradient_ramp([a.0, a.1], [b.0, b.1], &ramp) {
            self.mark_dirty();
            self.set_status("gradient painted");
        } else {
            self.set_status("gradient needs a raster layer (unlocked)");
        }
    }

    // --- `FI-050`/`FI-051`: the freeform gradient's multi-stroke gesture ----

    /// The colour the NEXT guide will carry: main, then sub, then main
    /// again for every line after that.
    ///
    /// Guide 2 taking the sub colour is what keeps the two-line gesture
    /// exactly the ramp it always was (main → sub). From the third line on
    /// there is no "other end" to reach for, so each line takes the main
    /// colour as it stands when it is drawn — pick, draw, pick, draw.
    fn grad_free_next_colour(&self, drawn: usize) -> [f32; 4] {
        let c = if drawn == 1 {
            self.sub_color
        } else {
            self.active_color()
        };
        [c[0], c[1], c[2], 1.0]
    }

    /// One guide stroke ended: bank it and wait for the next. Two banked
    /// lines are already a gradient — Enter, or a click away from the lines,
    /// paints it ([`Self::commit_grad_free`]); another drag adds another
    /// colour instead.
    ///
    /// A tap is not a guide. With fewer than two lines down it is refused
    /// outright (a stray click must not paint a page-wide radial ramp); once
    /// the gesture is ready a tap is the COMMIT, which is the figure tool's
    /// "click elsewhere finishes" idiom.
    pub fn release_grad_free(&mut self, cx: f32, cy: f32) {
        let Some(g) = &mut self.grad_free else {
            return;
        };
        g.drawing = false;
        g.cur.push([cx, cy]);
        // Simplify in CANVAS px scaled by zoom, exactly as the frame pen
        // does — the tremor is in screen px, and a shorter guide is a
        // cheaper distance field (see `mn_core::freeform`).
        let eps = (2.0 / self.viewport.zoom.max(0.01)).max(1.0);
        let stroke = mn_core::balloon::simplify_polyline(&std::mem::take(&mut g.cur), eps);
        let span = |pts: &[[f32; 2]]| -> f32 {
            let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
            for p in pts {
                for k in 0..2 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            (hi[0] - lo[0]) + (hi[1] - lo[1])
        };
        let (drawn, ready) = (g.done.len(), g.ready());
        if stroke.len() < 2 || span(&stroke) < 3.0 {
            if ready {
                // The click-elsewhere commit.
                self.commit_grad_free();
                return;
            }
            if drawn == 0 {
                self.grad_free = None;
            }
            self.set_status(if drawn == 0 {
                "draw the FIRST guide line — a tap is not a line"
            } else {
                "draw the SECOND guide line — a tap is not a line"
            });
            return;
        }
        let colour = self.grad_free_next_colour(drawn);
        let Some(g) = &mut self.grad_free else {
            return;
        };
        g.done.push(mn_core::freeform::ColourGuide::new(stroke, colour));
        let n = g.done.len();
        self.set_status(match n {
            1 => "now draw the second line — the sub colour lands on it; Esc cancels".to_string(),
            _ => format!(
                "{n} lines — Enter (or a click) paints; draw another line to add the main \
                 colour to it, Backspace takes one back"
            ),
        });
    }

    /// `FI-050`/`FI-051`: paint what the gesture has drawn. Enter, or a
    /// click away from the lines. Two guides run the ramp, three and up the
    /// colour-per-guide field — one undo press either way.
    pub fn commit_grad_free(&mut self) {
        let Some(g) = self.grad_free.take() else {
            return;
        };
        if !g.ready() {
            // Enter pressed too early: the gesture is put back, not thrown
            // away, and nothing was painted to undo.
            self.grad_free = Some(g);
            self.set_status("a freeform gradient needs two drawn lines — Esc cancels");
            return;
        }
        self.finish_gradient_freeform_multi(&g.done);
        self.needs_redraw = true;
    }

    /// `FI-051`: Backspace takes the LAST guide back — the figure tool's
    /// `FG-012` rule, including its ending: at the first line there is
    /// nothing left to walk back to, so the gesture cancels rather than
    /// leaving a dead key.
    pub fn grad_free_undo_guide(&mut self) {
        let Some(g) = &mut self.grad_free else {
            return;
        };
        g.done.pop();
        let left = g.done.len();
        if left == 0 {
            self.cancel_grad_free();
            return;
        }
        self.set_status(format!(
            "line removed — {left} left, Backspace again to keep walking back"
        ));
        self.needs_redraw = true;
    }

    /// `FI-050`/`FI-051`: the guides are in, apply them. TWO route to the shipped
    /// ramp (byte for byte — `mn_core::doc::paint_gradient_freeform_multi`
    /// dispatches, so there is one place that decides); three and up blend
    /// by proximity with a colour per guide. One undo press either way.
    pub fn finish_gradient_freeform_multi(&mut self, guides: &[mn_core::freeform::ColourGuide]) {
        // Re-guard at the COMMIT: the gesture spans several strokes with the
        // Layers palette live throughout (`finish_figure_stage2`'s reason).
        if self.guard_frame_layer() {
            return;
        }
        // The ramp's ENDS are ignored by the multi door (each guide carries
        // its own colour); what it reads off this is the authored stops and
        // options — mixing space, brightness lift and dither for a field,
        // all of them for the two-line ramp.
        let ramp = mn_core::Ramp::new(
            guides.first().map_or([0.0, 0.0, 0.0, 1.0], |g| g.colour),
            guides.get(1).map_or([1.0, 1.0, 1.0, 1.0], |g| g.colour),
            self.grad_mid,
            self.grad_opts,
        );
        // The GPU tile-kernel seam (`FI-050` follow-up), offered the same
        // way `AppCmd::FilterApply` offers the blur family theirs: the
        // destructure is what lets the closure hold `renderer` while the doc
        // is borrowed. A decline anywhere — software adapter, a job under
        // the floor, a ramp the shader cannot express, a segment pool past
        // the cap, the dispatch canary — paints that batch on the CPU.
        let crate::app::App { doc, renderer, .. } = &mut *self;
        let painted = doc.paint_gradient_freeform_multi_with(guides, &ramp, &mut |job, px| {
            if !renderer.kernels_preferred(px.len() / mn_core::tile::TILE_CHANNELS) {
                return None;
            }
            renderer.run_freeform_kernel(job, px)
        });
        if painted {
            self.mark_dirty();
            // `fill_live` is a LINEAR-ramp switch: `FillKind::Gradient` is
            // two endpoints, and there is nowhere in it to put drawn
            // polylines. Freeform always bakes, and says so rather than
            // silently ignoring the toggle.
            let n = guides.len();
            self.set_status(if self.fill_live {
                "freeform gradient painted — baked, live gradient layers are straight-line only"
                    .to_string()
            } else if n > 2 {
                format!("freeform gradient painted — {n} lines, a colour each")
            } else {
                "freeform gradient painted".to_string()
            });
        } else {
            self.set_status("gradient needs a raster layer (unlocked)");
        }
    }

    /// `FI-050`: Esc (or a tool/sub-tool switch) between the two strokes.
    /// Nothing was painted, so there is nothing to undo and nothing left on
    /// the canvas.
    pub fn cancel_grad_free(&mut self) {
        if self.grad_free.take().is_some() {
            self.set_status("freeform gradient cancelled");
            self.needs_redraw = true;
        }
    }

    /// Object tool + modifier clicks on the SELECTED balloon (TODO "balloon
    /// spline editing"): Ctrl+click on an edge inserts an anchor there,
    /// Ctrl+click on an anchor deletes it, Ctrl+click on a tail handle
    /// deletes that tail; Alt+click on an anchor toggles its corner/smooth
    /// flag. Each edit is ONE undo step. Returns true when the press was
    /// consumed (no drag starts). The Alt-eyedropper only covers
    /// Pen/Eraser/Fill, so Object+Alt is free.
    pub(crate) fn balloon_anchor_edit(&mut self, cx: f32, cy: f32, ctrl: bool, alt: bool) -> bool {
        if !(ctrl || alt) {
            return false;
        }
        let Some((li, bi)) = self.balloon_sel else {
            return false;
        };
        let Some(mut bs) = self.doc.layers.get(li).and_then(|l| l.balloons()).cloned() else {
            return false;
        };
        if bi >= bs.balloons.len() {
            return false;
        }
        let tol = (10.0 / self.viewport.zoom.max(0.01)).max(2.0);
        let commit = |app: &mut Self, bs: BalloonSet, msg: &str| {
            app.push_cmd(AppCmd::BalloonCommit {
                layer: li,
                balloons: bs,
            });
            app.set_status(msg);
        };

        // A handle under the cursor first — Ctrl deletes it, Alt toggles.
        if let Some(h) = bs.balloons[bi].handle_near([cx, cy], tol * 1.4) {
            match (ctrl, h) {
                (true, BalloonHandle::Shape(i)) => {
                    if bs.balloons[bi].delete_anchor(i) {
                        commit(self, bs, "anchor deleted");
                    } else if matches!(bs.balloons[bi].shape, BalloonShape::Polygon { .. }) {
                        self.set_status("a balloon keeps at least 3 anchors");
                    } else {
                        self.set_status("ellipse/rounded bodies resize by their handles");
                    }
                    return true;
                }
                (true, BalloonHandle::TailTip(i)) | (true, BalloonHandle::TailBase(i)) => {
                    if bs.balloons[bi].delete_tail(i) {
                        commit(self, bs, "tail deleted");
                    }
                    return true;
                }
                (false, BalloonHandle::Shape(i)) => {
                    if bs.balloons[bi].toggle_anchor_corner(i) {
                        commit(self, bs, "anchor corner/smooth toggled");
                    } else {
                        self.set_status("only drawn balloons have corner anchors");
                    }
                    return true;
                }
                (false, _) => return false,
            }
        }

        // Ctrl near an edge (away from handles): insert an anchor there.
        if ctrl {
            if let Some((seg, p)) = bs.balloons[bi].edge_point_near([cx, cy], tol) {
                if bs.balloons[bi].insert_anchor(seg, p) {
                    commit(self, bs, "anchor inserted");
                }
                return true;
            }
        }
        false
    }

    /// Object-tool press on a ruler: grab an anchor (that end moves) or the
    /// body (the whole ruler translates). Returns true when the press was
    /// consumed. The tolerance is the shared on-canvas-handle one — screen
    /// px divided by zoom — and the same value gates the anchor and the
    /// body, so there is no band where a press means something else.
    fn ruler_grab(&mut self, cx: f32, cy: f32) -> bool {
        let tol = (10.0 / self.viewport.zoom.max(0.01)).max(2.0);
        // Row 149: only the active layer's rulers are grabbable.
        let Some((ruler, grab)) = self
            .doc
            .rulers
            .for_layer(self.doc.active)
            .grab_near([cx, cy], tol)
        else {
            return false;
        };
        self.ruler_move = Some(RulerMove {
            ruler,
            grab,
            last: [cx, cy],
            moved: false,
            before: self.doc.rulers.clone(),
        });
        // M3 phase A: name the handle. The generic "this end" was right
        // for a line ruler and useless on a perspective set, where WHICH
        // point you grabbed is the whole question. Curve rulers (index
        // past `items`) have no roles and keep the old wording.
        let msg = match grab {
            mn_core::RulerGrab::Anchor(i) => self.ruler_anchor_role(ruler, i).map_or_else(
                || "ruler handle — drag to move this end".into(),
                |r| r.hint(),
            ),
            mn_core::RulerGrab::Body => "ruler — drag to move the whole ruler".to_string(),
        };
        self.set_status(msg);
        true
    }

    /// The role of anchor `i` on the ruler at a combined index (see
    /// [`mn_core::Rulers::grab_near`]) — `None` for a curve ruler's
    /// vertices, which are bare path points.
    fn ruler_anchor_role(&self, ruler: usize, i: usize) -> Option<mn_core::AnchorRole> {
        self.doc
            .rulers
            .items
            .get(ruler)?
            .anchor_roles()
            .get(i)
            .copied()
    }

    /// Select the effect-line run on layer `li` — and MAKE THAT LAYER
    /// ACTIVE. Selecting a run without activating it left three places
    /// disagreeing about what you had picked: the canvas drew handles on
    /// one run, the Layers palette highlighted another, and Layer ▸ Edit
    /// effect lines (which keys on the ACTIVE layer) edited that other
    /// one. Whatever is selected on the page is the layer you are on.
    pub(crate) fn gen_select(&mut self, li: usize) {
        self.gen_sel = Some(li);
        // A half-finished Tool Property drag belongs to the run that was
        // selected when it started; carried over, it would be applied to
        // this one.
        self.gen_edit = None;
        self.text_sel = None;
        self.object_sel = None;
        self.balloon_sel = None;
        if li < self.doc.layers.len() {
            self.doc.active = li;
        }
    }

    /// Object tool press: find what the cursor grabbed, topmost vector layer
    /// first. Balloons: a control handle, then the body. Frames: a vertex
    /// handle, then an edge, then the panel body.
    pub(crate) fn object_hit(&mut self, cx: f32, cy: f32) {
        let tol = (10.0 / self.viewport.zoom.max(0.01)).max(2.0);
        let mut frame_hit: Option<ObjectDrag> = None;
        let mut balloon_hit: Option<BalloonObjDrag> = None;
        // SF-004/005: generated effect-line layers first — a driver
        // handle beats every other affordance; ink under the pointer
        // SELECTS the run (its handles appear). Topmost wins.
        for li in (0..self.doc.layers.len()).rev() {
            let Some(spec) = self.doc.layers[li].genlines else {
                continue;
            };
            if !self.doc.layers[li].visible {
                continue;
            }
            for (mode, p) in gen_handle_points(&spec, self.doc.size) {
                if (p[0] - cx).abs() + (p[1] - cy).abs() <= tol * 1.4 {
                    self.gen_select(li);
                    self.gen_drag = Some(GenLinesDrag {
                        layer: li,
                        mode,
                        start: (cx, cy),
                        cur: (cx, cy),
                        orig: spec,
                    });
                    self.object_pick = Some((cx, cy));
                    return;
                }
            }
            if layer_ink_near(&self.doc.layers[li], cx, cy, tol) {
                self.gen_select(li);
                self.object_pick = Some((cx, cy));
                self.set_status("effect lines selected — drag the blue handles");
                return;
            }
        }
        'outer: for li in (0..self.doc.layers.len()).rev() {
            let layer = &self.doc.layers[li];
            if !layer.visible {
                continue;
            }
            if let Some(bs) = layer.balloons() {
                for bi in (0..bs.balloons.len()).rev() {
                    let b = &bs.balloons[bi];
                    let mode = if let Some(h) = b.handle_near([cx, cy], tol * 1.4) {
                        Some(BalloonDragMode::Handle(h))
                    } else if self.balloon_sel == Some((li, bi)) {
                        // The blue box exists only on the SELECTED balloon
                        // (same rule as frames): lollipop, corners, edges,
                        // then the body drag.
                        let bb = b.bbox();
                        let lolly = [
                            (bb[0] + bb[2]) * 0.5,
                            bb[1] - super::ROTATE_STALK_SCREEN / self.viewport.zoom.max(0.01),
                        ];
                        let near = |p: [f32; 2]| (p[0] - cx).abs() + (p[1] - cy).abs() <= tol * 1.4;
                        let corners = [
                            [bb[0], bb[1]],
                            [bb[2], bb[1]],
                            [bb[2], bb[3]],
                            [bb[0], bb[3]],
                        ];
                        let mids = [
                            [(bb[0] + bb[2]) * 0.5, bb[1]],
                            [bb[2], (bb[1] + bb[3]) * 0.5],
                            [(bb[0] + bb[2]) * 0.5, bb[3]],
                            [bb[0], (bb[1] + bb[3]) * 0.5],
                        ];
                        if near(lolly) {
                            Some(BalloonDragMode::BoxRotate)
                        } else if let Some(i) = corners.iter().position(|&c| near(c)) {
                            Some(BalloonDragMode::BoxCorner(i))
                        } else if let Some(i) = mids.iter().position(|&m| near(m)) {
                            Some(BalloonDragMode::BoxEdge(i))
                        } else if b.contains([cx, cy]) {
                            Some(BalloonDragMode::MoveWhole)
                        } else {
                            None
                        }
                    } else if b.contains([cx, cy]) {
                        Some(BalloonDragMode::MoveWhole)
                    } else {
                        None
                    };
                    if let Some(mode) = mode {
                        balloon_hit = Some(BalloonObjDrag {
                            layer: li,
                            balloon: bi,
                            mode,
                            start: (cx, cy),
                            cur: (cx, cy),
                            orig: b.clone(),
                            shift_snap: false,
                        });
                        break 'outer;
                    }
                }
            }
            let Some(fs) = layer.frames() else { continue };
            for fi in (0..fs.frames.len()).rev() {
                let f = &fs.frames[fi];
                let mode = if let Some(v) = f.vertex_near([cx, cy], tol * 1.4) {
                    Some(ObjectDragMode::Vertex(v))
                } else if self.object_sel == Some((li, fi)) {
                    // Affordances exist only on the SELECTED panel (CSP
                    // Object tool): rotation lollipop, bbox corner scale,
                    // bbox edge stretch, then the plain edge/body drags.
                    let b = f.bbox();
                    let lolly = [
                        (b[0] + b[2]) * 0.5,
                        b[1] - super::ROTATE_STALK_SCREEN / self.viewport.zoom.max(0.01),
                    ];
                    let near = |p: [f32; 2]| (p[0] - cx).abs() + (p[1] - cy).abs() <= tol * 1.4;
                    let corners = [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]];
                    let mids = [
                        [(b[0] + b[2]) * 0.5, b[1]],
                        [b[2], (b[1] + b[3]) * 0.5],
                        [(b[0] + b[2]) * 0.5, b[3]],
                        [b[0], (b[1] + b[3]) * 0.5],
                    ];
                    if near(lolly) {
                        Some(ObjectDragMode::Rotate)
                    } else if let Some(i) = corners.iter().position(|&c| near(c)) {
                        Some(ObjectDragMode::ScaleCorner(i))
                    } else if let Some(i) = mids.iter().position(|&m| near(m)) {
                        Some(ObjectDragMode::ScaleEdge(i))
                    } else if let Some(e) = f.edge_near([cx, cy], tol) {
                        Some(ObjectDragMode::Edge(e))
                    } else if f.contains([cx, cy]) {
                        Some(ObjectDragMode::MoveWhole)
                    } else {
                        None
                    }
                } else if let Some(e) = f.edge_near([cx, cy], tol) {
                    Some(ObjectDragMode::Edge(e))
                } else if f.contains([cx, cy]) {
                    Some(ObjectDragMode::MoveWhole)
                } else {
                    None
                };
                if let Some(mode) = mode {
                    frame_hit = Some(ObjectDrag {
                        layer: li,
                        frame: fi,
                        mode,
                        start: (cx, cy),
                        cur: (cx, cy),
                        orig: f.clone(),
                        shift_snap: false,
                    });
                    break 'outer;
                }
            }
        }
        if let Some(d) = balloon_hit {
            self.balloon_sel = Some((d.layer, d.balloon));
            self.object_pick = Some((cx, cy));
            self.balloon_obj_drag = Some(d);
            self.object_sel = None;
            self.gen_sel = None;
        } else if {
            // Expand arrows (owner 2026-08-26): only the SELECTED frame has
            // them, so they are checked before the ordinary frame hit —
            // an arrow sits just OUTSIDE the bbox and would otherwise fall
            // through to the ink grab. One tap, one undo step; the frame
            // stays selected so the arrows stay up for the next direction.
            let (sx, sy) = self.viewport.to_screen(cx, cy);
            self.frame_expand_arrow_pts()
                .into_iter()
                .find(|&(_, tip)| (sx - tip.x).abs() <= 12.0 && (sy - tip.y).abs() <= 12.0)
                .is_some_and(|(dir, _)| self.frame_expand_press(dir))
        } {
            self.object_pick = Some((cx, cy));
        } else if let Some(d) = frame_hit {
            self.object_sel = Some((d.layer, d.frame));
            self.object_pick = Some((cx, cy));
            self.object_drag = Some(d);
            self.balloon_sel = None;
            self.gen_sel = None;
        } else if let Some(li) = self
            .doc
            .layers
            .iter()
            .enumerate()
            .rev()
            .find(|(_, l)| {
                l.visible
                    && matches!(
                        l.kind,
                        mn_core::LayerKind::Fill(mn_core::fill_layer::FillKind::Tone { .. })
                    )
                    && display_ink_near(l, cx, cy, tol)
            })
            .map(|(i, _)| i)
        {
            // Walk #3 (CSP "Move tone pattern"): ink of a live TONE layer
            // grabs the LATTICE, not the pixels — the raster is derived and
            // must never lift into a float. One commit on release.
            self.doc.active = li;
            self.object_pick = Some((cx, cy));
            self.fill_lattice_drag = Some(FillLatticeDrag {
                layer: li,
                start: (cx, cy),
                cur: (cx, cy),
            });
            self.set_status("tone lattice grabbed — drag moves the dots under the art");
        } else {
            self.object_sel = None;
            self.balloon_sel = None;
            self.gen_sel = None;
            // Owner 2026-08-24: no shape under the pointer — the Object
            // tool grabs RASTER INK directly ("drag the lineart"): the
            // topmost visible plain-raster layer with ink near the press
            // becomes active and lifts into the Transform float, the drag
            // already moving it. Same flow as Ctrl+T, invoked by grabbing.
            let tol2 = (10.0 / self.viewport.zoom.max(0.01)).max(2.0);
            let mut ink = None;
            for li in (0..self.doc.layers.len()).rev() {
                let l = &self.doc.layers[li];
                if !l.visible
                    || l.lock
                    || l.folder
                    || l.is_vector()
                    // A stroke-recording layer's Object-tool affordance is
                    // its GEOMETRY (`vector_hit`, above, on the active
                    // layer); lifting its pixels into the Transform float
                    // would commit a raster the next re-derive throws away.
                    || l.records_strokes()
                    // Shape-carrier layers have their own Object-tool
                    // affordances (handled above); text layers belong to
                    // the text flow, which ran before us.
                    || l.balloons().is_some()
                    || l.frames().is_some()
                    || l.genlines.is_some()
                    || l.texts().is_some()
                    // Live fill layers are DERIVED — their ink is a
                    // rasterized window/param pair, never float material.
                    // Tones got their lattice grab above; flat/gradient
                    // keep their Tool Property surface for now (endpoint
                    // dragging is the open NL v1 cut).
                    || matches!(l.kind, mn_core::LayerKind::Fill(_))
                {
                    continue;
                }
                if layer_ink_near(l, cx, cy, tol2) {
                    ink = Some(li);
                    break;
                }
            }
            // THE SIZE CAP (owner freeze report 2026-08-26): a layer whose
            // ink spans a whole page lifts a page-sized float — the copy,
            // the preview and every dragged frame of a giant quad froze
            // the app on integrated graphics (the mystery "second rect"
            // was exactly that float). CSP's Object tool never lifts
            // raster layers at all; our grab is for LINEART — whole-layer
            // ink of modest size. Oversize: select the layer and say so;
            // Ctrl+T remains the deliberate door for whole-page layers.
            if let Some(li) = ink {
                // 4096 populated tiles ≈ a 4096² page of ink, or roughly
                // half a B4 600 dpi page — the most a drag should ever
                // carry on this GPU.
                const GRAB_MAX_TILES: u64 = 4096;
                let oversize = self.doc.layers[li]
                    .tile_bounds()
                    .map(|(_, _, w, h)| (w as u64 / 64) * (h as u64 / 64) > GRAB_MAX_TILES)
                    .unwrap_or(true);
                if oversize {
                    self.doc.set_active(li);
                    self.set_status(
                        "layer selected — too much ink to grab directly; Ctrl+T transforms it",
                    );
                    ink = None;
                }
            }
            if let Some(li) = ink {
                self.doc.set_active(li);
                let size = self.doc.size;
                // The box hugs the ink, not the tile grid — same rule as
                // Ctrl+T (`transform_lift_rect`).
                let rect = self.doc.layers[li].ink_bounds().map(|b| {
                    [
                        b[0].max(0),
                        b[1].max(0),
                        b[2].min(size.0 as i32),
                        b[3].min(size.1 as i32),
                    ]
                });
                if let Some(r) = rect.filter(|r| r[0] < r[2] && r[1] < r[3])
                    && crate::cmd::open_layer_transform(self, li, r)
                    && let Some(d) = self.transform_drag.as_mut()
                {
                    // The press that opened the float IS the drag: arm the
                    // move gesture so the ink follows the pointer at once
                    // (same state the float's own press handler sets).
                    let g = crate::app::TransformGesture {
                        grab: crate::app::TransformGrab::Move,
                        start: [cx, cy],
                        bbox0: d.bbox,
                        sx0: d.sx,
                        sy0: d.sy,
                        rad0: d.rad,
                        tx0: d.tx,
                        ty0: d.ty,
                    };
                    d.gesture = Some(g);
                    // An ink grab's pure MOVE commits on release (CSP's
                    // Object tool feel); see canvas_up.
                    d.object_lift = true;
                    self.set_status("layer lifted — drag to move, Enter commits, Esc cancels");
                }
            }
        }
    }

    /// Every object whose hit area contains (x, y), in the CLICK
    /// hit-test's total order (texts -> effect-line runs -> balloons ->
    /// frames, topmost first within each family — the r87 precedence) so
    /// the cycle order and the click order can never disagree.
    fn object_candidates_at(&self, x: f32, y: f32) -> Vec<ObjRef> {
        let mut out = Vec::new();
        for li in (0..self.doc.layers.len()).rev() {
            let l = &self.doc.layers[li];
            if !l.visible {
                continue;
            }
            if let Some(ts) = l.texts() {
                for ti in (0..ts.texts.len()).rev() {
                    if ts.texts[ti].contains([x, y], 0.0) {
                        out.push(ObjRef::Text(li, ti));
                    }
                }
            }
        }
        for li in (0..self.doc.layers.len()).rev() {
            let l = &self.doc.layers[li];
            if !l.visible || l.genlines.is_none() {
                continue;
            }
            // The SAME tolerance the click path uses — the doc comment
            // above is a promise that the cycle order and the click order
            // cannot disagree, and a tighter test here would break it.
            let tol = (10.0 / self.viewport.zoom.max(0.01)).max(2.0);
            if layer_ink_near(l, x, y, tol) {
                out.push(ObjRef::Gen(li));
            }
        }
        for li in (0..self.doc.layers.len()).rev() {
            let l = &self.doc.layers[li];
            if !l.visible {
                continue;
            }
            if let Some(bs) = l.balloons() {
                for bi in (0..bs.balloons.len()).rev() {
                    if bs.balloons[bi].contains([x, y]) {
                        out.push(ObjRef::Balloon(li, bi));
                    }
                }
            }
        }
        for li in (0..self.doc.layers.len()).rev() {
            let l = &self.doc.layers[li];
            if !l.visible {
                continue;
            }
            if let Some(fs) = l.frames() {
                for fi in (0..fs.frames.len()).rev() {
                    if fs.frames[fi].contains([x, y]) {
                        out.push(ObjRef::Frame(li, fi));
                    }
                }
            }
        }
        out
    }

    /// The current Object-tool selection as one referent.
    pub(crate) fn object_selection(&self) -> Option<ObjRef> {
        if let Some((li, ti)) = self.text_sel {
            return Some(ObjRef::Text(li, ti));
        }
        if let Some(li) = self.gen_sel {
            return Some(ObjRef::Gen(li));
        }
        if let Some((li, bi)) = self.balloon_sel {
            return Some(ObjRef::Balloon(li, bi));
        }
        if let Some((li, fi)) = self.object_sel {
            return Some(ObjRef::Frame(li, fi));
        }
        None
    }

    /// Rows 78/76: deselect every object (primary + set) without
    /// touching the layers.
    pub(crate) fn clear_object_selection(&mut self) {
        self.text_sel = None;
        self.gen_sel = None;
        self.balloon_sel = None;
        self.object_sel = None;
        self.object_multi.clear();
        self.object_pick = None;
    }

    fn object_select_ref(&mut self, r: ObjRef) {
        self.text_sel = None;
        self.gen_sel = None;
        self.balloon_sel = None;
        self.object_sel = None;
        match r {
            ObjRef::Text(li, ti) => self.text_sel = Some((li, ti)),
            // Cycling onto a run activates its layer, same as clicking it.
            ObjRef::Gen(li) => self.gen_select(li),
            ObjRef::Balloon(li, bi) => self.balloon_sel = Some((li, bi)),
            ObjRef::Frame(li, fi) => self.object_sel = Some((li, fi)),
        }
    }

    /// The owner's Object-tool ask: with something selected, pressing the
    /// tool key AGAIN cycles the selection through every object stacked
    /// under the pick point — selection only, no mutation, no undo.
    /// `O` forward, `Shift+O` back, wraparound.
    pub fn object_cycle(&mut self, forward: bool) {
        let Some(cur) = self.object_selection() else {
            return;
        };
        // The anchor: the pick point, else the selection's bbox centre.
        let anchor = self.object_pick.unwrap_or_else(|| {
            let b = match cur {
                ObjRef::Text(li, ti) => self
                    .doc
                    .layers
                    .get(li)
                    .and_then(|l| l.texts())
                    .and_then(|ts| ts.texts.get(ti))
                    .map(|t| {
                        let c = t.center();
                        [
                            c[0] - t.size[0] * 0.5,
                            c[1] - t.size[1] * 0.5,
                            c[0] + t.size[0] * 0.5,
                            c[1] + t.size[1] * 0.5,
                        ]
                    }),
                ObjRef::Gen(_) => None,
                ObjRef::Balloon(li, bi) => self
                    .doc
                    .layers
                    .get(li)
                    .and_then(|l| l.balloons())
                    .and_then(|bs| bs.balloons.get(bi))
                    .map(|b| b.bbox()),
                ObjRef::Frame(li, fi) => self
                    .doc
                    .layers
                    .get(li)
                    .and_then(|l| l.frames())
                    .and_then(|fs| fs.frames.get(fi))
                    .map(|f| f.bbox()),
            };
            b.map(|b| ((b[0] + b[2]) * 0.5, (b[1] + b[3]) * 0.5))
                .unwrap_or((f32::NAN, f32::NAN))
        });
        if !anchor.0.is_finite() {
            self.set_status("nothing stacked here to cycle to");
            return;
        }
        let cands = self.object_candidates_at(anchor.0, anchor.1);
        if cands.len() < 2 {
            return; // nothing onward — keep the selection as-is
        }
        let n = cands.len();
        let i = cands.iter().position(|c| *c == cur).unwrap_or(0);
        let j = if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        };
        let (from, to) = (cur.label(), cands[j].label());
        self.object_select_ref(cands[j]);
        self.set_status(format!("{from} → {to} ({} of {n} here)", j + 1));
        self.needs_redraw = true;
    }

    /// KB-020: arm the live size drag at the press point, reading the
    /// current px diameter the Tool Property slider shares.
    pub(crate) fn size_drag_begin(&mut self, x: f32) {
        self.size_drag = Some((x, self.props_current.size_px));
        self.set_status(format!("brush size: {:.1} px", self.brush_radius() * 2.0));
        self.needs_redraw = true;
    }

    /// KB-022: a temporary Object grab from a drawing tool — run the
    /// Object hit-test, and when something is under the pen, arm its
    /// drag WITHOUT changing tools (the drag handlers are
    /// tool-independent). Returns true when the press was consumed.
    pub(crate) fn temp_object_try(&mut self, x: f32, y: f32) -> bool {
        let (cx, cy) = self.viewport.to_canvas(x, y);
        // The gate used to be object_candidates_at's zero-tolerance
        // containment, but the hit tests below accept edges/handles
        // within ~10 screen px — a Ctrl+drag starting on the GUTTER side
        // of a frame border fell in the gap and drew ink over it. Run
        // the real hit machinery instead; when nothing arms, put back
        // every piece of selection state a miss clears (keeping the
        // standing selection is all the gate ever existed for).
        let snap = (
            self.object_sel,
            self.balloon_sel,
            self.gen_sel,
            self.text_sel,
            self.object_pick,
        );
        if !self.text_object_hit(cx, cy) {
            self.object_hit(cx, cy);
        }
        let armed = self.text_obj_drag.is_some()
            || self.object_drag.is_some()
            || self.balloon_obj_drag.is_some()
            || self.gen_drag.is_some();
        if armed {
            self.temp_object = true;
            self.set_status("object grab — release to keep drawing");
        } else {
            (
                self.object_sel,
                self.balloon_sel,
                self.gen_sel,
                self.text_sel,
                self.object_pick,
            ) = snap;
        }
        armed
    }

    pub fn canvas_move(&mut self, x: f32, y: f32, batch: &[PenSample]) {
        // Leak repair, virtual mode: the stroke streams into points and
        // nowhere else — the barrier mask is built from them on release.
        if let Some(r) = &mut self.fill_repair
            && r.virtual_barrier
            && !r.pts.is_empty()
        {
            let (cx, cy) = self.viewport.to_canvas(x, y);
            r.pts.push((cx, cy));
            self.needs_redraw = true;
            return;
        }
        if let Some(d) = self.liquify_drag.as_mut() {
            let (cx, cy) = self.viewport.to_canvas(x, y);
            let (dx, dy) = (cx - d.last.0, cy - d.last.1);
            d.last = (cx, cy);
            d.last_step = std::time::Instant::now();
            let li = self.doc.active;
            let (mode, radius, strength) =
                (self.liquify_mode, self.liquify_radius, self.liquify_strength);
            let invert = self.shell.sync_modifiers().alt;
            mn_core::liquify::step(
                &mut self.doc, li, mode, cx, cy, dx, dy, radius, strength, 0.0, invert,
            );
            self.needs_redraw = true;
            return;
        }
        if self.rotating() {
            self.update_rotate(x, y);
            return;
        }
        if self.panning() {
            self.update_pan(x, y);
            return;
        }
        // KB-020: the live size drag — horizontal screen travel scales the
        // px diameter the Tool Property slider shares.
        if let Some((x0, px0)) = self.size_drag {
            let px = (px0 * (1.0 + (x - x0) / 240.0))
                .clamp(crate::cmd::SIZE_PX_MIN, crate::cmd::SIZE_PX_MAX);
            self.push_cmd(AppCmd::SetBrushSizePx(px));
            self.set_status(format!("brush size: {px:.1} px"));
            self.needs_redraw = true;
            return;
        }
        if self.drawing() {
            // Row 156 / `FG-021`: once the hold has produced a figure the
            // pen ADJUSTS it, so its travel must not keep laying ink for a
            // stroke that is about to be taken back off the page.
            if self.smart_shape.as_ref().is_some_and(|g| g.armed()) {
                self.smart_shape_move(x, y);
                return;
            }
            self.push_batch(batch);
            self.doc.mask_stroke_to_selection();
            // Row 156: a Smart shape stroke records its path beside the ink
            // and restarts the hold whenever the hand moves on.
            self.smart_shape_move(x, y);
            return;
        }
        let (cx, cy) = self.viewport.to_canvas(x, y);
        if self
            .transform_drag
            .as_ref()
            .is_some_and(|d| d.gesture.is_some())
        {
            self.transform_move(cx, cy);
            self.needs_redraw = true;
            return;
        }
        if self.text_gesture.is_some() {
            self.text_tool_move(cx, cy);
            return;
        }
        if let Some(d) = &mut self.text_obj_drag {
            d.cur = (cx, cy);
            self.needs_redraw = true;
            return;
        }
        if let Some(d) = &mut self.group_drag {
            d.cur = (cx, cy);
            self.needs_redraw = true;
            return;
        }
        // A ruler move edits the ruler LIVE: the next snap reads the new
        // geometry with no invalidation step, because the geometry IS the
        // ruler. (The symmetric ruler's mirror twins are a derived cache —
        // they are rebuilt at release; no stroke can start mid-drag.)
        if let Some(m) = self.ruler_move.as_mut() {
            let d = [cx - m.last[0], cy - m.last[1]];
            m.last = [cx, cy];
            // M3 phase A: say what is moving, ONCE, as the drag starts —
            // the status line is not a per-sample readout.
            let starting = !m.moved && d[0].abs() + d[1].abs() > 0.0;
            m.moved |= d[0].abs() + d[1].abs() > 0.0;
            let (k, grab) = (m.ruler, m.grab);
            self.doc.rulers.move_by(k, grab, d);
            if starting
                && let mn_core::RulerGrab::Anchor(i) = grab
                && let Some(role) = self.ruler_anchor_role(k, i)
            {
                self.set_status(role.moving());
            }
            self.needs_redraw = true;
            return;
        }
        if self.vector_drag.is_some() {
            // Geometry moves live (the overlay draws it); the raster
            // re-derives once at release.
            self.vector_drag_move(cx, cy);
            return;
        }
        if let Some(d) = &mut self.gen_drag {
            d.cur = (cx, cy);
            self.needs_redraw = true;
            return;
        }
        // The dots re-rasterize only on release — the status line is the
        // live readout of where the lattice is going.
        let lattice_msg = if let Some(d) = &mut self.fill_lattice_drag {
            d.cur = (cx, cy);
            Some(format!(
                "lattice → ({:+.0}, {:+.0}) px",
                d.cur.0 - d.start.0,
                d.cur.1 - d.start.1
            ))
        } else {
            None
        };
        if let Some(msg) = lattice_msg {
            self.set_status(msg);
            self.needs_redraw = true;
            return;
        }
        if let Some(d) = &mut self.object_drag {
            d.shift_snap = self.shell.sync_modifiers().shift;
            // O-010: an edge drag SNAPS to other frames' edge lines (and
            // their extensions — any edge of the same orientation defines
            // an infinite line) when within 3 canvas px. Axis-aligned
            // edges only, the manga-grid case; the gutter stay-put
            // behaviour needs the drag to LAND on the neighbour, and a
            // 1.5 px on-segment carry at release is what makes the two
            // panels share the border afterwards. Panels separated by a
            // real gutter never touch, so they are the facing carry's job
            // ("Keep gutters aligned", canvas_up), not this snap's.
            let mut cur = (cx, cy);
            if let ObjectDragMode::Edge(i) = d.mode {
                let n = d.orig.points.len();
                let (a, b) = (d.orig.points[i], d.orig.points[(i + 1) % n]);
                const TOL: f32 = 3.0;
                if (a[0] - b[0]).abs() < 0.5 {
                    // Vertical edge: snap its x onto other vertical lines.
                    let now_x = a[0] + (cx - d.start.0);
                    let mut best: Option<f32> = None;
                    for l in self.doc.layers.iter() {
                        let Some(fs) = l.frames() else { continue };
                        for fr in &fs.frames {
                            for k in 0..fr.points.len() {
                                let p = fr.points[k];
                                let q = fr.points[(k + 1) % fr.points.len()];
                                if (p[0] - q[0]).abs() < 0.5
                                    && ((now_x - p[0]).abs() <= TOL)
                                    && best
                                        .is_none_or(|bx| (now_x - p[0]).abs() < (now_x - bx).abs())
                                    && !(p == a && q == b)
                                {
                                    best = Some(p[0]);
                                }
                            }
                        }
                    }
                    if let Some(bx) = best {
                        cur.0 = d.start.0 + (bx - a[0]);
                    }
                } else if (a[1] - b[1]).abs() < 0.5 {
                    let now_y = a[1] + (cy - d.start.1);
                    let mut best: Option<f32> = None;
                    for l in self.doc.layers.iter() {
                        let Some(fs) = l.frames() else { continue };
                        for fr in &fs.frames {
                            for k in 0..fr.points.len() {
                                let p = fr.points[k];
                                let q = fr.points[(k + 1) % fr.points.len()];
                                if (p[1] - q[1]).abs() < 0.5
                                    && ((now_y - p[1]).abs() <= TOL)
                                    && best
                                        .is_none_or(|by| (now_y - p[1]).abs() < (now_y - by).abs())
                                    && !(p == a && q == b)
                                {
                                    best = Some(p[1]);
                                }
                            }
                        }
                    }
                    if let Some(by) = best {
                        cur.1 = d.start.1 + (by - a[1]);
                    }
                }
            }
            d.cur = cur;
            self.needs_redraw = true;
            return;
        }
        if let Some(d) = &mut self.balloon_obj_drag {
            d.shift_snap = self.shell.sync_modifiers().shift;
            d.cur = (cx, cy);
            self.needs_redraw = true;
            return;
        }
        if let Some(pts) = &mut self.frame_pen {
            pts.push((cx, cy));
            self.needs_redraw = true;
            return;
        }
        if self.frame_poly.is_some() {
            // Rubber-band preview follows the pointer (overlay reads
            // last_pointer); nothing to record.
            self.needs_redraw = true;
        }
        // `FG-014`: a Ctrl+drag has hold of one anchor of the in-progress
        // click list. The point follows the pointer LIVE, which is the whole
        // row — the preview (chords for Polygon, the spline for Curve) is
        // rebuilt from the list every frame, so what redraws is the shape as
        // it will ink and not a ghost of where the anchor used to be.
        if self.figure_anchor_drag.is_some() {
            self.figure_anchor_drag_move(cx, cy);
            return;
        }
        if let Some((a, cur)) = &mut self.figure_drag {
            *cur = (cx, cy);
            // Shift constrains line/rect/ellipse to 45° steps (CSP).
            let shift = self.shell.sync_modifiers().shift;
            if shift {
                let dx = cx - a.0;
                let dy = cy - a.1;
                let ang = dy.atan2(dx);
                let oct = (ang / std::f32::consts::FRAC_PI_4).round() * std::f32::consts::FRAC_PI_4;
                let len = (dx * dx + dy * dy).sqrt();
                *cur = (a.0 + oct.cos() * len, a.1 + oct.sin() * len);
            }
            self.needs_redraw = true;
            return;
        }
        if let Some((_, cur)) = &mut self.grad_drag {
            *cur = (cx, cy);
            self.needs_redraw = true;
            return;
        }
        if let Some(g) = &mut self.grad_free {
            if g.drawing {
                // Thin the trail as it comes in: the per-tile segment cull
                // that makes a full-page apply affordable is only as good as
                // the guide is short, and a raw pointer batch at 400% zoom
                // is hundreds of sub-pixel steps. 1 canvas px is well under
                // what the distance field can resolve.
                let far = g
                    .cur
                    .last()
                    .is_none_or(|p| (p[0] - cx).abs() + (p[1] - cy).abs() >= 1.0);
                if far {
                    g.cur.push([cx, cy]);
                }
            }
            self.needs_redraw = true;
            return;
        }
        if let Some((a, cur)) = &mut self.frame_drag {
            // The rectangle sub tool wants the raw corner; cuts axis-snap.
            *cur = if self.frame_mode == FrameMode::Rect {
                (cx, cy)
            } else {
                Self::snap_axis(*a, (cx, cy))
            };
            self.needs_redraw = true;
            return;
        }
        if let Some(pts) = &mut self.balloon_drag {
            if self.balloon_mode == BalloonMode::Draw {
                // The full pen history (dense WM_POINTER batches), pressure
                // riding along — the balloon's anchors become a smooth
                // pressure-aware spline on release.
                for s in batch {
                    let (sx, sy) = self.viewport.to_canvas(s.x, s.y);
                    pts.push([sx, sy, s.pressure]);
                }
                if batch.is_empty() {
                    let (cx, cy) = self.viewport.to_canvas(x, y);
                    pts.push([cx, cy, 0.5]);
                }
            } else {
                let (cx, cy) = self.viewport.to_canvas(x, y);
                let pr = batch.last().map(|s| s.pressure).unwrap_or(0.5);
                pts.truncate(1);
                pts.push([cx, cy, pr]);
            }
            self.needs_redraw = true;
            return;
        }
        if let Some(pts) = &mut self.fill_drag {
            pts.push((cx, cy));
            self.needs_redraw = true;
            return;
        }
        if self.magnetic.is_some() {
            self.magnetic_track(cx, cy);
            return;
        }
        if let Some((_, cur)) = &mut self.select_moving {
            *cur = (cx, cy);
            self.needs_redraw = true;
        } else if let Some(pts) = &mut self.select_drag {
            match self.select_mode {
                // Both corner-drag shapes keep exactly the press and the
                // live corner; the ellipse is derived from the same pair.
                SelectMode::Rect | SelectMode::Ellipse => {
                    pts.truncate(1);
                    pts.push((cx, cy));
                }
                SelectMode::Lasso | SelectMode::Shrink => pts.push((cx, cy)),
                // Magnetic never uses `select_drag` — its trace lives in
                // `self.magnetic` and was handled by the early return above.
                SelectMode::Magnetic => {}
            }
            self.needs_redraw = true;
        }
    }

    pub fn canvas_up(&mut self, x: f32, y: f32, batch: &[PenSample]) {
        // Leak repair, virtual mode: the release IS the gesture — build
        // the barrier from the captured stroke and re-run the fill. A
        // bare tap is not a barrier; the arm stands, waiting for a drag.
        if let Some(r) = self.fill_repair.take() {
            if r.virtual_barrier {
                if r.pts.len() >= 2 {
                    let mut r = r;
                    let (cx, cy) = self.viewport.to_canvas(x, y);
                    r.pts.push((cx, cy));
                    self.finish_fill_repair(r);
                } else {
                    self.fill_repair = Some(r);
                    self.set_status("fill repair: drag the closing stroke, not a tap");
                }
                self.needs_redraw = true;
                return;
            }
            self.fill_repair = Some(r); // real-ink mode: the stroke below runs first
        }
        // KB-020: the size drag ends with its last readout standing.
        if self.size_drag.take().is_some() {
            self.set_status(format!("brush size: {:.1} px", self.brush_radius() * 2.0));
            return;
        }
        // KB-022: the temporary grab releases; the drag finishers below
        // run through their tool-independent paths.
        self.temp_object = false;
        // TODO #3: complete an armed ruler creation.
        if let (Some(kind), Some(a)) = (self.ruler_pending.take(), self.ruler_drag.take()) {
            let (cx, cy) = self.viewport.to_canvas(x, y);
            let mut b = [cx, cy];
            // CSP: "Hold Shift while dragging to draw a straight line ruler
            // in 45 degree increments." The same octant snap the Figure
            // tool's drag uses, applied where every creation drag reads its
            // end point — a 流線 block wants its parallel ruler dead
            // horizontal (or on the 45) far more often than eyeballed.
            // Length is preserved, so a concentric drag's ring spacing is
            // untouched and the guides, which ignore `b`, are unaffected.
            if self.shell.sync_modifiers().shift {
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let len = dx.hypot(dy);
                if len > 0.0 {
                    let oct = (dy.atan2(dx) / std::f32::consts::FRAC_PI_4).round()
                        * std::f32::consts::FRAC_PI_4;
                    b = [a[0] + oct.cos() * len, a[1] + oct.sin() * len];
                }
            }
            let ruler = match kind {
                RulerKind::Line => {
                    if (b[0] - a[0]).abs() + (b[1] - a[1]).abs() < 2.0 {
                        self.set_status("drag further to draw a line ruler");
                        return;
                    }
                    mn_core::Ruler::Line { a, b }
                }
                RulerKind::VanishingPoint => mn_core::Ruler::VanishingPoint {
                    c: a,
                    rays: 12,
                    angle0: (b[1] - a[1]).atan2(b[0] - a[0]),
                },
                // Part 4: the drag IS the eye level — its two ends become
                // the horizon VPs.
                RulerKind::Perspective => {
                    if (b[0] - a[0]).abs() + (b[1] - a[1]).abs() < 32.0 {
                        self.set_status("drag the horizon — both ends become vanishing points");
                        return;
                    }
                    mn_core::Ruler::Perspective { a, b }
                }
                // One-point: the drag STARTS at the single vanishing point
                // and runs along the eye level, so the same gesture places
                // the VP and sets the horizon's tilt.
                RulerKind::Perspective1 => {
                    if (b[0] - a[0]).abs() + (b[1] - a[1]).abs() < 32.0 {
                        self.set_status("drag from the vanishing point along the eye level");
                        return;
                    }
                    mn_core::Ruler::Perspective1 { vp: a, h: b }
                }
                // Three-point: the 2-point gesture plus a third VP dropped
                // on the perpendicular through the horizon's middle, on
                // the side the drag pointed to (left→right = below the
                // horizon, a high angle). It is an anchor, so the Object
                // tool drags it to where the shot actually wants it.
                RulerKind::Perspective3 => {
                    let d = [b[0] - a[0], b[1] - a[1]];
                    let n = (d[0] * d[0] + d[1] * d[1]).sqrt();
                    if n < 32.0 {
                        self.set_status("drag the horizon — both ends become vanishing points");
                        return;
                    }
                    let perp = [-d[1] / n, d[0] / n];
                    let mid = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
                    let reach = n * 0.75;
                    mn_core::Ruler::Perspective3 {
                        a,
                        b,
                        z: [mid[0] + perp[0] * reach, mid[1] + perp[1] * reach],
                    }
                }
                // Curve creation never reaches the drag path (it collects
                // clicks in canvas_down); reaching here means a stray
                // state — drop it harmlessly.
                RulerKind::Curve => {
                    self.set_status("curve ruler: click vertices on the canvas");
                    return;
                }
                RulerKind::Parallel => {
                    if (b[0] - a[0]).abs() + (b[1] - a[1]).abs() < 2.0 {
                        self.set_status("drag further to set the direction");
                        return;
                    }
                    mn_core::Ruler::Parallel { a, b }
                }
                // CSP: "Tap on the canvas to create a Radial line ruler."
                // The press IS the centre; the drag (if any) is free,
                // because a continuum fan has no angle to aim.
                RulerKind::Radial => mn_core::Ruler::Radial { c: a },
                // A CLICK leaves the ring spacing free (CSP's concentric
                // circle ruler: the stroke keeps its own radius). Drag and
                // the drag length becomes the ladder's spacing — ours, and
                // worth keeping for evenly spaced panel furniture.
                RulerKind::Concentric => {
                    let d = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
                    let dr = if d < 8.0 { 0.0 } else { d };
                    mn_core::Ruler::Concentric { c: a, dr }
                }
                RulerKind::Symmetric => {
                    if (b[0] - a[0]).abs() + (b[1] - a[1]).abs() < 2.0 {
                        self.set_status("drag outward from the symmetry centre");
                        return;
                    }
                    mn_core::Ruler::Symmetric {
                        c: a,
                        lines: self.symmetric_lines,
                        angle0: (b[1] - a[1]).atan2(b[0] - a[0]),
                    }
                }
                // Guides sit at the PRESS coordinate (the drag is free).
                RulerKind::GuideH => mn_core::Ruler::Guide {
                    horizontal: true,
                    pos: a[1],
                },
                RulerKind::GuideV => mn_core::Ruler::Guide {
                    horizontal: false,
                    pos: a[0],
                },
            };
            let symmetric = matches!(ruler, mn_core::Ruler::Symmetric { .. });
            // One creation drag = one undo step. The snapshot is taken HERE
            // and not at the press because every arm above can bail out
            // (too short a drag, wrong kind) without creating anything.
            let before = self.doc.rulers.clone();
            self.doc.rulers.items.push(ruler);
            self.doc.rulers.fix_len();
            self.doc.rulers.on = true;
            self.doc.record_rulers(before, "Add ruler");
            if symmetric {
                // The mirror twins exist only while a symmetric ruler is
                // live — rebuild replaces the checkbox twins with the
                // dihedral set.
                self.rebuild_twins();
                self.set_status(format!(
                    "symmetric ruler — {} lines, mirroring ON",
                    self.symmetric_lines
                ));
            } else {
                // The two continuum rulers say WHAT they will do, because
                // nothing on the canvas distinguishes a free ring ruler
                // from a laddered one until a stroke lands.
                self.set_status(match ruler {
                    mn_core::Ruler::Radial { .. } => {
                        "radial line ruler — every stroke runs through the centre".to_string()
                    }
                    mn_core::Ruler::Concentric { dr, .. } if dr <= 0.0 => {
                        "concentric ruler — free radius: each stroke keeps the one it starts on"
                            .to_string()
                    }
                    mn_core::Ruler::Concentric { dr, .. } => {
                        format!("concentric ruler — rings every {dr:.0} px")
                    }
                    _ => "ruler created — snapping on".to_string(),
                });
            }
            self.needs_redraw = true;
            return;
        }
        // A completed ruler move. The geometry was updated step by step, so
        // release only ends the gesture — plus the one thing that is a
        // CACHE of a ruler and not the ruler: the symmetric ruler's mirror
        // twins carry its centre/axes, so a moved symmetric ruler must
        // rebuild them or the next stroke mirrors about the old centre.
        if let Some(m) = self.ruler_move.take() {
            if matches!(
                self.doc.rulers.items.get(m.ruler),
                Some(mn_core::Ruler::Symmetric { .. })
            ) {
                self.rebuild_twins();
            }
            // The drag applied its deltas live; the UNDO step is the whole
            // gesture, pushed once here against the snapshot taken at the
            // grab. A press that only grabbed changes nothing and records
            // nothing (`record_rulers` compares).
            self.doc.record_rulers(m.before, "Move ruler");
            if m.moved {
                self.set_status("ruler moved");
            }
            self.needs_redraw = true;
            return;
        }
        if self.vector_drag.is_some() {
            self.vector_drag_release();
            return;
        }
        if self.rotating() {
            self.end_rotate();
            return;
        }
        if self.panning() {
            self.end_pan();
            return;
        }
        if self.drawing() {
            // Row 156 / `FG-021`: an armed gesture takes the lift-off point
            // as the last frame of the ADJUSTMENT, not as more ink.
            if self.smart_shape.as_ref().is_some_and(|g| g.armed()) {
                self.smart_shape_move(x, y);
            } else {
                self.push_batch(batch);
                // Row 156: fold the lift-off point in and give the hold one
                // last chance to mature (a hold that ripened between the
                // final frame and the release still counts), THEN close the
                // stroke — the swap needs a finished undo group to take back.
                if let Some(g) = &mut self.smart_shape {
                    let (cx, cy) = self.viewport.to_canvas(x, y);
                    g.pts.push([cx, cy]);
                }
                self.smart_shape_tick();
            }
            self.end_stroke();
            self.finish_smart_shape();
            // Leak repair, real-ink mode: the stroke just committed as its
            // own op — re-run the fill now and wrap the pair into one
            // undo press (the leak itself was undone at arm time).
            if let Some(r) = self.fill_repair.take() {
                self.finish_fill_repair(r);
            }
            return;
        }
        let (cx, cy) = self.viewport.to_canvas(x, y);
        if self
            .transform_drag
            .as_ref()
            .is_some_and(|d| d.gesture.is_some())
        {
            // Fold the final motion in, then release the gesture — the
            // float itself stays until Enter/Esc. ONE exception, the
            // CSP-parity rule (owner 2026-08-25): the OBJECT tool's ink
            // grab commits on release when all that happened was a MOVE —
            // CSP's Object tool drags layers directly, no Enter. A grab
            // that scaled or rotated keeps the float. (Evaluated AFTER
            // the fold: the release delta is part of the move.)
            self.transform_move(cx, cy);
            let commit_now = self.transform_drag.as_ref().is_some_and(|d| {
                d.object_lift
                    && d.xform.m == mn_core::Affine2::IDENTITY.m
                    && (d.xform.t[0] != 0.0 || d.xform.t[1] != 0.0)
            });
            if let Some(d) = &mut self.transform_drag {
                d.gesture = None;
            }
            if commit_now {
                self.push_cmd(AppCmd::TransformCommit);
            }
            self.needs_redraw = true;
            return;
        }
        if self.text_gesture.is_some() {
            self.text_tool_up(cx, cy);
            return;
        }
        if self.text_obj_drag.is_some() {
            self.finish_text_obj_drag(cx, cy);
            return;
        }
        if let Some(d) = self.group_drag.take() {
            let (dx, dy) = d.delta();
            if dx != 0 || dy != 0 {
                self.push_cmd(AppCmd::ObjectMultiMove { dx, dy });
            }
            self.needs_redraw = true;
            return;
        }
        if let Some(d) = self.gen_drag.take() {
            let spec = gen_drag_spec(&d, self.doc.size);
            let li = d.layer;
            let mode = d.mode;
            let moved = d.cur != d.start;
            if moved
                && self
                    .doc
                    .layers
                    .get(li)
                    .is_some_and(|l| l.genlines.is_some())
            {
                // The spec is stored only if the regen SUCCEEDS (audit F,
                // 2026-08-19): on failure neither the raster nor the
                // parameters move, keeping the two in agreement.
                if self.doc.regen_genlines(li, spec) {
                    self.set_status(match mode {
                        GenDragMode::Center => "run moved",
                        GenDragMode::RIn => "inner radius set",
                        GenDragMode::ROut => "outer radius set",
                        GenDragMode::Angle => "direction set",
                        GenDragMode::LenMin => "minimum length set",
                        GenDragMode::LenMax => "maximum length set",
                    });
                } else {
                    self.set_status("generator produced nothing — parameters widened");
                }
            }
            self.mark_dirty();
            return;
        }
        if let Some(mut d) = self.object_drag.take() {
            d.cur = (cx, cy);
            if d.moved() {
                let f = d.preview();
                // Concave panels are legal since the polyline/pen frames;
                // only self-intersections and slivers are refused.
                let valid =
                    f.area() >= mn_core::frame::MIN_FRAME_AREA && (f.is_convex() || f.is_simple());
                let fs = self.doc.layers.get(d.layer).and_then(|l| l.frames());
                if let (true, Some(fs)) = (valid, fs) {
                    let mut fs = fs.clone();
                    if d.frame < fs.frames.len() {
                        // Shared gutter, O-011 page-wide: an edge drag carries
                        // along every vertex that sat on the ORIGINAL edge —
                        // same folder AND sibling frame folders (a divide
                        // makes each panel its own folder; the gutter between
                        // panels must move as ONE border). T-junction
                        // vertices follow too. The carry is ONE gesture: if
                        // any carried frame (this folder's siblings or
                        // another folder's) would break, the whole commit is
                        // dropped — a silent squash here is the geometry the
                        // reading-order stack overflow crashes on (audit
                        // C/D, 2026-08-19).
                        let mut gutter_touched: Vec<(usize, mn_core::FrameSet)> = Vec::new();
                        let mut carried_breaks = false;
                        let carried_frame_ok = |f: &mn_core::Frame| {
                            f.area() >= mn_core::frame::MIN_FRAME_AREA
                                && (f.is_convex() || f.is_simple())
                        };
                        if let ObjectDragMode::Edge(i) = d.mode {
                            let n = d.orig.points.len();
                            let (a, b) = (d.orig.points[i], d.orig.points[(i + 1) % n]);
                            let (na, _nb) = (f.points[i], f.points[(i + 1) % n]);
                            let (ddx, ddy) = (na[0] - a[0], na[1] - a[1]);
                            if ddx != 0.0 || ddy != 0.0 {
                                let abx = b[0] - a[0];
                                let aby = b[1] - a[1];
                                let len2 = abx * abx + aby * aby;
                                let t_eps = if len2 > 1e-6 { 1.5 / len2.sqrt() } else { 0.0 };
                                let on_seg_t = |p: [f32; 2]| -> Option<f32> {
                                    if len2 < 1e-6 {
                                        return None;
                                    }
                                    let t = ((p[0] - a[0]) * abx + (p[1] - a[1]) * aby) / len2;
                                    if !(0.0..=1.0).contains(&t) {
                                        return None;
                                    }
                                    let px = a[0] + abx * t - p[0];
                                    let py = a[1] + aby * t - p[1];
                                    (px * px + py * py < 2.25).then_some(t) // 1.5 px
                                };
                                // Carry a neighbour's on-edge vertices — but
                                // a frame whose ONLY hit sits at an ENDPOINT
                                // of the dragged edge is a corner that merely
                                // touches the edge's end (the panel diagonal
                                // to this one), not a shared border or a
                                // T-junction: dragging one of its four
                                // corners sheared it into a trapezoid, and
                                // carried_frame_ok (area + simplicity) let
                                // the shear commit. A genuine shared border
                                // hits with BOTH its corners; a T-junction
                                // hits once at interior t. Both still carry.
                                // CSP "Keep gutters aligned = All" (audit
                                // P0-4). The coincident carry below only
                                // reaches vertices ON the dragged edge, so
                                // the most-used paneling gesture — nudge a
                                // border, keep the gutter — silently closed
                                // the 3–10 mm gap instead. The facing carry
                                // moves the border ACROSS the gutter by the
                                // same delta. Only the NEAREST facing rank
                                // travels (a gutter is shared by however
                                // many panels line its far side, but not by
                                // the row behind them), and only when that
                                // gap is plausibly a gutter: twice the
                                // widest cut-tool gutter pref, in px.
                                let facing = self.gutter_align_all.then(|| {
                                    let e = [b[0] - a[0], b[1] - a[1]];
                                    let len = (e[0] * e[0] + e[1] * e[1]).sqrt();
                                    let mut nrm = [e[1] / len, -e[0] / len];
                                    let c = d.orig.centroid();
                                    if (a[0] - c[0]) * nrm[0] + (a[1] - c[1]) * nrm[1] < 0.0 {
                                        nrm = [-nrm[0], -nrm[1]];
                                    }
                                    nrm
                                });
                                let (gl, gt) = self.gutter_folder_mm;
                                let (bl, bt) = self.gutter_border_mm;
                                let reach =
                                    2.0 * self.mm_to_px(gl.max(gt).max(bl).max(bt)).max(1.0);
                                // The nearest facing gap on the whole page,
                                // measured before anything moves.
                                let mut nearest = f32::INFINITY;
                                if let Some(nrm) = facing {
                                    let mut scan =
                                        |set: &mn_core::FrameSet, skip: Option<usize>| {
                                            for (si, sib) in set.frames.iter().enumerate() {
                                                if Some(si) == skip {
                                                    continue;
                                                }
                                                if let Some((dist, _)) =
                                                    mn_core::frame::facing_vertices(sib, a, b, nrm)
                                                    && dist > 1.5
                                                {
                                                    nearest = nearest.min(dist);
                                                }
                                            }
                                        };
                                    scan(&fs, Some(d.frame));
                                    for (li, l) in self.doc.layers.iter().enumerate() {
                                        if li != d.layer
                                            && let Some(other) = l.frames()
                                        {
                                            scan(other, None);
                                        }
                                    }
                                }
                                // A panel is on this gutter when it faces the
                                // edge at the nearest gap (1.5 px of slop, the
                                // same the coincident carry uses); the page
                                // edge, or anything further than a gutter,
                                // means a plain resize.
                                let carry_facing = move |sib: &mut mn_core::Frame| -> bool {
                                    let Some(nrm) = facing else { return false };
                                    if !(nearest.is_finite() && nearest <= reach) {
                                        return false;
                                    }
                                    let Some((dist, idx)) =
                                        mn_core::frame::facing_vertices(sib, a, b, nrm)
                                    else {
                                        return false;
                                    };
                                    if dist > nearest + 1.5 {
                                        return false;
                                    }
                                    for k in idx {
                                        sib.points[k][0] += ddx;
                                        sib.points[k][1] += ddy;
                                    }
                                    true
                                };
                                let carry = |sib: &mut mn_core::Frame| -> bool {
                                    let hits: Vec<(usize, f32)> = sib
                                        .points
                                        .iter()
                                        .enumerate()
                                        .filter_map(|(k, p)| on_seg_t(*p).map(|t| (k, t)))
                                        .collect();
                                    if hits.len() == 1
                                        && (hits[0].1 <= t_eps || hits[0].1 >= 1.0 - t_eps)
                                    {
                                        return false;
                                    }
                                    for (k, _) in &hits {
                                        sib.points[*k][0] += ddx;
                                        sib.points[*k][1] += ddy;
                                    }
                                    !hits.is_empty()
                                };
                                for (si, sib) in fs.frames.iter_mut().enumerate() {
                                    if si == d.frame {
                                        continue;
                                    }
                                    // Coincident first: a neighbour that
                                    // SHARES the border is not across a
                                    // gutter from it, and must not move twice.
                                    let moved = carry(sib) || carry_facing(sib);
                                    if moved && !carried_frame_ok(sib) {
                                        carried_breaks = true;
                                    }
                                }
                                // The other frame folders on the page. Each
                                // commits its own FrameSet — one undo group
                                // per folder, the dual-step convention.
                                for (li, l) in self.doc.layers.iter().enumerate() {
                                    if li == d.layer {
                                        continue;
                                    }
                                    let Some(other) = l.frames() else {
                                        continue;
                                    };
                                    let mut other = other.clone();
                                    let mut changed = false;
                                    for sib in other.frames.iter_mut() {
                                        if carry(sib) || carry_facing(sib) {
                                            changed = true;
                                            if !carried_frame_ok(sib) {
                                                carried_breaks = true;
                                            }
                                        }
                                    }
                                    if changed {
                                        gutter_touched.push((li, other));
                                    }
                                }
                                if carried_breaks {
                                    self.set_status(
                                        "that edit would break a neighbouring panel — reverted",
                                    );
                                    self.needs_redraw = true;
                                    return;
                                }
                            }
                        }
                        // A folder-header drag moves the folder's pixels with
                        // the panel (CSP). One Tiles undo group PER CHILD (+
                        // a Mask group when a linked mask rode along) + the
                        // Frames group for the geometry — N+1 undo steps
                        // until op-group merging lands. Per-child on purpose:
                        // a single begin_op() armed only whichever layer was
                        // ACTIVE, so undo deleted that child's art and
                        // stranded every sibling (GLM-audit survivor #1).
                        if matches!(d.mode, ObjectDragMode::MoveWhole) {
                            let dx = (f.points[0][0] - d.orig.points[0][0]).round() as i32;
                            let dy = (f.points[0][1] - d.orig.points[0][1]).round() as i32;
                            if dx != 0 || dy != 0 {
                                for k in self.doc.children_range(d.layer) {
                                    let mask_before = self.doc.layers[k].mask.clone();
                                    let rev0 = mask_before.as_ref().map(|m| m.revision);
                                    self.doc.begin_op_on(k);
                                    self.doc.set_op_label("Move panel");
                                    self.doc.layers[k].translate_content(dx, dy);
                                    self.doc.end_op();
                                    let rev1 = self.doc.layers[k].mask.as_ref().map(|m| m.revision);
                                    if rev1 != rev0 {
                                        self.doc.record_mask_change(k, mask_before, "Move panel");
                                    }
                                }
                            }
                        }
                        fs.frames[d.frame] = f;
                        self.push_cmd(AppCmd::FrameCommit {
                            layer: d.layer,
                            frames: fs,
                        });
                        for (li, frames) in gutter_touched {
                            self.push_cmd(AppCmd::FrameCommit { layer: li, frames });
                        }
                    }
                } else {
                    self.set_status("that edit would break the panel shape — reverted");
                }
            }
            self.needs_redraw = true;
            return;
        }
        if let Some(d) = self.fill_lattice_drag.take() {
            let (dx, dy) = (d.cur.0 - d.start.0, d.cur.1 - d.start.1);
            if dx.abs() + dy.abs() >= 1.0 {
                if let Some(mn_core::LayerKind::Fill(mn_core::fill_layer::FillKind::Tone {
                    tone,
                    density,
                })) = self.doc.layers.get(d.layer).map(|l| l.kind.clone())
                {
                    let mut tone = tone;
                    tone.offset = [tone.offset[0] + dx, tone.offset[1] + dy];
                    self.push_cmd(AppCmd::SetFillParams(
                        d.layer,
                        mn_core::fill_layer::FillKind::Tone { tone, density },
                    ));
                    self.set_status("tone lattice moved — the dots slid under the art");
                }
            }
            self.needs_redraw = true;
            return;
        }
        if self.liquify_drag.take().is_some() {
            self.doc.end_op();
            self.set_status("liquified — one undo takes the whole gesture back");
            self.needs_redraw = true;
            return;
        }
        if let Some(mut d) = self.balloon_obj_drag.take() {
            d.cur = (cx, cy);
            if d.moved() {
                let b = d.preview();
                let bs = self.doc.layers.get(d.layer).and_then(|l| l.balloons());
                if let (true, Some(bs)) = (b.is_valid(), bs) {
                    let mut bs = bs.clone();
                    if d.balloon < bs.balloons.len() {
                        // Read off the committed shape BEFORE the store
                        // moves it — the move-carry branch wants it.
                        let b1 = b.bbox();
                        bs.balloons[d.balloon] = b;
                        self.push_cmd(AppCmd::BalloonCommit {
                            layer: d.layer,
                            balloons: bs,
                        });
                        // TRIAGE 134 + the carry: turning MOVES, MOVING
                        // translates, RESIZING re-fractions — and whatever
                        // lettering came along is bundled with the
                        // balloon's own commit into ONE undo step (audit
                        // small, 2026-08-25: the status used to promise
                        // "two steps"; now it's true that it's one).
                        let carried_layers = if let Some((pivot, rad)) = d.rotation() {
                            self.carry_texts_with_balloon(&d.orig, pivot, rad)
                        } else if let BalloonDragMode::MoveWhole = d.mode {
                            // The delta is read off the committed shapes,
                            // so a Shift-constrained move carries the same
                            // constrained distance it showed.
                            let b0 = d.orig.bbox();
                            self.translate_texts_with_balloon(
                                &d.orig,
                                [
                                    (b1[0] + b1[2]) * 0.5 - (b0[0] + b0[2]) * 0.5,
                                    (b1[1] + b1[3]) * 0.5 - (b0[1] + b0[3]) * 0.5,
                                ],
                            )
                        } else if matches!(
                            d.mode,
                            BalloonDragMode::Handle(_)
                                | BalloonDragMode::BoxCorner(_)
                                | BalloonDragMode::BoxEdge(_)
                        ) {
                            self.scale_texts_with_balloon(&d.orig, b1)
                        } else {
                            0
                        };
                        if carried_layers > 0 {
                            self.push_cmd(AppCmd::HistoryWrapLast {
                                label: "Balloon".into(),
                                count: 1 + carried_layers,
                            });
                        }
                    }
                } else {
                    self.set_status("that edit would collapse the balloon — reverted");
                }
            }
            self.needs_redraw = true;
            return;
        }
        // `FG-014`: the anchor drag ends where the pointer let go. Nothing is
        // committed and nothing is undone — the list is still a list, and the
        // single undo press is still waiting at the commit.
        if self.figure_anchor_drag.is_some() {
            self.figure_anchor_drag_move(cx, cy);
            self.figure_anchor_drag = None;
            self.set_status("point moved — keep clicking, or Enter to ink");
            self.needs_redraw = true;
            return;
        }
        if let Some((a, b)) = self.figure_drag.take() {
            self.release_figure_drag(a, b);
            self.needs_redraw = true;
            return;
        }
        if let Some((a, b)) = self.grad_drag.take() {
            self.finish_gradient(a, b);
            self.needs_redraw = true;
            return;
        }
        if self.grad_free.as_ref().is_some_and(|g| g.drawing) {
            self.release_grad_free(cx, cy);
            self.needs_redraw = true;
            return;
        }
        if let Some(mut pts) = self.balloon_drag.take() {
            let pr = batch.last().map(|s| s.pressure).unwrap_or(0.5);
            pts.push([cx, cy, pr]);
            self.finish_balloon_drag(pts);
            self.needs_redraw = true;
            return;
        }
        if let Some(mut pts) = self.frame_pen.take() {
            pts.push((cx, cy));
            // Same simplification as the balloon pen: ~2 screen px epsilon.
            let eps = (2.0 / self.viewport.zoom.max(0.01)).max(1.0);
            let raw: Vec<[f32; 2]> = pts.iter().map(|p| [p.0, p.1]).collect();
            let mut simple = mn_core::balloon::simplify_polyline(&raw, eps);
            if simple.len() >= 2 {
                let (f, l) = (simple[0], simple[simple.len() - 1]);
                if (f[0] - l[0]).abs() + (f[1] - l[1]).abs() < eps * 2.0 {
                    simple.pop();
                }
            }
            self.push_cmd(AppCmd::FramePoly { points: simple });
            self.needs_redraw = true;
            return;
        }
        if let Some((a, _)) = self.frame_drag.take() {
            if self.frame_mode == FrameMode::Rect {
                self.push_cmd(AppCmd::FrameRect { a, b: (cx, cy) });
            } else {
                let b = Self::snap_axis(a, (cx, cy));
                if (b.0 - a.0).abs() + (b.1 - a.1).abs() > 4.0 {
                    self.push_cmd(AppCmd::FrameDivide { a, b });
                } else {
                    // TRIAGE 129 / FB-030: a TAP, not a drag. CSP hangs this
                    // on a triangle handle; a tap on the edge itself needs no
                    // handle to find and no selection first.
                    self.push_cmd(AppCmd::FrameExtendEdge { at: a });
                }
            }
            self.needs_redraw = true;
            return;
        }
        // L-001: the pen lifting is itself an anchor, so whatever comes next
        // (another trace, a click, Enter) starts from a point the user chose
        // rather than from wherever the last auto-anchor happened to land.
        // The trace stays OPEN — only Enter, a click on the first anchor or
        // Esc ends it.
        if self.magnetic.is_some() {
            let at = (cx.round() as i32, cy.round() as i32);
            if let Some(l) = self.magnetic.as_mut()
                && l.last_anchor() != at
            {
                l.anchor(&self.doc, at);
            }
            self.needs_redraw = true;
            return;
        }
        if let Some((start, _)) = self.select_moving.take() {
            let (dx, dy) = ((cx - start.0).round() as i32, (cy - start.1).round() as i32);
            // SE-039 (owner spec): the drag moves THE MARCHING ANTS — the
            // selection itself, pixels untouched. Moving the CONTENTS is
            // the launcher's Move/Transform action (a different op).
            if (dx, dy) != (0, 0)
                && let Some(sel) = self.doc.selection.as_mut()
            {
                sel.translate(dx, dy);
                self.doc.touch();
            }
            self.needs_redraw = true;
            return;
        }
        if let Some(mut pts) = self.fill_drag.take() {
            // FI-003 / FI-004 / FI-005: all three sub tools are one freehand
            // path. The command arms own the geometry (pockets vs. the shape
            // itself vs. the empty pockets only) and the status line; this
            // arm only decides "was that a drag".
            pts.push((cx, cy));
            if pts.len() < 3 {
                self.set_status(match self.fill_mode {
                    FillMode::Lasso => "drag the shape to fill",
                    FillMode::Leftover => "scrub across the area with holes in it",
                    FillMode::Dust => "drag around the patch to clean",
                    _ => "drag right around the areas to fill",
                });
            } else {
                match self.fill_mode {
                    FillMode::Lasso => self.push_cmd(AppCmd::LassoFill { pts }),
                    FillMode::Leftover => self.push_cmd(AppCmd::LeftoverFill { pts }),
                    // Row 160: the path CLOSES into the window, so unlike
                    // its three siblings this one is a polygon, not seeds.
                    FillMode::Dust => self.push_cmd(AppCmd::DustScrub { pts }),
                    _ => self.push_cmd(AppCmd::EncloseFill { pts }),
                }
            }
            self.needs_redraw = true;
            return;
        }
        if let Some(mut pts) = self.select_drag.take() {
            // SE-020: the shrink drag is a WAND-family gesture — the path
            // seeds a union of floods; the command arm owns the combine
            // (and the pockets status), so this arm hands it off and is done.
            if self.select_mode == SelectMode::Shrink {
                if pts.len() >= 3 {
                    pts.push((cx, cy));
                    let m = self.shell.sync_modifiers();
                    let op = crate::cmd::effective_sel_op(m.shift, m.alt, self.sel_op);
                    self.push_cmd(AppCmd::MagicSelectPath { pts, op });
                } else {
                    self.set_status("drag across the empty space inside the drawing");
                }
                self.needs_redraw = true;
                return;
            }
            let sel = match self.select_mode {
                SelectMode::Rect => {
                    let a = pts[0];
                    if (a.0 - cx).abs() < 2.0 && (a.1 - cy).abs() < 2.0 {
                        None // a click, not a drag: deselect (CSP-like)
                    } else {
                        Some(Selection::from_rect(&self.doc, a.0, a.1, cx, cy))
                    }
                }
                SelectMode::Ellipse => {
                    let a = pts[0];
                    if (a.0 - cx).abs() < 2.0 && (a.1 - cy).abs() < 2.0 {
                        None // a click, not a drag: deselect, like Rect
                    } else {
                        let poly = Selection::ellipse_polygon(a.0, a.1, cx, cy);
                        Some(Selection::from_polygon(&self.doc, &poly))
                    }
                }
                SelectMode::Lasso => {
                    pts.push((cx, cy));
                    if pts.len() < 3 {
                        None
                    } else {
                        Some(Selection::from_polygon(&self.doc, &pts))
                    }
                }
                // Unreachable: the Shrink branch above returned.
                SelectMode::Shrink => None,
                // Unreachable: a magnetic trace never fills `select_drag`,
                // and the magnetic arm earlier in `canvas_up` returned.
                SelectMode::Magnetic => None,
            };
            match sel {
                Some(s) if !s.is_empty() => {
                    // SE-022 / the owner's everyday path: the modifier held
                    // at release combines the shape with the current
                    // selection; no modifier takes the persistent mode.
                    let m = self.shell.sync_modifiers();
                    let op = crate::cmd::effective_sel_op(m.shift, m.alt, self.sel_op);
                    let combined = match &self.doc.selection {
                        Some(cur) if op != mn_core::SelectionOp::Replace => {
                            cur.combine(&s, &self.doc, op)
                        }
                        _ => s,
                    };
                    if combined.is_empty() {
                        // Subtracted away: empty means "everything", so a
                        // real deselect instead.
                        self.push_cmd(AppCmd::Deselect);
                    } else {
                        self.doc.selection = Some(combined);
                        self.doc.touch();
                    }
                }
                _ => self.push_cmd(AppCmd::Deselect),
            }
            self.needs_redraw = true;
        }
    }

    // --- Edit ▸ Transform gestures -----------------------------------------

    /// Press during an active Transform: pick what the press grabbed — the
    /// rotate stalk, a bbox corner (two-axis scale off the opposite corner),
    /// an edge midpoint (one-axis scale off the opposite edge, TR-004), the
    /// pivot marker (TR-003; Alt+press places it anywhere), inside the bbox
    /// (move), outside (rotate). The decision itself is
    /// [`TransformDrag::hit_test`], shared with the cursor.
    fn transform_down(&mut self, x: f32, y: f32) {
        let (cx, cy) = self.viewport.to_canvas(x, y);
        let zoom = self.viewport.zoom;
        // Read the modifiers BEFORE borrowing the drag: `sync_modifiers`
        // wants `&mut self.shell`.
        let alt = self.shell.sync_modifiers().alt;
        let Some(drag) = &mut self.transform_drag else {
            return;
        };
        let grab = if drag.mesh.is_some() {
            // A mesh drag claims the pointer: lattice points first (10 px
            // screen), then whole-lattice translate; affine grabs no-op.
            if drag.mesh.as_ref().is_some_and(|m| m.puppet) {
                // PUPPET (row 54): Alt+click a pin removes it; a press on
                // a pin drags it; a press anywhere else DROPS a new pin
                // and drags it immediately (CSP's drop-and-pull).
                let tol = 10.0 / zoom.max(0.01);
                let hit = drag.mesh.as_ref().unwrap().pin_at([cx, cy], tol);
                if alt {
                    if let Some(i) = hit {
                        let m = drag.mesh.as_mut().unwrap();
                        m.pins.remove(i);
                        m.sync(drag.source.rect);
                    }
                    return;
                }
                match hit {
                    Some(i) => crate::app::TransformGrab::PuppetPin(i),
                    None => {
                        let m = drag.mesh.as_mut().unwrap();
                        m.pins.push(mn_core::mesh::PuppetPin {
                            orig: [cx, cy],
                            cur: [cx, cy],
                        });
                        m.sync(drag.source.rect);
                        crate::app::TransformGrab::PuppetPin(m.pins.len() - 1)
                    }
                }
            } else {
                drag.mesh_point_at([cx, cy], zoom)
                    .map(crate::app::TransformGrab::MeshPoint)
                    .unwrap_or(crate::app::TransformGrab::Move)
            }
        } else {
            drag.hit_test([cx, cy], zoom, alt)
        };
        drag.gesture = Some(TransformGesture {
            grab,
            start: [cx, cy],
            bbox0: drag.bbox,
            sx0: drag.sx,
            sy0: drag.sy,
            rad0: drag.rad,
            tx0: drag.tx,
            ty0: drag.ty,
        });
    }

    /// Pointer motion while a Transform gesture is down. The math is
    /// [`TransformDrag::apply_gesture`] (pure, unit-tested); this arm only
    /// collects the live modifiers and the Keep-aspect setting.
    fn transform_move(&mut self, cx: f32, cy: f32) {
        // Same borrow order as `transform_down` — modifiers first.
        let m = self.shell.sync_modifiers();
        let (shift, alt) = (m.shift, m.alt);
        let keep_aspect = self.transform_keep_aspect;
        let Some(drag) = &mut self.transform_drag else {
            return;
        };
        let Some(g) = drag.gesture else {
            return;
        };
        drag.apply_gesture(&g, [cx, cy], shift, alt, keep_aspect);
    }
}

/// One Object-tool referent under a point (owner item 2026-08-19, top of
/// the text arc): the cycle's currency.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ObjRef {
    Text(usize, usize),
    Gen(usize),
    Balloon(usize, usize),
    Frame(usize, usize),
}

impl ObjRef {
    pub fn label(&self) -> &'static str {
        match self {
            ObjRef::Text(..) => "text",
            ObjRef::Gen(_) => "effect lines",
            ObjRef::Balloon(..) => "balloon",
            ObjRef::Frame(..) => "frame",
        }
    }
}

impl App {
    /// Rulers part 2: close the in-progress curve ruler (Enter or
    /// double-click). Fewer than 2 vertices is discarded.
    pub fn finish_curve_ruler(&mut self) {
        self.ruler_pending = None;
        if let Some(pts) = self.curve_pending.take() {
            if pts.len() >= 2 {
                let before = self.doc.rulers.clone();
                self.doc.rulers.curves.push(mn_core::CurveRuler { pts });
                self.doc.rulers.on = true;
                self.doc.record_rulers(before, "Add ruler");
                self.set_status("curve ruler created — snapping on");
                self.needs_redraw = true;
                return;
            }
        }
        self.set_status("curve ruler discarded (needs 2+ vertices)");
    }
}

#[cfg(test)]
mod gen_drag_tests {
    use super::*;
    use mn_core::genlines::GenLinesSpec;

    fn spec(focus: bool) -> GenLinesSpec {
        GenLinesSpec {
            focus,
            a: if focus { 200.0 } else { 30.0 },
            b: if focus { 150.0 } else { 80.0 },
            c: if focus { 40.0 } else { 240.0 },
            d: if focus { 180.0 } else { 0.0 },
            count: 60,
            width: 2.0,
            jitter: 0.1,
            seed: 3,
            ..Default::default()
        }
    }

    /// SF-004/005: the driver math — centre moves the run, radii clamp
    /// inside each other, angle follows the pointer, lengths clamp.
    #[test]
    fn driver_math() {
        let size = (400u32, 300u32);
        let d = |mode, s, c, o| GenLinesDrag {
            layer: 0,
            mode,
            start: s,
            cur: c,
            orig: o,
        };
        // Centre drag moves the convergence point only.
        let s = gen_drag_spec(
            &d(
                GenDragMode::Center,
                (200.0, 150.0),
                (230.0, 170.0),
                spec(true),
            ),
            size,
        );
        assert_eq!((s.a, s.b), (230.0, 170.0));
        assert_eq!(s.c, 40.0, "radii untouched");

        // Outer radius = distance from the centre; clamped above r_in.
        let s = gen_drag_spec(
            &d(
                GenDragMode::ROut,
                (240.0, 150.0),
                (200.0, 150.0),
                spec(true),
            ),
            size,
        );
        assert!((s.d - 44.0).abs() < 0.01, "clamped to r_in + 4");
        let s = gen_drag_spec(
            &d(
                GenDragMode::ROut,
                (240.0, 150.0),
                (290.0, 150.0),
                spec(true),
            ),
            size,
        );
        assert!((s.d - 90.0).abs() < 0.01);

        // Speed angle follows the pointer around the canvas centre.
        let s = gen_drag_spec(
            &d(GenDragMode::Angle, (0.0, 0.0), (400.0, 150.0), spec(false)),
            size,
        );
        assert!((s.a - 0.0).abs() < 0.01, "east = 0deg: {}", s.a);
        let s = gen_drag_spec(
            &d(GenDragMode::Angle, (0.0, 0.0), (200.0, 0.0), spec(false)),
            size,
        );
        assert!((s.a - (-90.0)).abs() < 0.01, "north = -90deg");

        // Lengths project onto the direction and clamp ordered.
        let mut zero_deg = spec(false);
        zero_deg.a = 0.0;
        let s = gen_drag_spec(
            &d(GenDragMode::LenMax, (0.0, 0.0), (320.0, 150.0), zero_deg),
            size,
        );
        assert!((s.c - 120.0).abs() < 0.01, "projected onto the 0deg line");
    }

    /// Audit B, 2026-08-19: the dialog sets inner/outer radius and
    /// min/max length as INDEPENDENT values — nothing orders them, and
    /// f32::clamp panics on min > max, which aborted the process through
    /// wndproc. Every drag on every degenerate spec must instead produce
    /// an ordered, non-panicking spec.
    #[test]
    fn driver_math_survives_inverted_dialog_values() {
        let size = (6070u32, 8598u32); // a 600 dpi B4 — wider than the old 6000 ceiling
        let d = |mode, s, c, o| GenLinesDrag {
            layer: 0,
            mode,
            start: s,
            cur: c,
            orig: o,
        };

        // Outer radius at the dialog's minimum (4.0), inner dragged past
        // it: the RIn clamp degenerates to 4..4 instead of 4.0 > d - 4.
        let mut tiny_out = spec(true);
        tiny_out.d = 4.0;
        let s = gen_drag_spec(
            &d(GenDragMode::RIn, (0.0, 0.0), (3000.0, 4000.0), tiny_out),
            size,
        );
        assert_eq!(s.c, 4.0, "clamped to the degenerate 4..4");

        // Inner radius near the canvas diagonal, outer dragged: the old
        // 6000.0 literal ceiling sat BELOW the lower bound c + 4 — the
        // bounds are canvas-derived now and stay ordered.
        let mut huge_in = spec(true);
        huge_in.c = 9000.0;
        huge_in.d = 9500.0;
        let s = gen_drag_spec(
            &d(GenDragMode::ROut, (0.0, 0.0), (3000.0, 4000.0), huge_in),
            size,
        );
        assert!(s.d >= s.c, "outer stays >= inner");

        // Speed lengths: max at the dialog's minimum (8.0), min dragged.
        let mut tiny_max = spec(false);
        tiny_max.c = 8.0;
        let s = gen_drag_spec(
            &d(GenDragMode::LenMin, (0.0, 0.0), (3000.0, 4000.0), tiny_max),
            size,
        );
        assert_eq!(s.b, 8.0, "clamped to the degenerate 8..8");

        // Min dragged far past max: LenMax keeps the order.
        let mut huge_min = spec(false);
        huge_min.b = 11000.0;
        huge_min.c = 11100.0;
        let s = gen_drag_spec(
            &d(GenDragMode::LenMax, (0.0, 0.0), (3000.0, 4000.0), huge_min),
            size,
        );
        assert!(s.c >= s.b, "max stays >= min");
    }
}

// --- frame expand arrows (owner ask 2026-08-26: CSP's yellow triangles) ---

/// The nearest EXPANSION target beyond each bbox edge of frame `fi` in
/// `layer`: [left, right, top, bottom] as canvas coordinates. Candidates
/// are sibling frames' bbox edges (any visible frame folder — a divide
/// makes each panel its own folder) and the page's template lines
/// (trim/bleed/inner/safety); the nearest beyond the edge wins. `None` =
/// nothing to expand to in that direction. Axis-aligned bbox math: a
/// slanted panel's OTHER vertices keep their shape (only vertices on the
/// moved edge follow).
pub(crate) fn frame_expand_targets(
    doc: &mn_core::Document,
    page: Option<&mn_core::PageSetup>,
    layer: usize,
    fi: usize,
) -> [Option<f32>; 4] {
    let Some(frames) = doc.layers.get(layer).and_then(|l| l.frames()) else {
        return [None; 4];
    };
    let Some(f) = frames.frames.get(fi) else {
        return [None; 4];
    };
    let b = f.bbox();
    const EPS: f32 = 1.5;
    // Vertical template lines for left/right, horizontal for top/bottom.
    let mut vlines: Vec<f32> = Vec::new();
    let mut hlines: Vec<f32> = Vec::new();
    if let Some(p) = page.filter(|p| p.has_guides()) {
        let push = |r: [f32; 4], v: &mut Vec<f32>, h: &mut Vec<f32>| {
            v.push(r[0]);
            v.push(r[2]);
            h.push(r[1]);
            h.push(r[3]);
        };
        push(p.trim_rect_px(), &mut vlines, &mut hlines);
        push(p.bleed_rect_px(), &mut vlines, &mut hlines);
        push(p.inner_rect_px_on(true), &mut vlines, &mut hlines);
        push(p.inner_rect_px_on(false), &mut vlines, &mut hlines);
    }
    // Every sibling bbox edge is a candidate line on BOTH axes (a
    // neighbour's LEFT edge can also be the thing we expand onto, when it
    // sits inside our span) — no lefts/rights split.
    for (li, l) in doc.layers.iter().enumerate() {
        if !l.visible || l.frames().is_none() {
            continue;
        }
        let Some(fs) = l.frames() else { continue };
        for (si, sib) in fs.frames.iter().enumerate() {
            if li == layer && si == fi {
                continue;
            }
            let sb = sib.bbox();
            vlines.push(sb[0]);
            vlines.push(sb[2]);
            hlines.push(sb[1]);
            hlines.push(sb[3]);
        }
    }
    // Left: the nearest edge strictly LEFT of our left edge (a neighbour's
    // right edge to expand INTO, or a template line). Max of candidates
    // below b[0]; likewise mirrored for the other three.
    let nearest_below = |x: f32, cands: &mut Vec<f32>| -> Option<f32> {
        let best = cands
            .iter()
            .filter(|&&c| c < x - EPS)
            .cloned()
            .fold(f32::MIN, f32::max);
        (best > f32::MIN / 2.0).then_some(best)
    };
    let nearest_above = |x: f32, cands: &mut Vec<f32>| -> Option<f32> {
        let best = cands
            .iter()
            .filter(|&&c| c > x + EPS)
            .cloned()
            .fold(f32::MAX, f32::min);
        (best < f32::MAX / 2.0).then_some(best)
    };
    [
        nearest_below(b[0], &mut vlines),
        nearest_above(b[2], &mut vlines),
        nearest_below(b[1], &mut hlines),
        nearest_above(b[3], &mut hlines),
    ]
}

impl App {
    /// Where each of the SELECTED frame's expand arrows sits (screen px),
    /// for the direction it expands in — `[dir, tip_pos]`, dirs 0..3 =
    /// left, right, top, bottom. Arrows live just OUTSIDE the bbox edge
    /// midpoint (CSP's triangles); the top arrow dodges the rotation
    /// lollipop by sitting beside it.
    pub(crate) fn frame_expand_arrow_pts(&self) -> Vec<(usize, egui::Pos2)> {
        let Some((li, fi)) = self.object_sel else {
            return Vec::new();
        };
        let Some(f) = self
            .doc
            .layers
            .get(li)
            .and_then(|l| l.frames())
            .and_then(|fs| fs.frames.get(fi))
        else {
            return Vec::new();
        };
        let targets = frame_expand_targets(&self.doc, self.page.as_ref(), li, fi);
        let b = f.bbox();
        let to_screen = |p: [f32; 2]| {
            let (x, y) = self.viewport.to_screen(p[0], p[1]);
            egui::pos2(x, y)
        };
        let (tl, br) = (to_screen([b[0], b[1]]), to_screen([b[2], b[3]]));
        let cx = (tl.x + br.x) * 0.5;
        let cy = (tl.y + br.y) * 0.5;
        const OFF: f32 = 16.0;
        let mut out = Vec::new();
        if targets[0].is_some() {
            out.push((0usize, egui::pos2(tl.x - OFF, cy)));
        }
        if targets[1].is_some() {
            out.push((1usize, egui::pos2(br.x + OFF, cy)));
        }
        if targets[2].is_some() {
            // Beside the lollipop, not under it.
            out.push((2usize, egui::pos2(cx + 18.0, tl.y - OFF)));
        }
        if targets[3].is_some() {
            out.push((3usize, egui::pos2(cx, br.y + OFF)));
        }
        out
    }

    /// One tap on an expand arrow: grow the SELECTED frame's matching bbox
    /// edge to its target (the neighbour's edge or the template line —
    /// the gutter between them dies, CSP's bleed panel in one press).
    /// Only vertices ON the moved edge follow; a slanted panel keeps its
    /// shape elsewhere. Records ONE undo step via `set_frames`.
    pub(crate) fn frame_expand_press(&mut self, dir: usize) -> bool {
        let Some((li, fi)) = self.object_sel else {
            return false;
        };
        let targets = frame_expand_targets(&self.doc, self.page.as_ref(), li, fi);
        let Some(target) = targets[dir] else {
            return false;
        };
        let Some(fs) = self.doc.layers.get(li).and_then(|l| l.frames()) else {
            return false;
        };
        let Some(f) = fs.frames.get(fi) else {
            return false;
        };
        let b = f.bbox();
        const EPS: f32 = 1.5;
        let mut nf = f.clone();
        let mut moved = false;
        for p in nf.points.iter_mut() {
            let on_edge = match dir {
                0 => (p[0] - b[0]).abs() < EPS,
                1 => (p[0] - b[2]).abs() < EPS,
                2 => (p[1] - b[1]).abs() < EPS,
                _ => (p[1] - b[3]).abs() < EPS,
            };
            if on_edge {
                match dir {
                    0 | 1 => p[0] = target,
                    _ => p[1] = target,
                }
                moved = true;
            }
        }
        if !moved {
            return false;
        }
        let valid =
            nf.area() >= mn_core::frame::MIN_FRAME_AREA && (nf.is_convex() || nf.is_simple());
        if !valid {
            self.set_status("expand refused — the panel would collapse");
            return true;
        }
        let mut set = fs.clone();
        set.frames[fi] = nf;
        if self.doc.set_frames(li, set) {
            self.renumber_frames();
            self.renderer.invalidate();
            self.set_status("panel expanded to the next border");
            self.mark_dirty();
            return true;
        }
        false
    }
}
