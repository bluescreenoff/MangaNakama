//! Speech balloons (fukidashi) — vector bubbles that rasterize into a layer.
//!
//! Same shape of idea as [`crate::frame`]: a balloon layer (`LayerKind::Balloon`)
//! owns a [`BalloonSet`], and its raster is fully *derived* — opaque white
//! inside every balloon (the fill that hides art behind the bubble), an
//! anti-aliased black border on the outline, transparent elsewhere.
//!
//! The whole layer is one signed-distance field: `min` over every body and
//! tail. That union is the feature, not an optimization — a tail's triangle
//! overlapping its body erases the border along the shared edge (the classic
//! "tail opens into the bubble" look), and two overlapping balloons on one
//! layer merge into a joined balloon exactly like CSP combines balloons that
//! share a layer.
//!
//! Bodies may be ellipses, rounded rects, or arbitrary (concave-ok) polygons
//! from the freehand balloon tool. Polygon/rect/tail distances are exact and
//! 1-Lipschitz, which the rasterizer's tile classification leans on; the
//! ellipse uses the standard scaled approximation and gets a conservative
//! inscribed-circle bound instead.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::tile::{FIX15_ONE, TILE_SIZE, Tile, TileIdx};
use crate::tone::TonePattern;

/// Bodies smaller than this across (px) are refused / considered degenerate.
pub const MIN_BALLOON_EXTENT: f32 = 8.0;

/// A balloon body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum BalloonShape {
    /// Axis-aligned ellipse.
    Ellipse { center: [f32; 2], radii: [f32; 2] },
    /// `rect = [x0, y0, x1, y1]`, `corner` = corner radius in px.
    RoundRect { rect: [f32; 4], corner: f32 },
    /// Closed polygon, any winding, concave allowed (the drawn balloon).
    /// The optional per-vertex data is what the freehand balloon tool really
    /// produces: `widths` = pen pressure 0..1 at that anchor (CSP's drawn
    /// balloons are smooth pressure-aware curves, not hard polygons — the
    /// raster runs a Catmull-Rom spline through the anchors, see
    /// [`tessellate_closed`]), `corners` = CSP's corner-anchor flag (a sharp
    /// kink instead of the smooth curve through). Both default empty: a plain
    /// polygon (old files) renders with hard edges as before.
    Polygon {
        points: Vec<[f32; 2]>,
        #[serde(default)]
        widths: Vec<f32>,
        #[serde(default)]
        corners: Vec<bool>,
    },
}

/// What a tail LOOKS like (`B-005`). Manga uses several: the spoken wedge,
/// the thought bubble's chain of puffs, the shout spike.
///
/// [`TailKind::Spoken`] is the default and is the exact isosceles triangle
/// every tail was before this enum existed — an old file deserializes to it
/// and rasterizes down the identical code path.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TailKind {
    /// The classic wedge: straight flanks from a `width`-wide base to a point.
    #[default]
    Spoken,
    /// 思考 — a chain of shrinking circles instead of a wedge. They are
    /// unioned like everything else, so the first puff merges into the body
    /// and the last floats free with its own outline.
    Thought,
    /// The shout spike: flanks bowed INWARD, so the tail leaves the bubble
    /// wide and narrows to a needle.
    Spike,
}

impl TailKind {
    pub const ALL: [TailKind; 3] = [TailKind::Spoken, TailKind::Thought, TailKind::Spike];

    pub fn label(self) -> &'static str {
        match self {
            TailKind::Spoken => "Spoken",
            TailKind::Thought => "Thought",
            TailKind::Spike => "Shout",
        }
    }
}

/// How many samples a bent/spiked tail's flank is tessellated into.
const TAIL_STEPS: usize = 16;

/// Flank exponent of a [`TailKind::Spike`]: half-width `hw·(1-t)^k`. `k = 1`
/// is the straight wedge, so anything above 1 bows the flanks inward; 2.5
/// gives the needle a shout balloon wants without collapsing it at the base.
const SPIKE_TAPER: f32 = 2.5;

/// A tail: by default an isosceles triangle from a base segment (centred on
/// `base`, perpendicular to base→tip, `width` wide) out to `tip`. Drawn
/// unioned with the body, so a base placed inside the balloon merges
/// seamlessly.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tail {
    pub base: [f32; 2],
    pub tip: [f32; 2],
    pub width: f32,
    /// `B-005` tail type. Old files have none: [`TailKind::Spoken`].
    #[serde(default)]
    pub kind: TailKind,
    /// `B-006` bend: how far the centreline bows sideways, as a fraction of
    /// the base→tip length (positive = to the LEFT of that direction). This
    /// is how a tail curves *around* the art instead of stabbing through it.
    /// 0 = dead straight, which is what every old file has and what keeps the
    /// spoken tail on its original triangle code path.
    #[serde(default)]
    pub bend: f32,
}

impl Default for Tail {
    fn default() -> Self {
        Self {
            base: [0.0, 0.0],
            tip: [0.0, 0.0],
            width: 1.0,
            kind: TailKind::Spoken,
            bend: 0.0,
        }
    }
}

/// A screened (toned) balloon fill — `C-04x`, the printed whisper/flashback
/// bubble whose interior is a halftone rather than flat paper.
///
/// The cell is stored in **canvas pixels, not LPI**. [`BalloonSet::rasterize`]
/// is called from `Document` paths that carry no dpi, so Tool Property does
/// the LPI→px conversion once, when you set it. The quirk that follows is
/// real and documented: changing the document's dpi afterwards does not
/// re-flow a balloon's screen — it stays the size you gave it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BalloonTone {
    pub cell_px: f32,
    pub angle_deg: f32,
    /// Flat ink coverage 0..=1 — the "30 % tone" number.
    pub density: f32,
    pub pattern: TonePattern,
}

impl Default for BalloonTone {
    fn default() -> Self {
        Self {
            cell_px: 10.0,
            angle_deg: 45.0,
            density: 0.3,
            pattern: TonePattern::Dots,
        }
    }
}

impl BalloonTone {
    /// Screen coverage 0..=1 at a canvas pixel CENTRE, 2×2 subsampled — the
    /// same geometry and the same sample offsets as `tone::rasterize_tile`,
    /// so a toned balloon and a tone layer at the same settings print the
    /// same screen.
    pub fn coverage(&self, p: [f32; 2]) -> f32 {
        let (sn, cs) = self.angle_deg.to_radians().sin_cos();
        let cell = self.cell_px.max(2.0);
        let ink = self.density.clamp(0.0, 1.0);
        let mut n = 0u32;
        for (dx, dy) in [
            (-0.25f32, -0.25f32),
            (0.25, -0.25),
            (-0.25, 0.25),
            (0.25, 0.25),
        ] {
            let (fx, fy) = (p[0] + dx, p[1] + dy);
            if self
                .pattern
                .on(fx * cs - fy * sn, fx * sn + fy * cs, ink, cell)
            {
                n += 1;
            }
        }
        n as f32 * 0.25
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Balloon {
    /// Stable identity (the automation round) — `TextItem::id`'s twin: `0`
    /// until a commit door (`Document::set_balloons`/`add_balloon_layer`)
    /// mints it; unique in the document; rides the `mnc-balloons` JSON.
    #[serde(default)]
    pub id: u64,
    pub shape: BalloonShape,
    #[serde(default)]
    pub tails: Vec<Tail>,
    /// CSP's "correct line width": a render-time multiplier on the outline
    /// width (with the pressure modulation inside it). The recorded
    /// per-anchor pressure widths are NEVER rewritten — scaling those (the
    /// old app-side implementation) saturated them at 1.0 and a later
    /// scale-down returned a flat border, not the original taper
    /// (auditor round 33). 1.0 = as drawn; old files default to 1.0.
    #[serde(default = "default_width_scale")]
    pub width_scale: f32,
    /// `B-001` outline colour. Old files: black, which is what the
    /// rasterizer hardcoded before this field existed.
    #[serde(default = "black")]
    pub line_color: [u8; 3],
    /// `B-003` interior colour. Old files: white, ditto.
    #[serde(default = "white")]
    pub fill_color: [u8; 3],
    /// `B-002` outline opacity 0..=1.
    #[serde(default = "full")]
    pub line_opacity: f32,
    /// `B-004` interior opacity 0..=1. **0 is CSP's "fill inside frame" off**
    /// — the outline inks and the art behind shows straight through the
    /// bubble. There is no separate boolean: a fill you cannot see and a fill
    /// that is not there are the same balloon.
    #[serde(default = "full")]
    pub fill_opacity: f32,
    /// `C-04x` screened fill. `None` = the flat fill every old file has.
    #[serde(default)]
    pub fill_tone: Option<BalloonTone>,
}

impl Default for Balloon {
    fn default() -> Self {
        Self {
            id: 0,
            shape: BalloonShape::Ellipse {
                center: [0.0, 0.0],
                radii: [0.0, 0.0],
            },
            tails: Vec::new(),
            width_scale: 1.0,
            line_color: black(),
            fill_color: white(),
            line_opacity: 1.0,
            fill_opacity: 1.0,
            fill_tone: None,
        }
    }
}

fn default_width_scale() -> f32 {
    1.0
}

fn black() -> [u8; 3] {
    [0, 0, 0]
}

fn white() -> [u8; 3] {
    [255, 255, 255]
}

fn full() -> f32 {
    1.0
}

fn rgb_f(c: [u8; 3]) -> [f32; 3] {
    [
        c[0] as f32 / 255.0,
        c[1] as f32 / 255.0,
        c[2] as f32 / 255.0,
    ]
}

/// Every balloon on a balloon layer + the shared outline width.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BalloonSet {
    pub balloons: Vec<Balloon>,
    /// Outline stroke width in canvas px, centred on the shape boundary.
    pub border_px: f32,
    /// Drawn balloons: modulate the outline width with the recorded pen
    /// pressure (CSP's drawn-bubble feel — a light hand inks a thin line).
    #[serde(default)]
    pub pressure_width: bool,
}

/// Widest `width_scale` in the set (≥ 1.0 so `reach` never shrinks below
/// the unscaled border) — the raster's bbox inflation must cover the
/// corrected line width too.
fn max_width_scale(set: &BalloonSet) -> f32 {
    set.balloons
        .iter()
        .map(|b| b.width_scale)
        .fold(1.0, f32::max)
}

/// Which control point of a balloon an Object-tool drag grabbed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BalloonHandle {
    /// Ellipse: 0=right 1=bottom 2=left 3=top. RoundRect: corner 0..4 (x0y0,
    /// x1y0, x1y1, x0y1). Polygon: vertex index.
    Shape(usize),
    TailTip(usize),
    TailBase(usize),
}

fn sub(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn dot(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

fn len(a: [f32; 2]) -> f32 {
    dot(a, a).sqrt()
}

/// Exact signed distance to a closed polygon (IQ's algorithm): min distance to
/// the edges, sign by crossing parity. Handles concave polygons.
fn polygon_sdf(pts: &[[f32; 2]], p: [f32; 2]) -> f32 {
    let n = pts.len();
    if n < 3 {
        return f32::INFINITY;
    }
    let mut d = dot(sub(p, pts[0]), sub(p, pts[0]));
    let mut s = 1.0f32;
    let mut j = n - 1;
    for i in 0..n {
        let e = sub(pts[j], pts[i]);
        let w = sub(p, pts[i]);
        let t = if dot(e, e) > 1e-12 {
            (dot(w, e) / dot(e, e)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let b = [w[0] - e[0] * t, w[1] - e[1] * t];
        d = d.min(dot(b, b));
        let c = [
            p[1] >= pts[i][1],
            p[1] < pts[j][1],
            e[0] * w[1] > e[1] * w[0],
        ];
        if c.iter().all(|&x| x) || c.iter().all(|&x| !x) {
            s = -s;
        }
        j = i;
    }
    s * d.sqrt()
}

/// Signed distance to a rounded rect (exact).
fn roundrect_sdf(rect: [f32; 4], corner: f32, p: [f32; 2]) -> f32 {
    let cx = (rect[0] + rect[2]) * 0.5;
    let cy = (rect[1] + rect[3]) * 0.5;
    let hx = ((rect[2] - rect[0]) * 0.5).abs();
    let hy = ((rect[3] - rect[1]) * 0.5).abs();
    let r = corner.clamp(0.0, hx.min(hy));
    let qx = (p[0] - cx).abs() - (hx - r);
    let qy = (p[1] - cy).abs() - (hy - r);
    let outside = len([qx.max(0.0), qy.max(0.0)]);
    outside + qx.max(qy).min(0.0) - r
}

/// Approximate signed distance to an ellipse (IQ's scaled distance). Not a
/// true metric — never use it for conservative tile classification, only for
/// per-pixel coverage where a fraction-of-a-pixel error is invisible.
fn ellipse_sdf(center: [f32; 2], radii: [f32; 2], p: [f32; 2]) -> f32 {
    let rx = radii[0].max(1e-3);
    let ry = radii[1].max(1e-3);
    let q = sub(p, center);
    let k1 = len([q[0] / rx, q[1] / ry]);
    let k2 = len([q[0] / (rx * rx), q[1] / (ry * ry)]);
    if k2 < 1e-9 {
        return -rx.min(ry); // at the exact center
    }
    k1 * (k1 - 1.0) / k2
}

/// A tail's outline, built ONCE per rasterize. The per-pixel loop must not
/// allocate, and a thought chain or a bent flank cannot be re-derived per
/// pixel for free — so the shapes are hoisted out here.
///
/// [`TailGeom::Tri`] is the straight spoken wedge on its own branch: an old
/// file's tail goes through the identical three-point `polygon_sdf` it always
/// did, which is what keeps its raster byte-identical.
#[derive(Clone, Debug, PartialEq)]
pub enum TailGeom {
    /// The classic wedge (`Spoken`, `bend == 0`).
    Tri([[f32; 2]; 3]),
    /// A bent and/or spiked flank, tessellated into a closed polygon.
    Poly(Vec<[f32; 2]>),
    /// A thought chain: (centre, radius) puffs, unioned.
    Puffs(Vec<([f32; 2], f32)>),
}

impl TailGeom {
    pub fn sdf(&self, p: [f32; 2]) -> f32 {
        match self {
            TailGeom::Tri(t) => polygon_sdf(t, p),
            TailGeom::Poly(v) => polygon_sdf(v, p),
            TailGeom::Puffs(cs) => cs
                .iter()
                .fold(f32::INFINITY, |d, (c, r)| d.min(len(sub(p, *c)) - r)),
        }
    }

    /// Grow `r` = `[x0, y0, x1, y1]` to cover this outline.
    fn grow_bbox(&self, r: &mut [f32; 4]) {
        let mut hit = |x0: f32, y0: f32, x1: f32, y1: f32| {
            r[0] = r[0].min(x0);
            r[1] = r[1].min(y0);
            r[2] = r[2].max(x1);
            r[3] = r[3].max(y1);
        };
        match self {
            TailGeom::Tri(t) => {
                for p in t {
                    hit(p[0], p[1], p[0], p[1]);
                }
            }
            TailGeom::Poly(v) => {
                for p in v {
                    hit(p[0], p[1], p[0], p[1]);
                }
            }
            TailGeom::Puffs(cs) => {
                for (c, rad) in cs {
                    hit(c[0] - rad, c[1] - rad, c[0] + rad, c[1] + rad);
                }
            }
        }
    }
}

impl Tail {
    fn triangle(&self) -> [[f32; 2]; 3] {
        let d = sub(self.tip, self.base);
        let l = len(d).max(1e-3);
        let perp = [-d[1] / l, d[0] / l];
        let hw = self.width.max(1.0) * 0.5;
        [
            self.tip,
            [self.base[0] + perp[0] * hw, self.base[1] + perp[1] * hw],
            [self.base[0] - perp[0] * hw, self.base[1] - perp[1] * hw],
        ]
    }

    /// Point on the tail's centreline at `t` ∈ 0..=1 — a quadratic Bézier
    /// whose control point is the midpoint pushed sideways by `bend`. At
    /// `bend == 0` it is exactly the straight segment base→tip.
    fn center_at(&self, t: f32) -> [f32; 2] {
        let d = sub(self.tip, self.base);
        let l = len(d).max(1e-3);
        let perp = [-d[1] / l, d[0] / l];
        let ctrl = [
            self.base[0] + d[0] * 0.5 + perp[0] * self.bend * l,
            self.base[1] + d[1] * 0.5 + perp[1] * self.bend * l,
        ];
        let u = 1.0 - t;
        [
            u * u * self.base[0] + 2.0 * u * t * ctrl[0] + t * t * self.tip[0],
            u * u * self.base[1] + 2.0 * u * t * ctrl[1] + t * t * self.tip[1],
        ]
    }

    /// The closed outline of a tapering ribbon along the centreline:
    /// half-width `hw·(1-t)^taper` on both sides of the curve. `taper == 1`
    /// is the wedge's straight flank, `SPIKE_TAPER` bows it inward.
    fn ribbon(&self, taper: f32) -> Vec<[f32; 2]> {
        let hw = self.width.max(1.0) * 0.5;
        let e = 0.5 / TAIL_STEPS as f32;
        let flank = |t: f32, side: f32| -> [f32; 2] {
            let c = self.center_at(t);
            let d = sub(
                self.center_at((t + e).min(1.0)),
                self.center_at((t - e).max(0.0)),
            );
            let l = len(d).max(1e-6);
            let n = [-d[1] / l, d[0] / l];
            let w = hw * (1.0 - t).powf(taper) * side;
            [c[0] + n[0] * w, c[1] + n[1] * w]
        };
        let mut out = Vec::with_capacity(TAIL_STEPS * 2 + 1);
        // Up one flank to the tip (where the half-width is 0, so the two
        // flanks meet in the single point pushed here)…
        for i in 0..=TAIL_STEPS {
            out.push(flank(i as f32 / TAIL_STEPS as f32, 1.0));
        }
        // …and back down the other, skipping that shared tip vertex.
        for i in (0..TAIL_STEPS).rev() {
            out.push(flank(i as f32 / TAIL_STEPS as f32, -1.0));
        }
        out
    }

    /// The thought chain: circles along the centreline, shrinking toward the
    /// tip. The count follows the tail's own proportions (≈2.2 radii apart)
    /// so a short stubby tail does not come out as one merged blob and a long
    /// one does not come out as two lonely dots.
    fn puffs(&self) -> Vec<([f32; 2], f32)> {
        let hw = self.width.max(1.0) * 0.5;
        let l = len(sub(self.tip, self.base)).max(1.0);
        let n = (l / (hw * 2.2).max(1.0)).round().clamp(3.0, 6.0) as usize;
        (0..n)
            .map(|i| {
                let t = i as f32 / (n - 1) as f32;
                (self.center_at(t), (hw * (0.65 - 0.42 * t)).max(0.5))
            })
            .collect()
    }

    /// This tail's outline. Allocation-free for the default wedge.
    pub fn geometry(&self) -> TailGeom {
        match self.kind {
            TailKind::Spoken if self.bend == 0.0 => TailGeom::Tri(self.triangle()),
            TailKind::Spoken => TailGeom::Poly(self.ribbon(1.0)),
            TailKind::Spike => TailGeom::Poly(self.ribbon(SPIKE_TAPER)),
            TailKind::Thought => TailGeom::Puffs(self.puffs()),
        }
    }

    fn sdf(&self, p: [f32; 2]) -> f32 {
        self.geometry().sdf(p)
    }
}

impl BalloonShape {
    /// The tessellated (dense) closed polyline + per-vertex pressure, for
    /// drawn shapes. `None` for the analytic bodies.
    fn dense(&self) -> Option<(Vec<[f32; 2]>, Vec<f32>)> {
        match self {
            BalloonShape::Polygon {
                points,
                corners,
                widths,
            } => Some(tessellate_closed(points, corners, widths)),
            _ => None,
        }
    }

    /// Signed distance. Drawn (anchor) shapes evaluate their dense spline.
    pub fn sdf(&self, p: [f32; 2]) -> f32 {
        match self {
            BalloonShape::Ellipse { center, radii } => ellipse_sdf(*center, *radii, p),
            BalloonShape::RoundRect { rect, corner } => roundrect_sdf(*rect, *corner, p),
            BalloonShape::Polygon { .. } => {
                let (pts, _) = self.dense().expect("polygon");
                polygon_sdf(&pts, p)
            }
        }
    }

    /// (signed distance, pen pressure 0..1 at the nearest boundary). Used by
    /// the rasterizer; `widths` empty or a non-drawn body reports 1.0.
    pub fn sdf_w(&self, p: [f32; 2]) -> (f32, f32) {
        match self {
            BalloonShape::Polygon { .. } => {
                let (pts, ws) = self.dense().expect("polygon");
                polygon_sdf_w(&pts, &ws, p)
            }
            s => (s.sdf(p), 1.0),
        }
    }

    pub fn bbox(&self) -> [f32; 4] {
        match self {
            BalloonShape::Ellipse { center, radii } => [
                center[0] - radii[0],
                center[1] - radii[1],
                center[0] + radii[0],
                center[1] + radii[1],
            ],
            BalloonShape::RoundRect { rect, .. } => [
                rect[0].min(rect[2]),
                rect[1].min(rect[3]),
                rect[0].max(rect[2]),
                rect[1].max(rect[3]),
            ],
            BalloonShape::Polygon { points, .. } => {
                let mut r = [
                    f32::INFINITY,
                    f32::INFINITY,
                    f32::NEG_INFINITY,
                    f32::NEG_INFINITY,
                ];
                for p in points {
                    r[0] = r[0].min(p[0]);
                    r[1] = r[1].min(p[1]);
                    r[2] = r[2].max(p[0]);
                    r[3] = r[3].max(p[1]);
                }
                // The spline can bulge past the control hull slightly
                // (Catmull-Rom overshoot); pad so tile classification never
                // clips the curve.
                let pad = 16.0;
                [r[0] - pad, r[1] - pad, r[2] + pad, r[3] + pad]
            }
        }
    }

    /// A lower bound on "how deep inside the body is `p`" (0 when not
    /// guaranteed inside). Safe for tile classification: for exact SDFs the
    /// distance itself, for the ellipse the inscribed circle.
    fn inside_depth(&self, p: [f32; 2]) -> f32 {
        match self {
            BalloonShape::Ellipse { center, radii } => {
                (radii[0].min(radii[1]) - len(sub(p, *center))).max(0.0)
            }
            _ => (-self.sdf(p)).max(0.0),
        }
    }

    fn translate(&mut self, dx: f32, dy: f32) {
        match self {
            BalloonShape::Ellipse { center, .. } => {
                center[0] += dx;
                center[1] += dy;
            }
            BalloonShape::RoundRect { rect, .. } => {
                rect[0] += dx;
                rect[1] += dy;
                rect[2] += dx;
                rect[3] += dy;
            }
            BalloonShape::Polygon { points, .. } => {
                for p in points {
                    p[0] += dx;
                    p[1] += dy;
                }
            }
        }
    }
}

impl Balloon {
    /// Union signed distance over the body and every tail.
    pub fn sdf(&self, p: [f32; 2]) -> f32 {
        let mut d = self.shape.sdf(p);
        for t in &self.tails {
            d = d.min(t.sdf(p));
        }
        d
    }

    /// Union (signed distance, boundary pressure) over the body and every
    /// tail — the winning component's pressure wins (tails are uniform 1.0).
    pub fn sdf_w(&self, p: [f32; 2]) -> (f32, f32) {
        let (mut d, mut w) = self.shape.sdf_w(p);
        for t in &self.tails {
            let td = t.sdf(p);
            if td < d {
                d = td;
                w = 1.0;
            }
        }
        (d, w)
    }

    pub fn contains(&self, p: [f32; 2]) -> bool {
        self.sdf(p) <= 0.0
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        self.shape.translate(dx, dy);
        for t in &mut self.tails {
            t.base = [t.base[0] + dx, t.base[1] + dy];
            t.tip = [t.tip[0] + dx, t.tip[1] + dy];
        }
    }

    /// The Operation tool's blue-box transform: scale `(sx, sy)` then rotate
    /// `rad` around `c`, applied to every anchor, extent and tail. Drawn
    /// (spline) balloons transform EXACTLY; an ellipse/round-rect body is
    /// axis-aligned by definition, so a rotation collapses to its transformed
    /// extents (a v1 approximation, fine for the uniform scales and the
    /// lollipop's small rotations it is used for).
    pub fn transform_around(&mut self, c: [f32; 2], sx: f32, sy: f32, rad: f32) {
        let (cr, sr) = (rad.cos(), rad.sin());
        let map = |p: &mut [f32; 2]| {
            let x = (p[0] - c[0]) * sx;
            let y = (p[1] - c[1]) * sy;
            p[0] = c[0] + x * cr - y * sr;
            p[1] = c[1] + x * sr + y * cr;
        };
        match &mut self.shape {
            BalloonShape::Ellipse { center, radii } => {
                map(center);
                radii[0] *= sx;
                radii[1] *= sy;
            }
            BalloonShape::RoundRect { rect, .. } => {
                let mut pts = [
                    [rect[0], rect[1]],
                    [rect[2], rect[1]],
                    [rect[2], rect[3]],
                    [rect[0], rect[3]],
                ];
                for p in &mut pts {
                    map(p);
                }
                *rect = [
                    pts.iter().fold(f32::INFINITY, |m, p| m.min(p[0])),
                    pts.iter().fold(f32::INFINITY, |m, p| m.min(p[1])),
                    pts.iter().fold(f32::NEG_INFINITY, |m, p| m.max(p[0])),
                    pts.iter().fold(f32::NEG_INFINITY, |m, p| m.max(p[1])),
                ];
            }
            BalloonShape::Polygon { points, .. } => {
                for p in points {
                    map(p);
                }
            }
        }
        let s_avg = (sx + sy) * 0.5;
        for t in &mut self.tails {
            map(&mut t.base);
            map(&mut t.tip);
            t.width *= s_avg.max(0.1);
        }
    }

    /// Body bbox grown by the tails.
    pub fn bbox(&self) -> [f32; 4] {
        let mut r = self.shape.bbox();
        for t in &self.tails {
            t.geometry().grow_bbox(&mut r);
        }
        r
    }

    /// Every draggable control point: shape handles first, then per-tail
    /// tip/base pairs.
    pub fn handles(&self) -> Vec<([f32; 2], BalloonHandle)> {
        let mut out = Vec::new();
        match &self.shape {
            BalloonShape::Ellipse { center, radii } => {
                let [cx, cy] = *center;
                let [rx, ry] = *radii;
                out.push(([cx + rx, cy], BalloonHandle::Shape(0)));
                out.push(([cx, cy + ry], BalloonHandle::Shape(1)));
                out.push(([cx - rx, cy], BalloonHandle::Shape(2)));
                out.push(([cx, cy - ry], BalloonHandle::Shape(3)));
            }
            BalloonShape::RoundRect { rect, .. } => {
                out.push(([rect[0], rect[1]], BalloonHandle::Shape(0)));
                out.push(([rect[2], rect[1]], BalloonHandle::Shape(1)));
                out.push(([rect[2], rect[3]], BalloonHandle::Shape(2)));
                out.push(([rect[0], rect[3]], BalloonHandle::Shape(3)));
            }
            BalloonShape::Polygon { points, .. } => {
                for (i, p) in points.iter().enumerate() {
                    out.push((*p, BalloonHandle::Shape(i)));
                }
            }
        }
        for (i, t) in self.tails.iter().enumerate() {
            out.push((t.tip, BalloonHandle::TailTip(i)));
            out.push((t.base, BalloonHandle::TailBase(i)));
        }
        out
    }

    /// Nearest handle within `radius` of `p`.
    pub fn handle_near(&self, p: [f32; 2], radius: f32) -> Option<BalloonHandle> {
        let mut best: Option<(BalloonHandle, f32)> = None;
        for (pos, h) in self.handles() {
            let d = len(sub(pos, p));
            if d <= radius && best.map_or(true, |(_, bd)| d < bd) {
                best = Some((h, d));
            }
        }
        best.map(|(h, _)| h)
    }

    /// Drag `handle` to `p`. Ellipse handles resize one radius about the fixed
    /// center; rect corners keep the opposite corner anchored.
    ///
    /// Resizing the BODY re-anchors every tail's base to the boundary (the
    /// "tail tracks its balloon" rule): the base keeps its normalized body
    /// position while the tip stays where the user put it — the speaker does
    /// not move because the bubble got bigger. Ellipse and round-rect only:
    /// polygon anchors are free-form, so polygon drags leave tails absolute
    /// (whole-balloon move/transform still carries them).
    pub fn apply_handle(&mut self, handle: BalloonHandle, p: [f32; 2]) {
        match (handle, &mut self.shape) {
            (BalloonHandle::Shape(i), BalloonShape::Ellipse { center, radii }) => {
                let anchors: Vec<[f32; 2]> = self
                    .tails
                    .iter()
                    .map(|t| {
                        [
                            (t.base[0] - center[0]) / radii[0].max(1e-3),
                            (t.base[1] - center[1]) / radii[1].max(1e-3),
                        ]
                    })
                    .collect();
                let v = sub(p, *center);
                match i {
                    0 | 2 => radii[0] = v[0].abs().max(MIN_BALLOON_EXTENT * 0.5),
                    _ => radii[1] = v[1].abs().max(MIN_BALLOON_EXTENT * 0.5),
                }
                for (t, a) in self.tails.iter_mut().zip(anchors) {
                    t.base = [center[0] + a[0] * radii[0], center[1] + a[1] * radii[1]];
                }
            }
            (BalloonHandle::Shape(i), BalloonShape::RoundRect { rect, .. }) => {
                // Normalized tail bases in unit-rect coords before the edit.
                let anchors: Vec<[f32; 2]> = self
                    .tails
                    .iter()
                    .map(|t| {
                        let w = (rect[2] - rect[0]).max(1e-3);
                        let h = (rect[3] - rect[1]).max(1e-3);
                        [(t.base[0] - rect[0]) / w, (t.base[1] - rect[1]) / h]
                    })
                    .collect();
                // Opposite corner stays put; normalize afterwards.
                let (ax, ay) = match i {
                    0 => (rect[2], rect[3]),
                    1 => (rect[0], rect[3]),
                    2 => (rect[0], rect[1]),
                    _ => (rect[2], rect[1]),
                };
                *rect = [ax.min(p[0]), ay.min(p[1]), ax.max(p[0]), ay.max(p[1])];
                let w = (rect[2] - rect[0]).max(1e-3);
                let h = (rect[3] - rect[1]).max(1e-3);
                for (t, a) in self.tails.iter_mut().zip(anchors) {
                    t.base = [rect[0] + a[0] * w, rect[1] + a[1] * h];
                }
            }
            (BalloonHandle::Shape(i), BalloonShape::Polygon { points, .. }) => {
                if let Some(v) = points.get_mut(i) {
                    *v = p;
                }
            }
            (BalloonHandle::TailTip(i), _) => {
                if let Some(t) = self.tails.get_mut(i) {
                    t.tip = p;
                }
            }
            (BalloonHandle::TailBase(i), _) => {
                if let Some(t) = self.tails.get_mut(i) {
                    t.base = p;
                }
            }
        }
    }

    /// Closest point on any polygon EDGE within `tol` — the Object tool's
    /// "insert an anchor here" hit test. Returns the segment index the new
    /// anchor should be inserted AFTER (i.e. `points[0..=seg]` precede it).
    /// Analytic bodies (ellipse/round-rect) have no editable anchors: `None`.
    pub fn edge_point_near(&self, p: [f32; 2], tol: f32) -> Option<(usize, [f32; 2])> {
        let BalloonShape::Polygon { points, .. } = &self.shape else {
            return None;
        };
        let n = points.len();
        let mut best: Option<(usize, [f32; 2], f32)> = None;
        for i in 0..n {
            let a = points[i];
            let b = points[(i + 1) % n];
            let ab = sub(b, a);
            let l2 = dot(ab, ab);
            let t = if l2 < 1e-9 {
                0.0
            } else {
                (dot(sub(p, a), ab) / l2).clamp(0.0, 1.0)
            };
            let q = [a[0] + ab[0] * t, a[1] + ab[1] * t];
            let d = len(sub(p, q));
            if d <= tol && best.map_or(true, |(_, _, bd)| d < bd) {
                best = Some((i, q, d));
            }
        }
        best.map(|(i, q, _)| (i, q))
    }

    /// Insert an anchor after segment `seg` at `p` (smooth: corner = false,
    /// width = the mean of its new neighbours). Drawn bodies only.
    pub fn insert_anchor(&mut self, seg: usize, p: [f32; 2]) -> bool {
        let BalloonShape::Polygon {
            points,
            widths,
            corners,
        } = &mut self.shape
        else {
            return false;
        };
        if points.len() < 3 || seg >= points.len() {
            return false;
        }
        let w = if widths.is_empty() {
            None
        } else if widths.len() == points.len() {
            Some((widths[seg] + widths[(seg + 1) % points.len()]) * 0.5)
        } else {
            widths.first().copied()
        };
        let at = seg + 1;
        points.insert(at, p);
        match w {
            Some(w) => {
                // `Vec::insert` panics past the end; the empty-or-aligned
                // invariant keeps at <= widths.len(), but a malformed
                // shorter vec degrades to append instead of panicking
                // (auditor round 33, minor).
                if at <= widths.len() {
                    widths.insert(at, w.clamp(0.0, 1.0));
                } else {
                    widths.push(w.clamp(0.0, 1.0));
                }
            }
            None => {}
        }
        if corners.len() == points.len() - 1 {
            // A corner flag somewhere exists: keep the vecs aligned. The new
            // anchor is smooth by design.
            corners.insert(at, false);
        }
        true
    }

    /// Delete anchor `i`. Refused below three anchors or off the polygon.
    pub fn delete_anchor(&mut self, i: usize) -> bool {
        let BalloonShape::Polygon {
            points,
            widths,
            corners,
        } = &mut self.shape
        else {
            return false;
        };
        if points.len() <= 3 || i >= points.len() {
            return false;
        }
        points.remove(i);
        if widths.len() > i {
            widths.remove(i);
        }
        if corners.len() > i {
            corners.remove(i);
        }
        true
    }

    /// Toggle the corner (sharp kink) flag of anchor `i`. Materializes the
    /// flag vec (default: every anchor smooth) on first use.
    pub fn toggle_anchor_corner(&mut self, i: usize) -> bool {
        let BalloonShape::Polygon {
            points, corners, ..
        } = &mut self.shape
        else {
            return false;
        };
        if i >= points.len() {
            return false;
        }
        if corners.len() != points.len() {
            // Pad to full length (the aligned-vecs invariant every editor
            // keeps): empty = all-smooth; a partial vec (should not exist —
            // every writer keeps alignment) is grown with smooth anchors.
            corners.resize(points.len(), false);
        }
        corners[i] = !corners[i];
        true
    }

    /// Delete tail `i` (CSP: remove a bubble's tail without touching the
    /// body).
    pub fn delete_tail(&mut self, i: usize) -> bool {
        if i < self.tails.len() {
            self.tails.remove(i);
            true
        } else {
            false
        }
    }

    /// Big enough to keep / commit.
    pub fn is_valid(&self) -> bool {
        match &self.shape {
            BalloonShape::Ellipse { radii, .. } => {
                radii[0] >= MIN_BALLOON_EXTENT * 0.5 && radii[1] >= MIN_BALLOON_EXTENT * 0.5
            }
            BalloonShape::RoundRect { rect, .. } => {
                (rect[2] - rect[0]).abs() >= MIN_BALLOON_EXTENT
                    && (rect[3] - rect[1]).abs() >= MIN_BALLOON_EXTENT
            }
            BalloonShape::Polygon { points, .. } => {
                if points.len() < 3 {
                    return false;
                }
                let mut area = 0.0;
                for i in 0..points.len() {
                    let a = points[i];
                    let b = points[(i + 1) % points.len()];
                    area += a[0] * b[1] - a[1] * b[0];
                }
                (area * 0.5).abs() >= MIN_BALLOON_EXTENT * MIN_BALLOON_EXTENT
            }
        }
    }
}

/// Ramer–Douglas–Peucker polyline simplification — turns a freehand balloon
/// drag (hundreds of samples) into a polygon worth editing by its vertices.
pub fn simplify_polyline(pts: &[[f32; 2]], epsilon: f32) -> Vec<[f32; 2]> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    fn seg_dist(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
        let ab = sub(b, a);
        let l2 = dot(ab, ab);
        let t = if l2 < 1e-9 {
            0.0
        } else {
            (dot(sub(p, a), ab) / l2).clamp(0.0, 1.0)
        };
        len(sub(p, [a[0] + ab[0] * t, a[1] + ab[1] * t]))
    }
    fn rdp(pts: &[[f32; 2]], eps: f32, out: &mut Vec<[f32; 2]>) {
        let (a, b) = (pts[0], pts[pts.len() - 1]);
        let mut worst = (0usize, 0.0f32);
        for (i, p) in pts
            .iter()
            .enumerate()
            .skip(1)
            .take(pts.len().saturating_sub(2))
        {
            let d = seg_dist(*p, a, b);
            if d > worst.1 {
                worst = (i, d);
            }
        }
        if worst.1 > eps && pts.len() > 2 {
            rdp(&pts[..=worst.0], eps, out);
            out.pop(); // shared point
            rdp(&pts[worst.0..], eps, out);
        } else {
            out.push(a);
            out.push(b);
        }
    }
    let mut out = Vec::new();
    rdp(pts, epsilon.max(0.1), &mut out);
    out
}

/// Cubic Hermite spline between `p1`→`p2` with tangents `m1`, `m2`
/// (Catmull-Rom is the `m = neighbour-diff / 2` special case).
fn hermite(p1: [f32; 2], m1: [f32; 2], p2: [f32; 2], m2: [f32; 2], t: f32) -> [f32; 2] {
    let t2 = t * t;
    let t3 = t2 * t;
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;
    [
        h00 * p1[0] + h10 * m1[0] + h01 * p2[0] + h11 * m2[0],
        h00 * p1[1] + h10 * m1[1] + h01 * p2[1] + h11 * m2[1],
    ]
}

/// Tessellate the drawn balloon's anchors into a dense closed polyline:
/// a Catmull-Rom spline through the control points (smooth through every
/// anchor), with a corner-anchor's tangents zeroed so the curve dead-stops
/// there (CSP's two anchor types: smooth vs corner — a corner reads as a
/// hard kink because the curve arrives and leaves without direction).
/// Per-vertex pressure lerps along each segment. `widths`/`corners` may be
/// empty (uniform, all smooth).
pub fn tessellate_closed(
    points: &[[f32; 2]],
    corners: &[bool],
    widths: &[f32],
) -> (Vec<[f32; 2]>, Vec<f32>) {
    let n = points.len();
    if n < 3 {
        return (points.to_vec(), (0..n).map(|i| w_at(widths, i)).collect());
    }
    let is_corner = |i: usize| corners.get(i).copied().unwrap_or(false);
    let mut pts: Vec<[f32; 2]> = Vec::with_capacity(n * 8);
    let mut ws: Vec<f32> = Vec::with_capacity(n * 8);
    // Target ~6px per spline segment step, capped so a huge balloon stays
    // cheap and a tiny one keeps a minimum of curve resolution.
    const STEP_PX: f32 = 6.0;
    for i in 0..n {
        let p1 = points[i];
        let p2 = points[(i + 1) % n];
        let m1 = if is_corner(i) {
            [0.0, 0.0]
        } else {
            // Catmull-Rom tangent at p1 = (p2 - p0) / 2.
            mul(sub(p2, points[(i + n - 1) % n]), 0.5)
        };
        let m2 = if is_corner((i + 1) % n) {
            [0.0, 0.0]
        } else {
            mul(sub(points[(i + 2) % n], p1), 0.5)
        };
        let steps = (len(sub(p2, p1)) / STEP_PX).ceil().clamp(2.0, 24.0) as usize;
        for k in 0..steps {
            let t = k as f32 / steps as f32;
            pts.push(hermite(p1, m1, p2, m2, t));
            let wa = w_at(widths, i);
            let wb = w_at(widths, (i + 1) % n);
            ws.push(wa + (wb - wa) * t);
        }
    }
    (pts, ws)
}

/// Rows 84/85 (the Figure ▸ Curve sub tool): [`tessellate_closed`]'s OPEN
/// twin — a dense polyline along the Catmull-Rom spline through a chain of
/// clicked points, first point to last, nothing joined up. The curve tool
/// hands this to the brush, so the numbers differ from the balloon's in one
/// way that matters: the step cap is high, because a 900 px hair sweep is
/// exactly the mark this tool exists for and 24 chords across it would ink
/// a visible polygon.
///
/// End tangents are ONE-SIDED (the chord itself), so two points ink the
/// straight line you clicked and three ink an arc through the middle one —
/// no phantom control points, no overshoot past the ends.
pub fn tessellate_open(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let n = points.len();
    if n < 3 {
        return points.to_vec();
    }
    const STEP_PX: f32 = 4.0;
    let mut out: Vec<[f32; 2]> = Vec::with_capacity(n * 16);
    for i in 0..n - 1 {
        let p1 = points[i];
        let p2 = points[i + 1];
        let prev = if i == 0 { p1 } else { points[i - 1] };
        let next = if i + 2 < n { points[i + 2] } else { p2 };
        let m1 = mul(sub(p2, prev), 0.5);
        let m2 = mul(sub(next, p1), 0.5);
        let steps = (len(sub(p2, p1)) / STEP_PX).ceil().clamp(2.0, 512.0) as usize;
        for k in 0..steps {
            out.push(hermite(p1, m1, p2, m2, k as f32 / steps as f32));
        }
    }
    // The last control point verbatim: a spline the artist placed must
    // START and END where they clicked, whatever f32 does in between.
    out.push(points[n - 1]);
    out
}

/// `FG-016` (Figure ▸ Continuous curve ▸ Alt+tap an anchor): [`tessellate_open`]
/// with CORNERS — anchors the artist has marked as creases rather than as
/// points the spline sweeps smoothly through.
///
/// The trick is that a corner is not a different kind of interpolation, it is
/// a different set of NEIGHBOURS. Catmull-Rom takes its tangent at a point
/// from the chord between the point before it and the point after it, so a
/// crease is what you get when the run simply ENDS there: the run before the
/// corner tessellates with the corner as its last point (one-sided end
/// tangent), the run after it starts there (one-sided start tangent), and the
/// two arrive at the same coordinate from different directions. Hence "split
/// into smooth runs, tessellate each, join at the corner" rather than any
/// special-casing inside the Hermite loop.
///
/// Endpoint flags are ignored on purpose: `tessellate_open` already uses a
/// one-sided tangent at both ends, so the first and last anchors are creases
/// by construction and marking them changes nothing. They are still allowed to
/// carry the flag because the artist marks the point they JUST placed — which
/// is the last one at that moment, and becomes an interior one the next click.
///
/// `corners` shorter than `points` reads as "smooth from there on", so a
/// caller that has not built flags at all can pass `&[]`.
///
/// This is deliberately NOT [`tessellate_closed`]'s corner rule (zero the
/// tangent). Zeroing works there because a closed balloon has two-sided
/// tangents everywhere, so killing one is the only way to make a kink; here
/// the ends are already one-sided, and re-using them at a crease is both
/// simpler and truer to what the artist asked for — two anchors in a row
/// marked as corners give back the straight chord they clicked, which a
/// zeroed tangent on a one-sided end would not.
pub fn tessellate_open_corners(points: &[[f32; 2]], corners: &[bool]) -> Vec<[f32; 2]> {
    let n = points.len();
    // Interior creases only — see above. Without one there is nothing to
    // split, so this is exactly the smooth spline and shares its code.
    let breaks: Vec<usize> = (1..n.saturating_sub(1))
        .filter(|&i| corners.get(i).copied().unwrap_or(false))
        .collect();
    if breaks.is_empty() {
        return tessellate_open(points);
    }
    let mut out: Vec<[f32; 2]> = Vec::with_capacity(n * 16);
    let mut start = 0usize;
    for end in breaks.into_iter().chain(std::iter::once(n - 1)) {
        let run = tessellate_open(&points[start..=end]);
        // The joint is the corner itself, and the previous run already ended
        // ON it (`tessellate_open` pushes its last control point verbatim).
        // Skipping the duplicate keeps the brush from double-dabbing there,
        // which on a low-flow nib is a visible dark bead at every crease.
        out.extend(if start == 0 { &run[..] } else { &run[1..] });
        start = end;
    }
    out
}

/// `FG-002` (the Figure ▸ Curve sub tool): the quadratic Bezier that runs
/// from `a` to `b` and passes exactly THROUGH `through` at its own midpoint.
///
/// This is what makes CSP's two-stage curve gesture need no control-point
/// model at all. Stage one drags the straight baseline `a`→`b`; stage two
/// moves the pointer and the curve follows it, because the pointer IS a
/// point on the curve rather than an off-line Bezier handle nobody can aim.
/// The control point is solved for, not steered:
///
/// `B(½) = ¼a + ½c + ¼b = through`  ⇒  `c = 2·through − ½a − ½b`
///
/// so the pointer sits on the curve by construction. `through` at the
/// baseline's midpoint gives back the straight line, which is why releasing
/// stage one and clicking without moving inks exactly what stage one showed.
pub fn quad_through(a: [f32; 2], b: [f32; 2], through: [f32; 2]) -> Vec<[f32; 2]> {
    let c = [
        2.0 * through[0] - 0.5 * a[0] - 0.5 * b[0],
        2.0 * through[1] - 0.5 * a[1] - 0.5 * b[1],
    ];
    // Step count from the control polygon, not the chord: a hard bend is
    // much longer than the baseline under it and would ink as facets.
    const STEP_PX: f32 = 4.0;
    let span = len(sub(c, a)) + len(sub(b, c));
    let steps = (span / STEP_PX).ceil().clamp(2.0, 1024.0) as usize;
    let mut out = Vec::with_capacity(steps + 1);
    for k in 0..steps {
        let t = k as f32 / steps as f32;
        let u = 1.0 - t;
        out.push([
            u * u * a[0] + 2.0 * u * t * c[0] + t * t * b[0],
            u * u * a[1] + 2.0 * u * t * c[1] + t * t * b[1],
        ]);
    }
    // The dragged end verbatim — same rule as `tessellate_open`.
    out.push(b);
    out
}

fn mul(a: [f32; 2], k: f32) -> [f32; 2] {
    [a[0] * k, a[1] * k]
}

fn w_at(widths: &[f32], i: usize) -> f32 {
    widths.get(i).copied().unwrap_or(1.0).clamp(0.0, 1.0)
}

/// Signed distance to a closed polygon PLUS the interpolated per-vertex
/// value (pen pressure) at the nearest boundary point. Same algorithm as
/// [`polygon_sdf`] with the winning edge tracked. Empty `ws` = uniform 1.0.
fn polygon_sdf_w(pts: &[[f32; 2]], ws: &[f32], p: [f32; 2]) -> (f32, f32) {
    let n = pts.len();
    if n < 3 {
        return (f32::INFINITY, 1.0);
    }
    let mut best_d2 = f32::INFINITY;
    let mut best_w = 1.0;
    let mut s = 1.0f32;
    let mut j = n - 1;
    for i in 0..n {
        let e = sub(pts[j], pts[i]);
        let w = sub(p, pts[i]);
        let t = if dot(e, e) > 1e-12 {
            (dot(w, e) / dot(e, e)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let b = [w[0] - e[0] * t, w[1] - e[1] * t];
        let d2 = dot(b, b);
        if d2 < best_d2 {
            best_d2 = d2;
            let wa = w_at(ws, i);
            let wb = w_at(ws, j);
            best_w = wa + (wb - wa) * t;
        }
        let c = [
            p[1] >= pts[i][1],
            p[1] < pts[j][1],
            e[0] * w[1] > e[1] * w[0],
        ];
        if c.iter().all(|&x| x) || c.iter().all(|&x| !x) {
            s = -s;
        }
        j = i;
    }
    (s * best_d2.sqrt(), best_w)
}

/// RDP simplification that carries each kept anchor's pressure along.
pub fn simplify_anchors(
    pts: &[[f32; 2]],
    pressures: &[f32],
    epsilon: f32,
) -> (Vec<[f32; 2]>, Vec<f32>) {
    let simple = simplify_polyline(pts, epsilon);
    // `simplify_polyline` keeps a subset of the input points in order — pair
    // each kept point with the nearest original index's pressure.
    let mut out_p = Vec::with_capacity(simple.len());
    let mut out_w = Vec::with_capacity(simple.len());
    let mut next = 0usize;
    for sp in &simple {
        while next < pts.len() && (pts[next][0] - sp[0]).abs() + (pts[next][1] - sp[1]).abs() > 1e-3
        {
            next += 1;
        }
        out_p.push(*sp);
        out_w.push(pressures.get(next).copied().unwrap_or(1.0).clamp(0.0, 1.0));
        next += 1;
    }
    (out_p, out_w)
}

impl BalloonSet {
    /// Current index of the balloon with stable id `id`.
    pub fn index_of_id(&self, id: u64) -> Option<usize> {
        if id == 0 {
            return None;
        }
        self.balloons.iter().position(|b| b.id == id)
    }

    /// `TextSet::mint_ids`'s twin — remint `0` and duplicates, first
    /// occurrence keeps the id. The Document's commit doors call this.
    pub fn mint_ids(&mut self) {
        let mut seen = std::collections::HashSet::new();
        for b in &mut self.balloons {
            if b.id == 0 || !seen.insert(b.id) {
                b.id = crate::doc::mint_id();
                seen.insert(b.id);
            }
        }
    }

    pub fn new(border_px: f32) -> Self {
        Self {
            balloons: Vec::new(),
            border_px,
            pressure_width: false,
        }
    }

    /// Balloon containing `p` (body or tail), topmost in list order.
    pub fn balloon_at(&self, p: [f32; 2]) -> Option<usize> {
        self.balloons.iter().rposition(|b| b.contains(p))
    }

    /// Rasterize to sparse tiles: each balloon's fill inside the union of all
    /// balloons, its AA outline on the boundary, transparent elsewhere. Only
    /// tiles near a balloon are touched — a page is mostly empty and stays
    /// that way. Tiles fully inside a body share **one** `Arc` per distinct
    /// flat fill (one, for the overwhelmingly common all-white page).
    ///
    /// Colour is per BALLOON, not per set, and the union's per-pixel winner
    /// (the nearest body/tail) decides which balloon's ink a pixel gets —
    /// the same rule that already picked the outline width. Two overlapping
    /// balloons of different colours therefore meet along the union's ridge,
    /// which is where CSP puts the seam too.
    pub fn rasterize(&self, size: (u32, u32)) -> HashMap<TileIdx, Arc<Tile>> {
        let mut out = HashMap::new();
        if self.balloons.is_empty() {
            return out;
        }
        let reach = self.border_px * 0.5 * max_width_scale(self) + 1.0;
        let tile_r = (TILE_SIZE as f32) * 0.5 * std::f32::consts::SQRT_2;
        // Drawn balloons ink their outline thinner where the pen was light
        // (`pressure_width`): border 35% of nominal at zero pressure, 100% at
        // full pressure. The per-balloon `width_scale` multiplies the whole
        // thing at RENDER time — the stored pressure widths are data, never
        // edited by the correct-width UI.
        let border_of = |i: usize, pr: f32| {
            let base = if self.pressure_width {
                self.border_px * (0.35 + 0.65 * pr)
            } else {
                self.border_px
            };
            base * self.balloons[i].width_scale.max(0.0)
        };

        // Tessellate drawn bodies ONCE (the per-pixel loop must not rebuild
        // splines). `None` = analytic body.
        let dense: Vec<Option<(Vec<[f32; 2]>, Vec<f32>)>> =
            self.balloons.iter().map(|b| b.shape.dense()).collect();
        // Same for the tails: a bent/spiked/thought tail is a tessellated
        // outline, and re-deriving it per pixel would be a per-pixel Vec.
        let tail_geoms: Vec<Vec<TailGeom>> = self
            .balloons
            .iter()
            .map(|b| b.tails.iter().map(|t| t.geometry()).collect())
            .collect();
        // Per-balloon ink, hoisted out of the loop (line, fill) in 0..1.
        let ink: Vec<([f32; 3], [f32; 3])> = self
            .balloons
            .iter()
            .map(|b| (rgb_f(b.line_color), rgb_f(b.fill_color)))
            .collect();

        // Inflated bboxes decide which tiles each balloon can influence.
        let boxes: Vec<[f32; 4]> = self
            .balloons
            .iter()
            .map(|b| {
                let r = b.bbox();
                [
                    r[0] - reach - 1.0,
                    r[1] - reach - 1.0,
                    r[2] + reach + 1.0,
                    r[3] + reach + 1.0,
                ]
            })
            .collect();

        let tiles_x = (size.0 as usize).div_ceil(TILE_SIZE) as i32;
        let tiles_y = (size.1 as usize).div_ceil(TILE_SIZE) as i32;
        // Solid-interior tiles, shared by their premultiplied pixel value —
        // one entry for a page of ordinary white balloons.
        let mut fills: HashMap<[u16; 4], Arc<Tile>> = HashMap::new();

        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                let idx = TileIdx::new(tx, ty);
                let (ox, oy) = idx.origin();
                let (tx0, ty0) = (ox as f32, oy as f32);
                let (tx1, ty1) = (tx0 + TILE_SIZE as f32, ty0 + TILE_SIZE as f32);
                let center = [tx0 + TILE_SIZE as f32 * 0.5, ty0 + TILE_SIZE as f32 * 0.5];

                let mut near: Vec<usize> = Vec::new();
                for (i, bb) in boxes.iter().enumerate() {
                    if bb[0] <= tx1 && bb[2] >= tx0 && bb[1] <= ty1 && bb[3] >= ty0 {
                        near.push(i);
                    }
                }
                if near.is_empty() {
                    continue;
                }

                // Deep inside some body ⇒ the union SDF is below -reach across
                // the whole tile: one flat colour, shared allocation.
                //
                // Only when every balloon that could win here paints the SAME
                // flat fill: a screened fill varies per pixel and two
                // different fill colours have a seam somewhere in the tile,
                // so both fall through to the per-pixel path. An all-default
                // page satisfies this trivially and keeps the shortcut.
                let head = &self.balloons[near[0]];
                let flat = near.iter().all(|&i| {
                    let b = &self.balloons[i];
                    b.fill_tone.is_none()
                        && b.fill_color == head.fill_color
                        && b.fill_opacity == head.fill_opacity
                });
                let deep = flat
                    && near
                        .iter()
                        .any(|&i| self.balloons[i].shape.inside_depth(center) > tile_r + reach);
                if deep {
                    let a = head.fill_opacity.clamp(0.0, 1.0);
                    let f = rgb_f(head.fill_color);
                    let quad = [
                        (f[0] * a * FIX15_ONE as f32).round() as u16,
                        (f[1] * a * FIX15_ONE as f32).round() as u16,
                        (f[2] * a * FIX15_ONE as f32).round() as u16,
                        (a * FIX15_ONE as f32).round() as u16,
                    ];
                    // A fully transparent interior is no tile at all.
                    if quad[3] > 0 {
                        let t = fills
                            .entry(quad)
                            .or_insert_with(|| {
                                let mut t = Tile::new_transparent();
                                for px in t.data_mut().chunks_exact_mut(4) {
                                    px.copy_from_slice(&quad);
                                }
                                Arc::new(t)
                            })
                            .clone();
                        out.insert(idx, t);
                    }
                    continue;
                }

                let mut tile = Tile::new_transparent();
                let data = tile.data_mut();
                let mut any = false;
                for py in 0..TILE_SIZE {
                    for px in 0..TILE_SIZE {
                        let p = [tx0 + px as f32 + 0.5, ty0 + py as f32 + 0.5];
                        let mut d = f32::INFINITY;
                        let mut pr = 1.0;
                        let mut winner = near[0];
                        for &i in &near {
                            // Body, then the tails unioned on top (uniform
                            // pressure — a tail has no recorded pen widths).
                            let (mut td, mut tpr) = match &dense[i] {
                                Some((pts, ws)) => polygon_sdf_w(pts, ws, p),
                                None => self.balloons[i].shape.sdf_w(p),
                            };
                            for g in &tail_geoms[i] {
                                let ud = g.sdf(p);
                                if ud < td {
                                    td = ud;
                                    tpr = 1.0;
                                }
                            }
                            if td < d {
                                d = td;
                                pr = tpr;
                                winner = i;
                            }
                        }
                        let inside = (0.5 - d).clamp(0.0, 1.0);
                        let bw = border_of(winner, pr);
                        let border = (bw * 0.5 + 0.5 - d.abs()).clamp(0.0, 1.0);
                        // Outline OVER fill, premultiplied. With the defaults
                        // (opaque black line, opaque white fill, no screen)
                        // every multiply below is by exactly 1.0 or 0.0 and
                        // this reduces, bit for bit, to the two hardcoded
                        // lines it replaced.
                        let b = &self.balloons[winner];
                        let al = border * b.line_opacity;
                        let mut af = inside * b.fill_opacity;
                        if let Some(t) = &b.fill_tone {
                            af *= t.coverage(p);
                        }
                        let alpha = al + af * (1.0 - al);
                        if alpha <= 0.0 {
                            continue;
                        }
                        let fpre = af * (1.0 - al);
                        let (lc, fc) = &ink[winner];
                        any = true;
                        let o = Tile::offset(px, py);
                        for ch in 0..3 {
                            data[o + ch] =
                                ((lc[ch] * al + fc[ch] * fpre) * FIX15_ONE as f32).round() as u16;
                        }
                        data[o + 3] = (alpha * FIX15_ONE as f32).round() as u16;
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

/// Everything about how a balloon is INKED, with no geometry in it —
/// `B-001`–`004` plus `C-04x`'s screened fill.
///
/// It exists so one widget serves both halves of the row: the Balloon tool
/// carries a `BalloonInk` as the settings a *new* bubble is born with
/// (`C-039`–`048`, "create balloon options"), and the Object tool edits the
/// ink of the bubble already on the page — same fields, same code, so the two
/// can never disagree about what "40 % fill" means.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BalloonInk {
    pub line_color: [u8; 3],
    pub fill_color: [u8; 3],
    pub line_opacity: f32,
    pub fill_opacity: f32,
    pub fill_tone: Option<BalloonTone>,
}

impl Default for BalloonInk {
    fn default() -> Self {
        Self {
            line_color: black(),
            fill_color: white(),
            line_opacity: 1.0,
            fill_opacity: 1.0,
            fill_tone: None,
        }
    }
}

impl BalloonShape {
    /// True when [`Balloon::transform_around`] can TURN this body exactly.
    ///
    /// An ellipse and a rounded rect are axis-aligned by definition: a
    /// rotation collapses to their transformed extents, so an ellipse with no
    /// tail comes out of a rotate drag completely unchanged. Only a drawn
    /// (anchor) bubble carries a real angle. Anything that wants to *follow* a
    /// balloon's rotation — the lettering inside it, TRIAGE 134 — has to ask
    /// this first, or it turns while the bubble stands still.
    pub fn rotates_exactly(&self) -> bool {
        matches!(self, BalloonShape::Polygon { .. })
    }
}

impl Balloon {
    /// This balloon's ink, detached from its shape.
    pub fn ink(&self) -> BalloonInk {
        BalloonInk {
            line_color: self.line_color,
            fill_color: self.fill_color,
            line_opacity: self.line_opacity,
            fill_opacity: self.fill_opacity,
            fill_tone: self.fill_tone,
        }
    }

    /// Repaint without reshaping — the geometry, the tails and `width_scale`
    /// are untouched.
    pub fn set_ink(&mut self, ink: BalloonInk) {
        self.line_color = ink.line_color;
        self.fill_color = ink.fill_color;
        self.line_opacity = ink.line_opacity.clamp(0.0, 1.0);
        self.fill_opacity = ink.fill_opacity.clamp(0.0, 1.0);
        self.fill_tone = ink.fill_tone;
    }

    /// Give every tail the same look — the Tool Property panel edits a
    /// balloon, not a tail, and a bubble with two tails wants them matching.
    pub fn set_tail_style(&mut self, kind: TailKind, bend: f32) {
        for t in &mut self.tails {
            t.kind = kind;
            t.bend = bend;
        }
    }

    /// The style shared by every tail, or `None` when they disagree / there
    /// are no tails — what the panel shows.
    pub fn tail_style(&self) -> Option<(TailKind, f32)> {
        let first = self.tails.first()?;
        let s = (first.kind, first.bend);
        self.tails
            .iter()
            .all(|t| (t.kind, t.bend) == s)
            .then_some(s)
    }
}

/// TRIAGE 134 (JP #4, 258 votes) — **turn a balloon and its lettering turns
/// with it, still editable.**
///
/// Every item in `texts` whose centre lies inside `body` is carried by the
/// rigid motion "rotate `rad` about `pivot`": the centre swings around the
/// pivot and the item's own [`crate::text::TextItem::rotation`] gains `rad`,
/// which is the angle DirectWrite lays the glyphs out at. Nothing is
/// rasterized and no glyph is baked — the string, the style runs, the ruby,
/// the 縦中横 runs and the caret all survive the turn. That is the entire
/// complaint the row records: in CSP you must rasterize a text layer to
/// rotate it, and the lettering can never be edited again.
///
/// `body` must be the balloon as it was BEFORE the rotation (the drag keeps
/// the original for exactly this reason) — after it, a text that swung with
/// the bubble may no longer test as inside.
///
/// The sprite cache of a carried item is dropped: those pixels were shaped at
/// the old angle. The caller re-renders through the text engine; an item with
/// no cache contributes nothing to the raster until it does.
///
/// Returns the indices carried, so a caller can skip a no-op commit.
pub fn rotate_texts_in(
    body: &Balloon,
    texts: &mut crate::text::TextSet,
    pivot: [f32; 2],
    rad: f32,
) -> Vec<usize> {
    if rad == 0.0 {
        return Vec::new();
    }
    let (sn, cs) = rad.sin_cos();
    let mut moved = Vec::new();
    for (i, t) in texts.texts.iter_mut().enumerate() {
        let c = t.center();
        if !body.contains(c) {
            continue;
        }
        let (dx, dy) = (c[0] - pivot[0], c[1] - pivot[1]);
        let to = [pivot[0] + dx * cs - dy * sn, pivot[1] + dx * sn + dy * cs];
        t.pos = [t.pos[0] + to[0] - c[0], t.pos[1] + to[1] - c[1]];
        t.rotation += rad;
        t.cache = None;
        moved.push(i);
    }
    moved
}

/// The move's half of [`rotate_texts_in`]: a translated bubble takes its
/// lettering along by the same geometric pairing — a text is carried when
/// its centre was inside the ORIGINAL body. No reshaping: translation
/// never changes a glyph's layout, so the shaped cache stays valid.
pub fn translate_texts_in(
    body: &Balloon,
    texts: &mut crate::text::TextSet,
    d: [f32; 2],
) -> Vec<usize> {
    if d[0] == 0.0 && d[1] == 0.0 {
        return Vec::new();
    }
    let mut moved = Vec::new();
    for (i, t) in texts.texts.iter_mut().enumerate() {
        if !body.contains(t.center()) {
            continue;
        }
        t.pos = [t.pos[0] + d[0], t.pos[1] + d[1]];
        moved.push(i);
    }
    moved
}

/// The resize's half of [`rotate_texts_in`]: lettering inside a RESIZED
/// bubble keeps its same relative position — the centre's fraction of the
/// old bbox lands at that fraction of the new one, so centred lettering
/// stays centred and a deliberately off-centre shout stays off-centre.
/// A text is carried when its centre was inside the ORIGINAL body.
///
/// The type size is deliberately NOT scaled: the letterer's font is his
/// call, and a bubble stretched to fit a line does not get to change the
/// line. No reshaping means the shaped cache stays valid, as with the move.
pub fn scale_texts_in(
    body: &Balloon,
    texts: &mut crate::text::TextSet,
    new_bbox: [f32; 4],
) -> Vec<usize> {
    let b0 = body.bbox();
    let (w0, h0) = (b0[2] - b0[0], b0[3] - b0[1]);
    let (w1, h1) = (new_bbox[2] - new_bbox[0], new_bbox[3] - new_bbox[1]);
    if w0 < 1e-3 || h0 < 1e-3 || w1 < 1e-3 || h1 < 1e-3 {
        return Vec::new();
    }
    let mut moved = Vec::new();
    for (i, t) in texts.texts.iter_mut().enumerate() {
        let c = t.center();
        if !body.contains(c) {
            continue;
        }
        let to = [
            new_bbox[0] + (c[0] - b0[0]) / w0 * w1,
            new_bbox[1] + (c[1] - b0[1]) / h0 * h1,
        ];
        t.pos = [t.pos[0] + to[0] - c[0], t.pos[1] + to[1] - c[1]];
        moved.push(i);
    }
    moved
}

// --- fit a balloon to its lettering (ROADMAP good-first-issue #1) ----------

/// Breathing room left around the lettering, in **ems of the text's own type
/// size**. Proportional on purpose: a 9 pt bubble and a 24 pt shout want the
/// same *optical* margin, not the same pixel count, and the em is the only
/// number that tracks the type through a dpi change.
pub const FIT_PAD_EM: f32 = 0.75;

/// Minimum ratio between the balloon's long and short axis along the text's
/// writing direction. Tategaki (vertical) lettering gets a bubble that is at
/// least this much taller than it is wide, yokogaki one at least this much
/// wider than tall — the printed manga convention, which a single short
/// column or a one-word line would otherwise round off into a circle.
/// Deliberately mild: for ordinary multi-line lettering the measured extent
/// already exceeds it and this clamp does nothing.
const FIT_ASPECT: f32 = 1.05;

impl BalloonShape {
    /// The centre a fit keeps FIXED. The polygon bbox pads symmetrically, so
    /// the padding cancels here.
    fn center(&self) -> [f32; 2] {
        let r = self.bbox();
        [(r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5]
    }

    /// Widest/tallest the body's own anchors reach (no spline padding) —
    /// the polygon fit's "how big am I now".
    fn raw_extent(&self) -> [f32; 2] {
        match self {
            BalloonShape::Ellipse { radii, .. } => [radii[0] * 2.0, radii[1] * 2.0],
            BalloonShape::RoundRect { rect, .. } => {
                [(rect[2] - rect[0]).abs(), (rect[3] - rect[1]).abs()]
            }
            BalloonShape::Polygon { points, .. } => {
                let mut r = [
                    f32::INFINITY,
                    f32::INFINITY,
                    f32::NEG_INFINITY,
                    f32::NEG_INFINITY,
                ];
                for p in points {
                    r[0] = r[0].min(p[0]);
                    r[1] = r[1].min(p[1]);
                    r[2] = r[2].max(p[0]);
                    r[3] = r[3].max(p[1]);
                }
                [(r[2] - r[0]).max(0.0), (r[3] - r[1]).max(0.0)]
            }
        }
    }
}

impl Balloon {
    /// Scale the BODY about `c` and drag every tail's **base** along with it,
    /// leaving the tips and the tail widths alone.
    ///
    /// That split is the "tail tracks its balloon" rule
    /// [`Balloon::apply_handle`] already keeps for a resize drag, written once
    /// more here because a fit is a resize: the base has to stay welded to the
    /// boundary or the tail floats off the bubble, and the tip must NOT move
    /// because the speaker did not move just because the bubble did.
    fn scale_body_about(&mut self, c: [f32; 2], sx: f32, sy: f32) {
        let map = |p: &mut [f32; 2]| {
            p[0] = c[0] + (p[0] - c[0]) * sx;
            p[1] = c[1] + (p[1] - c[1]) * sy;
        };
        match &mut self.shape {
            BalloonShape::Ellipse { center, radii } => {
                map(center);
                radii[0] = (radii[0] * sx).abs();
                radii[1] = (radii[1] * sy).abs();
            }
            BalloonShape::RoundRect { rect, .. } => {
                let mut a = [rect[0], rect[1]];
                let mut b = [rect[2], rect[3]];
                map(&mut a);
                map(&mut b);
                *rect = [
                    a[0].min(b[0]),
                    a[1].min(b[1]),
                    a[0].max(b[0]),
                    a[1].max(b[1]),
                ];
            }
            BalloonShape::Polygon { points, .. } => {
                for p in points.iter_mut() {
                    map(p);
                }
            }
        }
        for t in &mut self.tails {
            map(&mut t.base);
        }
    }

    /// **Fit this bubble around `text`** — the ROADMAP's good-first-issue #1.
    ///
    /// `em_px` is the text's type size in canvas px (`mn_text::font_px`); the
    /// margin is [`FIT_PAD_EM`] of it, so the padding is proportional to the
    /// lettering rather than a constant that is fat at 9 pt and thin at 24.
    ///
    /// What is preserved, and why:
    ///
    /// * **The centre.** The bubble is sized about where it already sits — the
    ///   artist placed it against the art, and a fit that teleported it onto
    ///   the text's centre would move it off the speaker. Lettering that sits
    ///   off-centre therefore grows the bubble on that side rather than
    ///   sliding it, which is also what keeps the tail attached: with the
    ///   centre fixed, the tails only have to follow the scale.
    /// * **The tails.** Bases ride the body (see [`Self::scale_body_about`]),
    ///   tips stay put, kinds/bends/widths untouched.
    /// * **The style.** Nothing in [`BalloonInk`], `width_scale` or
    ///   `border_px` is read or written here.
    /// * **A hand-drawn shape.** A [`BalloonShape::Polygon`] is scaled
    ///   UNIFORMLY (one factor, both axes) until the lettering fits inside its
    ///   real outline — measured through the shape's own SDF, so a concave
    ///   drawn bubble is honoured rather than approximated by its box. Its
    ///   proportions and every anchor's pressure survive; it is never reset to
    ///   an ellipse, and the writing-direction clamp below is deliberately NOT
    ///   applied to it (that would restyle the drawing).
    ///
    /// Returns whether anything actually moved, so a caller can skip a no-op
    /// undo step.
    pub fn fit_to_text(&mut self, text: &crate::text::TextItem, em_px: f32) -> bool {
        // The lettering's axis-aligned extent — the ROTATED corners, so a
        // tilted text box is covered by what you can see of it, grown by the
        // margin on every side.
        let pad = (em_px.max(1.0) * FIT_PAD_EM).max(2.0);
        let mut want = [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ];
        for p in text.corners() {
            want[0] = want[0].min(p[0]);
            want[1] = want[1].min(p[1]);
            want[2] = want[2].max(p[0]);
            want[3] = want[3].max(p[1]);
        }
        if !want.iter().all(|v| v.is_finite()) {
            return false;
        }
        want = [want[0] - pad, want[1] - pad, want[2] + pad, want[3] + pad];

        let before = self.clone();
        let c = self.shape.center();
        // Half-extents measured FROM the fixed centre: whichever side the
        // lettering reaches further on decides. (No `abs()` — a box entirely
        // to one side gives one negative term, and `max` is exactly right.)
        let min_half = MIN_BALLOON_EXTENT * 0.5;
        let hx = (c[0] - want[0]).max(want[2] - c[0]).max(min_half);
        let hy = (c[1] - want[1]).max(want[3] - c[1]).max(min_half);

        // Whichever body it is, the answer is a pair of scale factors about
        // the fixed centre — so the tails follow through ONE path.
        let (sx, sy) = match &self.shape {
            BalloonShape::Ellipse { radii, .. } => {
                // An axis-aligned ellipse contains a rectangle of half-extents
                // (hx, hy) exactly when its radii are √2 × them — the minimal
                // such ellipse, so the corners of the padded box graze the
                // curve and nothing pokes out.
                let sqrt2 = std::f32::consts::SQRT_2;
                let (mut rx, mut ry) = (hx * sqrt2, hy * sqrt2);
                if text.vertical {
                    ry = ry.max(rx * FIT_ASPECT);
                } else {
                    rx = rx.max(ry * FIT_ASPECT);
                }
                (rx / radii[0].max(1e-3), ry / radii[1].max(1e-3))
            }
            BalloonShape::RoundRect { rect, .. } => {
                // A rectangle holds a rectangle: no √2 here.
                let (mut nx, mut ny) = (hx, hy);
                if text.vertical {
                    ny = ny.max(nx * FIT_ASPECT);
                } else {
                    nx = nx.max(ny * FIT_ASPECT);
                }
                let (ow, oh) = ((rect[2] - rect[0]).abs(), (rect[3] - rect[1]).abs());
                (nx * 2.0 / ow.max(1e-3), ny * 2.0 / oh.max(1e-3))
            }
            BalloonShape::Polygon { .. } => {
                let s = self.polygon_fit_scale(c, want);
                (s, s)
            }
        };
        self.scale_body_about(c, sx, sy);
        *self != before
    }

    /// The single factor a drawn body must be scaled by, about `c`, for all
    /// four corners of `want` to land inside its outline.
    ///
    /// Solved against the shape's own signed distance rather than its box: a
    /// point `p` is inside the body scaled by `s` about `c` exactly when
    /// `c + (p - c)/s` is inside the body as drawn, so one bracket-then-bisect
    /// over `s` answers it for a concave spline too. Monotone for any body
    /// that is star-shaped about its own centre, which every balloon a human
    /// draws is; a pathological shape still terminates, just on a bound rather
    /// than the true minimum.
    fn polygon_fit_scale(&self, c: [f32; 2], want: [f32; 4]) -> f32 {
        let corners = [
            [want[0], want[1]],
            [want[2], want[1]],
            [want[2], want[3]],
            [want[0], want[3]],
        ];
        let fits = |s: f32| {
            let inv = 1.0 / s.max(1e-3);
            corners.iter().all(|p| {
                self.shape
                    .sdf([c[0] + (p[0] - c[0]) * inv, c[1] + (p[1] - c[1]) * inv])
                    <= 0.0
            })
        };
        // Bracket: grow until it fits (a shape already big enough starts at 1).
        let mut hi = 1.0f32;
        for _ in 0..32 {
            if fits(hi) {
                break;
            }
            hi *= 1.5;
        }
        // …then squeeze from below: s → 0 collapses the body to a point, so
        // 0 never fits and the bracket is valid.
        let mut lo = 0.0f32;
        for _ in 0..32 {
            let mid = (lo + hi) * 0.5;
            if fits(mid) {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        // Never shrink a drawn bubble below the degenerate floor.
        let e = self.shape.raw_extent();
        let floor = MIN_BALLOON_EXTENT / e[0].min(e[1]).max(1e-3);
        hi.max(floor)
    }
}

/// The lettering a balloon should be fitted around: the LAST (topmost) item in
/// `texts` that belongs to `body`.
///
/// Pairing is geometry, not bookkeeping — there is no stored balloon→text link
/// in this app and this does not invent one, exactly like
/// [`rotate_texts_in`] and the Object tool's stack cycling. An item belongs
/// when its centre is in the bubble (the ordinary case) or when any corner is
/// (lettering that currently overflows a too-small bubble, which is the whole
/// reason to press Fit).
///
/// The test is against the BODY, not [`Balloon::contains`]: a tail is somewhere
/// a bubble points, not somewhere lettering goes, and an SFX sitting over the
/// tail must not capture the fit.
pub fn text_in(body: &Balloon, texts: &crate::text::TextSet) -> Option<usize> {
    let inside = |p: [f32; 2]| body.shape.sdf(p) <= 0.0;
    texts
        .texts
        .iter()
        .rposition(|t| inside(t.center()) || t.corners().into_iter().any(inside))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32) -> Balloon {
        Balloon {
            shape: BalloonShape::Ellipse {
                center: [cx, cy],
                radii: [rx, ry],
            },
            tails: Vec::new(),

            ..Default::default()
        }
    }

    fn px_of(tiles: &HashMap<TileIdx, Arc<Tile>>, x: i32, y: i32) -> [u16; 4] {
        let idx = TileIdx::of_pixel(x, y);
        let (ox, oy) = idx.origin();
        tiles
            .get(&idx)
            .map(|t| t.pixel((x - ox) as usize, (y - oy) as usize))
            .unwrap_or([0, 0, 0, 0])
    }

    #[test]
    fn shape_sdfs_have_the_right_sign() {
        let e = BalloonShape::Ellipse {
            center: [100.0, 100.0],
            radii: [50.0, 30.0],
        };
        assert!(e.sdf([100.0, 100.0]) < -20.0);
        assert!(e.sdf([100.0, 75.0]) < 0.0);
        assert!(e.sdf([160.0, 100.0]) > 5.0);
        assert!(e.sdf([150.0, 100.0]).abs() < 1.0, "on the boundary");

        let r = BalloonShape::RoundRect {
            rect: [0.0, 0.0, 100.0, 60.0],
            corner: 10.0,
        };
        assert!(r.sdf([50.0, 30.0]) < -20.0);
        assert!(r.sdf([50.0, -5.0]) > 4.0);
        assert!((r.sdf([50.0, 0.0])).abs() < 0.5);
        assert!(r.sdf([1.5, 1.5]) > 0.0, "outside the rounded corner");

        // Concave L-shape: the notch must be outside.
        let l = BalloonShape::Polygon {
            points: vec![
                [0.0, 0.0],
                [100.0, 0.0],
                [100.0, 40.0],
                [40.0, 40.0],
                [40.0, 100.0],
                [0.0, 100.0],
            ],
            widths: Vec::new(),
            corners: vec![true; 6],
        };
        assert!(l.sdf([20.0, 20.0]) < 0.0);
        assert!(l.sdf([20.0, 80.0]) < 0.0);
        assert!(l.sdf([80.0, 80.0]) > 10.0, "in the notch = outside");
    }

    #[test]
    fn raster_white_fill_black_rim_transparent_out() {
        let set = BalloonSet {
            pressure_width: false,
            balloons: vec![ellipse(128.0, 128.0, 80.0, 60.0)],
            border_px: 4.0,
        };
        let tiles = set.rasterize((256, 256));
        let one = FIX15_ONE as u16;

        let c = px_of(&tiles, 128, 128);
        assert_eq!(c, [one, one, one, one], "center = opaque white");
        let rim = px_of(&tiles, 128 + 80, 128); // right extremity
        assert_eq!(rim[3], one, "rim is opaque");
        assert!(rim[0] < one / 8, "rim is ink-black");
        assert_eq!(
            px_of(&tiles, 10, 10),
            [0, 0, 0, 0],
            "far outside = transparent"
        );
        assert_eq!(px_of(&tiles, 250, 10), [0, 0, 0, 0]);
    }

    #[test]
    fn tail_merges_into_the_body() {
        let mut b = ellipse(128.0, 100.0, 60.0, 40.0);
        // Tail from inside the body down to a tip below it, wide base.
        b.tails.push(Tail {
            base: [128.0, 120.0],
            tip: [128.0, 220.0],
            width: 40.0,
            ..Default::default()
        });
        let set = BalloonSet {
            balloons: vec![b],
            border_px: 4.0,
            pressure_width: false,
        };
        let tiles = set.rasterize((256, 256));
        let one = FIX15_ONE as u16;

        // Where the tail crosses the ellipse boundary (y ≈ 140 at x=128) the
        // border must be GONE — interior white, not black.
        let cross = px_of(&tiles, 128, 140);
        assert_eq!(cross[3], one);
        assert!(cross[0] > one - one / 8, "shared edge erased by the union");
        // The tip region is filled.
        let tip = px_of(&tiles, 128, 210);
        assert_eq!(tip[3], one, "tail interior is opaque");
        // The tail's own flank has a border: just outside the triangle edge.
        let flank = px_of(&tiles, 128 - 14, 180);
        assert!(flank[0] < one / 4, "tail flank is inked");
    }

    #[test]
    fn overlapping_balloons_merge() {
        let set = BalloonSet {
            pressure_width: false,
            balloons: vec![
                ellipse(100.0, 100.0, 50.0, 40.0),
                ellipse(160.0, 100.0, 50.0, 40.0),
            ],
            border_px: 4.0,
        };
        let tiles = set.rasterize((256, 256));
        let one = FIX15_ONE as u16;
        // Midpoint between centers is inside both: white, no border seam.
        let seam = px_of(&tiles, 130, 100);
        assert_eq!(seam[3], one);
        assert!(seam[0] > one - one / 8, "no seam between joined balloons");
    }

    #[test]
    fn raster_is_sparse_and_shares_white() {
        let set = BalloonSet {
            pressure_width: false,
            balloons: vec![ellipse(200.0, 200.0, 150.0, 150.0)],
            border_px: 4.0,
        };
        let tiles = set.rasterize((1024, 1024));
        let total = (1024 / 64) * (1024 / 64);
        assert!(
            tiles.len() < total / 2,
            "page far from the balloon has no tiles"
        );
        let one = FIX15_ONE as u16;
        let whites: Vec<_> = tiles
            .values()
            .filter(|t| t.pixel(0, 0) == [one; 4] && t.pixel(63, 63) == [one; 4])
            .collect();
        assert!(
            whites.len() >= 4,
            "a 300px balloon has solid interior tiles"
        );
        // A thin ring of boundary-classified tiles may come out all-white
        // per-pixel with their own allocation; the deep interior must share.
        let mut counts: HashMap<*const Tile, usize> = HashMap::new();
        for t in &whites {
            *counts.entry(Arc::as_ptr(t)).or_default() += 1;
        }
        let shared = counts.values().copied().max().unwrap_or(0);
        assert!(shared >= 4, "deep-interior tiles share one allocation");
    }

    #[test]
    fn handles_roundtrip_edits() {
        let mut b = ellipse(100.0, 100.0, 50.0, 30.0);
        b.tails.push(Tail {
            base: [100.0, 120.0],
            tip: [100.0, 200.0],
            width: 20.0,
            ..Default::default()
        });
        assert_eq!(b.handles().len(), 4 + 2);
        assert_eq!(
            b.handle_near([151.0, 100.0], 5.0),
            Some(BalloonHandle::Shape(0))
        );
        assert_eq!(
            b.handle_near([100.0, 201.0], 5.0),
            Some(BalloonHandle::TailTip(0))
        );
        b.apply_handle(BalloonHandle::Shape(0), [180.0, 100.0]);
        assert!(
            matches!(b.shape, BalloonShape::Ellipse { radii, .. } if (radii[0] - 80.0).abs() < 1e-3)
        );
        b.apply_handle(BalloonHandle::TailTip(0), [140.0, 230.0]);
        assert_eq!(b.tails[0].tip, [140.0, 230.0]);
        b.translate(10.0, -5.0);
        assert!(matches!(b.shape, BalloonShape::Ellipse { center, .. } if center == [110.0, 95.0]));
        assert_eq!(b.tails[0].base, [110.0, 115.0]);

        let mut r = Balloon {
            shape: BalloonShape::RoundRect {
                rect: [0.0, 0.0, 100.0, 60.0],
                corner: 8.0,
            },
            tails: Vec::new(),

            ..Default::default()
        };
        r.apply_handle(BalloonHandle::Shape(0), [-20.0, -10.0]);
        assert!(
            matches!(r.shape, BalloonShape::RoundRect { rect, .. } if rect == [-20.0, -10.0, 100.0, 60.0])
        );
    }

    #[test]
    fn simplify_keeps_corners_drops_noise() {
        // A noisy straight line simplifies to its endpoints.
        let mut line: Vec<[f32; 2]> = Vec::new();
        for i in 0..=100 {
            line.push([i as f32, if i % 2 == 0 { 0.3 } else { -0.3 }]);
        }
        let s = simplify_polyline(&line, 1.0);
        assert_eq!(s.len(), 2);
        // A right angle keeps its corner.
        let mut angle: Vec<[f32; 2]> = (0..=50).map(|i| [i as f32, 0.0]).collect();
        angle.extend((1..=50).map(|i| [50.0, i as f32]));
        let s = simplify_polyline(&angle, 1.0);
        assert_eq!(s.len(), 3);
        assert_eq!(s[1], [50.0, 0.0]);
    }

    #[test]
    fn validity_guards() {
        assert!(!ellipse(0.0, 0.0, 2.0, 30.0).is_valid());
        assert!(ellipse(0.0, 0.0, 20.0, 30.0).is_valid());
        let tiny = Balloon {
            shape: BalloonShape::Polygon {
                points: vec![[0.0, 0.0], [3.0, 0.0], [0.0, 3.0]],
                widths: Vec::new(),
                corners: Vec::new(),
            },
            tails: Vec::new(),

            ..Default::default()
        };
        assert!(!tiny.is_valid());
    }

    /// A square's anchors under the smooth spline: the curve passes through
    /// every anchor exactly and the edges between them bulge outward like a
    /// hand-drawn bubble — but never more than the classic 25% Catmull
    /// overshoot, and it never collapses inward.
    #[test]
    fn spline_rounds_the_corners() {
        let pts = [[-60.0, -60.0], [60.0, -60.0], [60.0, 60.0], [-60.0, 60.0]];
        let (dense, ws) = tessellate_closed(&pts, &[], &[]);
        assert!(
            dense.len() > 16,
            "dense enough for smooth AA, got {}",
            dense.len()
        );
        assert!(ws.iter().all(|&w| (w - 1.0).abs() < 1e-6));
        // Every anchor is ON the curve (a through-the-points spline).
        for a in &pts {
            let d = dense
                .iter()
                .map(|p| len(sub(*p, *a)))
                .fold(f32::INFINITY, f32::min);
            assert!(d < 1.5, "anchor on curve, {d}");
        }
        // Mid-edge bulge: right edge |p - (60,0)| along y≈0 — overshoot
        // bounded (uniform Catmull-Rom's worst case is 25% of the half-edge).
        let bulge = dense
            .iter()
            .filter(|p| p[1].abs() < 2.0 && p[0] > 0.0)
            .map(|p| p[0])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(
            bulge > 60.0,
            "the edge bulges outward like a drawn bubble ({bulge})"
        );
        assert!(bulge <= 76.0, "overshoot bounded ({bulge})");
    }

    /// Rows 84/85: the curve tool's spline. It must pass through every
    /// point the artist clicked, stay dense enough over a long sweep that
    /// the brush inks a curve and not a polygon, and never join the ends
    /// up (an open chain is the whole difference from the balloon).
    #[test]
    fn open_spline_runs_through_the_clicked_points() {
        // Two points = the straight line you clicked, verbatim.
        let two = [[0.0, 0.0], [900.0, 0.0]];
        assert_eq!(tessellate_open(&two), two.to_vec());

        // A long sweeping arc: three clicks, apex in the middle.
        let pts = [[0.0, 0.0], [450.0, -200.0], [900.0, 0.0]];
        let dense = tessellate_open(&pts);
        assert_eq!(dense[0], pts[0], "starts on the first click");
        assert_eq!(*dense.last().unwrap(), pts[2], "ends on the last one");
        for a in &pts {
            let d = dense
                .iter()
                .map(|p| len(sub(*p, *a)))
                .fold(f32::INFINITY, f32::min);
            assert!(d < 1e-3, "clicked point {a:?} is ON the curve ({d})");
        }
        // Dense enough that the longest chord is a brush step, not a facet.
        let gap = dense
            .windows(2)
            .map(|w| len(sub(w[1], w[0])))
            .fold(0.0f32, f32::max);
        assert!(gap <= 8.0, "longest chord over a 900 px sweep: {gap}");
        // Open: the last point is nowhere near the first.
        assert!(len(sub(dense[0], *dense.last().unwrap())) > 800.0);
        // And it actually bows — the middle click is not on the chord.
        let apex = dense
            .iter()
            .map(|p| p[1])
            .fold(f32::INFINITY, f32::min);
        assert!(apex <= -200.0, "the arc reaches its apex ({apex})");
    }

    /// `FG-016`: the open spline's corner anchor. The curve still passes
    /// THROUGH the marked point — it just arrives and leaves with a crease
    /// instead of sweeping — and the smooth runs either side of it keep the
    /// shape they had, which is what tells this apart from "flatten it".
    #[test]
    fn open_spline_creases_at_a_corner_anchor() {
        // A shallow "V" laid out as five clicks. Marked smooth it bows
        // through the middle; marked as a corner the middle is a kink.
        let pts = [
            [0.0, 0.0],
            [100.0, -60.0],
            [200.0, 0.0],
            [300.0, -60.0],
            [400.0, 0.0],
        ];
        let smooth = tessellate_open(&pts);
        let kinked = tessellate_open_corners(&pts, &[false, false, true, false, false]);

        // No flags at all is the smooth spline, byte for byte — the row adds
        // a case, it does not re-route the ordinary curve.
        assert_eq!(tessellate_open_corners(&pts, &[]), smooth);

        // Both run end to end through every click.
        assert_eq!(kinked[0], pts[0]);
        assert_eq!(*kinked.last().unwrap(), pts[4]);
        for a in &pts {
            let d = kinked
                .iter()
                .map(|p| len(sub(*p, *a)))
                .fold(f32::INFINITY, f32::min);
            assert!(d < 1e-3, "clicked point {a:?} is still ON the curve ({d})");
        }

        // THE discriminator, and the definition of a crease: somewhere along
        // the creased polyline the direction changes abruptly. A smooth
        // Catmull-Rom turns by a fraction of a degree per 4 px chord, so the
        // sharpest turn in the whole dense run separates the two cleanly —
        // no eyeballing of y values, which a spline can match by accident.
        let sharpest = |dense: &[[f32; 2]]| {
            dense
                .windows(3)
                .map(|w| {
                    let (u, v) = (sub(w[1], w[0]), sub(w[2], w[1]));
                    let d = (u[0] * v[0] + u[1] * v[1]) / (len(u) * len(v)).max(1e-6);
                    d.clamp(-1.0, 1.0).acos().to_degrees()
                })
                .fold(0.0f32, f32::max)
        };
        assert!(
            sharpest(&smooth) < 15.0,
            "the smooth spline never kinks ({}°)",
            sharpest(&smooth)
        );
        assert!(
            sharpest(&kinked) > 45.0,
            "the corner anchor is a real crease ({}°)",
            sharpest(&kinked)
        );

        // Two corners in a row give back the chord the artist clicked — the
        // span between them is a two-point run, i.e. the straight line.
        let straight = tessellate_open_corners(&pts, &[false, true, true, false, false]);
        let i = straight
            .iter()
            .position(|p| *p == pts[1])
            .expect("the corner is on the path");
        assert_eq!(
            straight[i + 1],
            pts[2],
            "nothing is tessellated between two corners — it is the chord"
        );
        // Which the smooth spline is emphatically not: it puts samples in
        // there, and they bow off the chord.
        let off = smooth
            .iter()
            .filter(|p| p[0] > 105.0 && p[0] < 195.0)
            .map(|p| (p[1] - (-60.0 + (p[0] - 100.0) * 0.6)).abs())
            .fold(0.0f32, f32::max);
        assert!(off > 3.0, "the smooth spline bows off that chord ({off})");
        // A flag on an END anchor is inert — that end is one-sided already.
        assert_eq!(
            tessellate_open_corners(&pts, &[true, false, false, false, true]),
            smooth
        );
    }

    /// A corner anchor kinks: with the tangent dead-stopped the segment
    /// between two corners is the straight chord — the square stays square.
    #[test]
    fn corner_anchor_kinks() {
        let pts = [[-60.0, -60.0], [60.0, -60.0], [60.0, 60.0], [-60.0, 60.0]];
        let (cornered, _) = tessellate_closed(&pts, &[true; 4], &[]);
        // Every dense point must sit ON the square's outline (straight edges).
        for p in &cornered {
            let on_edge = (p[0].abs() - 60.0).abs() < 0.5 && p[1].abs() <= 61.0
                || (p[1].abs() - 60.0).abs() < 0.5 && p[0].abs() <= 61.0;
            assert!(on_edge, "cornered square stays straight: {p:?}");
        }
    }

    /// Pressure-modulated outline: the same drawn square, one set with
    /// uniform 1.0 pressure and one with 0.0, inks visibly thinner where the
    /// pen was light.
    #[test]
    fn pressure_modulates_outline_width() {
        // A square centred on (128,128); the right edge runs at x=188.
        let pts = [[168.0, 68.0], [188.0, 68.0], [188.0, 188.0], [168.0, 188.0]];
        let mk = |ws: Vec<f32>, pw: bool| BalloonSet {
            balloons: vec![Balloon {
                shape: BalloonShape::Polygon {
                    points: pts.to_vec(),
                    widths: ws,
                    corners: vec![true; 4],
                },
                tails: Vec::new(),

                ..Default::default()
            }],
            border_px: 10.0,
            pressure_width: pw,
        };
        let thick = mk(vec![1.0; 4], true).rasterize((256, 256));
        let thin = mk(vec![0.0; 4], true).rasterize((256, 256));
        let ink = |t: &HashMap<TileIdx, Arc<Tile>>, x: i32, y: i32| px_of(t, x, y)[3];
        // 2px outside the boundary: inside the thick half-width (5.0) but
        // past the thin one (1.75) — the pressure difference must show.
        let a = ink(&thick, 190, 128);
        let b = ink(&thin, 190, 128);
        assert!(a > b, "pressure thins the line: {a} vs {b}");
        assert!(a > 0, "thick side inks at 2px out");
        assert_eq!(ink(&thin, 192, 128), 0, "thin side doesn't reach 4px");
    }

    /// Old files: a Polygon without widths/corners still parses (serde
    /// defaults) and renders as the hard-edged polygon it always was.
    #[test]
    fn legacy_polygon_json_loads() {
        let json = r#"{"balloons":[{"shape":{"Polygon":{"points":[[0,0],[100,0],[100,100],[0,100]]}},"tails":[]}],"border_px":3.0}"#;
        let set: BalloonSet = serde_json::from_str(json).expect("legacy polygon loads");
        match &set.balloons[0].shape {
            BalloonShape::Polygon {
                widths, corners, ..
            } => {
                assert!(widths.is_empty() && corners.is_empty());
            }
            _ => panic!("polygon"),
        }
        assert!(!set.pressure_width);
        let tiles = set.rasterize((128, 128));
        assert!(tiles.len() > 0);
    }

    /// The blue-box transform: scale + rotate a drawn balloon, anchors follow
    /// exactly (spline shapes transform losslessly).
    #[test]
    fn transform_around_moves_anchors() {
        let mut b = Balloon {
            shape: BalloonShape::Polygon {
                points: vec![[10.0, 0.0], [0.0, 10.0], [-10.0, 0.0], [0.0, -10.0]],
                widths: vec![0.5; 4],
                corners: vec![false; 4],
            },
            tails: vec![Tail {
                base: [0.0, 5.0],
                tip: [0.0, 20.0],
                width: 8.0,
                ..Default::default()
            }],

            ..Default::default()
        };
        b.transform_around([0.0, 0.0], 2.0, 2.0, std::f32::consts::FRAC_PI_2);
        let BalloonShape::Polygon { points, .. } = &b.shape else {
            unreachable!()
        };
        // (10,0) scaled 2x then rotated 90° (y-down clockwise): -> (0,20).
        assert!((points[0][0] - 0.0).abs() < 1e-4 && (points[0][1] - 20.0).abs() < 1e-4);
        // tail tip (0,20) -> scaled (0,40) -> rotated (-40,0).
        assert!(
            (b.tails[0].tip[0] + 40.0).abs() < 1e-4,
            "tail follows: {:?}",
            b.tails[0].tip
        );
        assert!((b.tails[0].tip[1] - 0.0).abs() < 1e-4);
        assert!((b.tails[0].width - 16.0).abs() < 1e-4, "tail width scales");
    }

    #[test]
    fn balloon_at_prefers_topmost() {
        let set = BalloonSet {
            pressure_width: false,
            balloons: vec![
                ellipse(100.0, 100.0, 50.0, 50.0),
                ellipse(120.0, 100.0, 50.0, 50.0),
            ],
            border_px: 4.0,
        };
        assert_eq!(set.balloon_at([110.0, 100.0]), Some(1));
        assert_eq!(set.balloon_at([60.0, 100.0]), Some(0));
        assert_eq!(set.balloon_at([250.0, 250.0]), None);
    }

    fn drawn_square() -> Balloon {
        Balloon {
            shape: BalloonShape::Polygon {
                points: vec![[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0]],
                widths: vec![0.4, 0.6, 0.8, 0.6],
                corners: vec![true, false, true, false],
            },
            tails: Vec::new(),
            ..Default::default()
        }
    }

    /// Auditor round 33 finding #3: "correct line width" scales the RENDERED
    /// outline, never the recorded pressure widths — the old multiplier
    /// rewrote the normalized widths, saturated them at 1.0 on the way up,
    /// and a later scale-down returned a flat border instead of the taper.
    /// `width_scale` applies at rasterize time, so up-then-down is an EXACT
    /// byte-level round-trip and the stored widths never move.
    #[test]
    fn width_scale_is_render_time_and_reversible() {
        let mk = |scale: f32| -> BalloonSet {
            let mut b = drawn_square();
            b.width_scale = scale;
            BalloonSet {
                balloons: vec![b],
                border_px: 6.0,
                pressure_width: true,
            }
        };
        let ink = |s: &BalloonSet| -> u64 {
            s.rasterize((256, 256))
                .values()
                .map(|t| t.alpha_sum())
                .sum()
        };

        let one = mk(1.0);
        let up = mk(2.0);
        assert!(
            ink(&up) > ink(&one),
            "2× correct-width must ink more border"
        );

        // The round-trip is exact — same tile set, same bytes.
        let (a, back) = (one.rasterize((256, 256)), mk(1.0).rasterize((256, 256)));
        assert_eq!(a.len(), back.len());
        for (k, t) in &a {
            let t2 = back.get(k).expect("same tile set after round-trip");
            assert_eq!(
                t.data(),
                t2.data(),
                "scale up then down must restore the raster"
            );
        }

        // The stored widths survive any scale.
        let BalloonShape::Polygon { widths, .. } = &mk(4.0).balloons[0].shape else {
            unreachable!()
        };
        assert_eq!(widths, &vec![0.4, 0.6, 0.8, 0.6]);

        // Serialization: the key round-trips, and files from before it load
        // as 1.0 (serde default).
        let json = serde_json::to_string(&mk(2.5)).unwrap();
        assert!(json.contains("\"width_scale\":2.5"), "{json}");
        let old = r#"{"balloons":[{"shape":{"Polygon":{"points":[[0,0],[10,0],[0,10]],
            "widths":[],"corners":[]}},"tails":[]}],"border_px":4.0,"pressure_width":false}"#;
        let loaded: BalloonSet = serde_json::from_str(old).expect("old files load");
        assert_eq!(loaded.balloons[0].width_scale, 1.0);
    }

    /// Object-tool anchor editing (TODO round 31): insert on an edge, delete
    /// a vertex, toggle the corner flag — all keeping the per-anchor vectors
    /// (points / widths / corners) aligned, and refusing to collapse.
    #[test]
    fn anchor_insert_delete_toggle_keep_vectors_aligned() {
        let mut b = drawn_square();
        // Hit the top edge (segment 0: (0,0)->(100,0)) near its middle.
        let (seg, p) = b.edge_point_near([50.0, 3.0], 6.0).expect("edge hit");
        assert_eq!(seg, 0);
        assert!((p[0] - 50.0).abs() < 1e-4 && p[1].abs() < 1e-4);
        assert!(b.insert_anchor(seg, p));
        let BalloonShape::Polygon {
            points,
            widths,
            corners,
        } = &b.shape
        else {
            unreachable!()
        };
        assert_eq!(points.len(), 5);
        assert_eq!(widths.len(), 5, "width inserted (mean of 0.4,0.6)");
        assert!((widths[1] - 0.5).abs() < 1e-5);
        assert_eq!(corners.len(), 5);
        assert!(!corners[1], "the new anchor is smooth");

        // No edge hit far away; analytic bodies have none at all.
        assert!(b.edge_point_near([500.0, 500.0], 6.0).is_none());
        assert!(
            ellipse(0.0, 0.0, 50.0, 50.0)
                .edge_point_near([50.0, 0.0], 10.0)
                .is_none()
        );

        // Toggle the new anchor to a corner and back.
        assert!(b.toggle_anchor_corner(1));
        assert!(b.toggle_anchor_corner(1));

        // Delete down to the floor of three anchors, then refuse.
        assert!(b.delete_anchor(2));
        assert!(b.delete_anchor(3));
        let BalloonShape::Polygon {
            points,
            widths,
            corners,
        } = &b.shape
        else {
            unreachable!()
        };
        assert_eq!(points.len(), 3);
        assert_eq!(widths.len(), 3);
        assert_eq!(corners.len(), 3);
        assert!(!b.delete_anchor(0), "below the 3-anchor floor");
        assert!(b.is_valid());

        // Analytic bodies refuse anchor ops (their handles are extents).
        let mut e = ellipse(0.0, 0.0, 50.0, 50.0);
        assert!(!e.insert_anchor(0, [50.0, 0.0]));
        assert!(!e.delete_anchor(0));
        assert!(!e.toggle_anchor_corner(0));
    }

    /// Tails track their balloon: resizing the BODY through a shape handle
    /// re-anchors every tail base to the boundary; the tip stays put (the
    /// speaker does not move because the bubble got bigger).
    #[test]
    fn body_handle_resize_keeps_tail_bases_on_the_boundary() {
        let mut b = ellipse(100.0, 100.0, 50.0, 50.0);
        b.tails = vec![Tail {
            base: [150.0, 100.0],
            tip: [220.0, 130.0],
            width: 10.0,
            ..Default::default()
        }];
        // Drag the right radius handle out to x = 200 (rx 50 -> 100).
        b.apply_handle(BalloonHandle::Shape(0), [200.0, 100.0]);
        let BalloonShape::Ellipse { center, radii } = &b.shape else {
            unreachable!()
        };
        assert!((radii[0] - 100.0).abs() < 1e-4);
        let t = &b.tails[0];
        // Unit-ellipse anchor (1,0) rides the boundary: base -> (200,100).
        assert!(
            (t.base[0] - (center[0] + radii[0])).abs() < 1e-3,
            "{:?}",
            t.base
        );
        assert!((t.base[1] - 100.0).abs() < 1e-3);
        // The tip is the user's own — absolute, unchanged.
        assert_eq!(t.tip, [220.0, 130.0]);

        // Round-rect: same rule in unit-rect coords.
        let mut r = Balloon {
            shape: BalloonShape::RoundRect {
                rect: [0.0, 0.0, 100.0, 100.0],
                corner: 8.0,
            },
            tails: vec![Tail {
                base: [50.0, 100.0],
                tip: [50.0, 170.0],
                width: 10.0,
                ..Default::default()
            }],

            ..Default::default()
        };
        // Bottom-right corner out to (150,150).
        r.apply_handle(BalloonHandle::Shape(2), [150.0, 150.0]);
        let BalloonShape::RoundRect { rect, .. } = &r.shape else {
            unreachable!()
        };
        assert_eq!(*rect, [0.0, 0.0, 150.0, 150.0]);
        // Anchor (0.5, 1.0) -> (75, 150): still on the bottom edge.
        assert!(
            (r.tails[0].base[0] - 75.0).abs() < 1e-3,
            "{:?}",
            r.tails[0].base
        );
        assert!((r.tails[0].base[1] - 150.0).abs() < 1e-3);
        assert_eq!(r.tails[0].tip, [50.0, 170.0]);
    }

    /// Per-tail delete (CSP: remove a bubble's tail, keep the body).
    #[test]
    fn delete_tail_removes_only_that_tail() {
        let mut b = ellipse(0.0, 0.0, 50.0, 50.0);
        b.tails = vec![
            Tail {
                base: [50.0, 0.0],
                tip: [90.0, 0.0],
                width: 8.0,
                ..Default::default()
            },
            Tail {
                base: [-50.0, 0.0],
                tip: [-90.0, 0.0],
                width: 8.0,
                ..Default::default()
            },
        ];
        assert!(b.delete_tail(0));
        assert_eq!(b.tails.len(), 1);
        assert_eq!(b.tails[0].tip, [-90.0, 0.0], "the wrong tail went");
        assert!(!b.delete_tail(9));
        assert!(b.delete_tail(0));
        assert!(b.tails.is_empty());
    }

    // --- colour, opacity, tone (rows 81/82) --------------------------------

    /// The rasterizer as it stood BEFORE balloons had colour: an opaque black
    /// outline over an opaque white fill, in two lines.
    ///
    /// ```text
    /// alpha = border + inside * (1 - border)
    /// rgb   = inside * (1 - border)
    /// ```
    ///
    /// Kept here verbatim so the byte-identity test below has something real
    /// to compare against instead of a hash nobody can re-derive.
    fn legacy_pixel(set: &BalloonSet, x: i32, y: i32) -> [u16; 4] {
        let p = [x as f32 + 0.5, y as f32 + 0.5];
        let (mut d, mut pr, mut winner) = (f32::INFINITY, 1.0f32, 0usize);
        for (i, b) in set.balloons.iter().enumerate() {
            let (bd, bpr) = b.sdf_w(p);
            if bd < d {
                d = bd;
                pr = bpr;
                winner = i;
            }
        }
        let base = if set.pressure_width {
            set.border_px * (0.35 + 0.65 * pr)
        } else {
            set.border_px
        };
        let bw = base * set.balloons[winner].width_scale.max(0.0);
        let inside = (0.5 - d).clamp(0.0, 1.0);
        let border = (bw * 0.5 + 0.5 - d.abs()).clamp(0.0, 1.0);
        let alpha = border + inside * (1.0 - border);
        let rgb = inside * (1.0 - border);
        if alpha <= 0.0 {
            return [0, 0, 0, 0];
        }
        let rv = (rgb * FIX15_ONE as f32).round() as u16;
        let av = (alpha * FIX15_ONE as f32).round() as u16;
        [rv, rv, rv, av]
    }

    /// **The load-bearing test of the colour round.** Every field this round
    /// added serde-defaults to the old hardcoded appearance, so a page saved
    /// before any of them existed must rasterize to the SAME BYTES.
    ///
    /// Proved the honest way: the pre-round formula is recomputed above from
    /// the same public SDFs, and every pixel of a 256×256 page is compared —
    /// interior, anti-aliased rim, pressure-modulated outline, corrected
    /// width, the shared-allocation deep tiles and the empty margin alike.
    /// Both bodies (analytic ellipse and drawn spline) and a tail are in the
    /// scene, and the balloons overlap so the union's winner-takes-all path
    /// is exercised too.
    #[test]
    fn old_files_render_byte_identically() {
        let legacy = r#"{
            "balloons": [
                {"shape":{"Ellipse":{"center":[96.0,110.0],"radii":[60.0,44.0]}},
                 "tails":[{"base":[96.0,150.0],"tip":[70.0,220.0],"width":26.0}]},
                {"shape":{"Polygon":{"points":[[140,60],[210,70],[205,150],[135,140]],
                 "widths":[0.2,0.9,0.45,0.7],"corners":[false,true,false,true]}},
                 "tails":[]}
            ],
            "border_px": 5.0,
            "pressure_width": true
        }"#;
        let set: BalloonSet = serde_json::from_str(legacy).expect("a pre-colour file loads");

        // Every new field sits at the neutral value…
        for b in &set.balloons {
            assert_eq!(b.line_color, [0, 0, 0], "old outlines are black");
            assert_eq!(b.fill_color, [255, 255, 255], "old fills are white");
            assert_eq!(b.line_opacity, 1.0);
            assert_eq!(b.fill_opacity, 1.0);
            assert!(b.fill_tone.is_none(), "old fills are flat, not screened");
            assert_eq!(b.width_scale, 1.0);
            for t in &b.tails {
                assert_eq!(t.kind, TailKind::Spoken);
                assert_eq!(t.bend, 0.0);
            }
        }

        // …and the pixels are the old pixels, all 65 536 of them.
        let tiles = set.rasterize((256, 256));
        let mut inked = 0u32;
        for y in 0..256 {
            for x in 0..256 {
                let got = px_of(&tiles, x, y);
                let want = legacy_pixel(&set, x, y);
                assert_eq!(got, want, "pixel ({x},{y}) moved");
                if want[3] > 0 {
                    inked += 1;
                }
            }
        }
        assert!(
            inked > 10_000,
            "the scene actually drew something ({inked})"
        );

        // The correct-width multiplier rides the same proof.
        let mut scaled = set.clone();
        scaled.balloons[1].width_scale = 1.8;
        let tiles = scaled.rasterize((256, 256));
        for y in 0..256 {
            for x in 0..256 {
                assert_eq!(
                    px_of(&tiles, x, y),
                    legacy_pixel(&scaled, x, y),
                    "corrected-width pixel ({x},{y}) moved"
                );
            }
        }
    }

    /// `B-001`–`B-004`: line and fill each get their own colour and opacity,
    /// and they land in the raster premultiplied.
    #[test]
    fn line_and_fill_colour_and_opacity() {
        let mut b = ellipse(128.0, 128.0, 70.0, 55.0);
        b.line_color = [220, 20, 40]; // red outline
        b.fill_color = [40, 80, 200]; // blue fill
        let set = BalloonSet {
            balloons: vec![b],
            border_px: 6.0,
            pressure_width: false,
        };
        let tiles = set.rasterize((256, 256));
        let one = FIX15_ONE as u16;

        let mid = px_of(&tiles, 128, 128);
        assert_eq!(mid[3], one, "an opaque fill is still opaque");
        let want = |c: u8| ((c as f32 / 255.0) * FIX15_ONE as f32).round() as u16;
        assert_eq!([mid[0], mid[1], mid[2]], [want(40), want(80), want(200)]);
        let rim = px_of(&tiles, 128 + 70, 128);
        assert_eq!(rim[3], one, "rim opaque");
        assert!(rim[0] > rim[2], "the rim is the RED we asked for: {rim:?}");

        // Half-opaque fill: the interior thins, the outline does not.
        let mut half = set.clone();
        half.balloons[0].fill_opacity = 0.5;
        let tiles = half.rasterize((256, 256));
        let mid = px_of(&tiles, 128, 128);
        assert!(
            (mid[3] as i32 - (one / 2) as i32).abs() <= 1,
            "half-opaque interior: {mid:?}"
        );
        // Premultiplied: rgb never exceeds alpha.
        assert!(mid[0] <= mid[3] && mid[1] <= mid[3] && mid[2] <= mid[3]);
        assert_eq!(
            px_of(&tiles, 128 + 70, 128)[3],
            one,
            "the outline is intact"
        );

        // Half-opaque LINE: the rim pixel that was fully opaque above goes
        // translucent, because out at the boundary there is almost no fill
        // underneath for the thinned ink to composite over. (Sampled ON the
        // boundary rather than outside the AA band — a pixel past the band
        // is transparent at any line opacity and would prove nothing.)
        let mut faint = set.clone();
        faint.balloons[0].line_opacity = 0.4;
        let tiles = faint.rasterize((256, 256));
        let out = px_of(&tiles, 128 + 70, 128);
        assert!(out[3] > 0 && out[3] < one, "faint outline: {out:?}");
        assert!(out[0] > out[2], "still the red we asked for: {out:?}");
    }

    /// `C-04x` "fill inside frame" off is fill opacity 0: the outline inks,
    /// the interior is a hole the art shows through — and the solid-interior
    /// tiles are not allocated at all.
    #[test]
    fn zero_fill_opacity_is_an_outline_only_bubble() {
        let mut b = ellipse(200.0, 200.0, 150.0, 150.0);
        b.fill_opacity = 0.0;
        let set = BalloonSet {
            balloons: vec![b],
            border_px: 5.0,
            pressure_width: false,
        };
        let tiles = set.rasterize((512, 512));
        assert_eq!(
            px_of(&tiles, 200, 200),
            [0, 0, 0, 0],
            "the interior is a hole"
        );
        let rim = px_of(&tiles, 350, 200);
        assert_eq!(rim[3], FIX15_ONE as u16, "the outline still inks");
        assert!(rim[0] < FIX15_ONE as u16 / 8, "and it is still black");
        // Deep-interior tiles were never allocated: a 300px-wide hollow
        // balloon touches only the ring of tiles its outline crosses.
        assert!(
            tiles.len() < 40,
            "an outline-only balloon allocates the rim only ({})",
            tiles.len()
        );
    }

    /// `C-04x` toning: the fill becomes a screen. Dots at 30 % put roughly
    /// 30 % of the interior's coverage down and leave the rest of it open —
    /// and the outline is NOT screened.
    #[test]
    fn a_toned_fill_screens_the_interior_only() {
        let mut b = ellipse(128.0, 128.0, 90.0, 90.0);
        b.fill_tone = Some(BalloonTone {
            cell_px: 8.0,
            angle_deg: 45.0,
            density: 0.3,
            pattern: TonePattern::Dots,
        });
        let set = BalloonSet {
            balloons: vec![b],
            border_px: 4.0,
            pressure_width: false,
        };
        let tiles = set.rasterize((256, 256));

        // Sample a square well inside the body and average the coverage.
        let (mut sum, mut n, mut open, mut solid) = (0u64, 0u32, 0u32, 0u32);
        for y in 100..156 {
            for x in 100..156 {
                let a = px_of(&tiles, x, y)[3];
                sum += a as u64;
                n += 1;
                if a == 0 {
                    open += 1;
                }
                if a == FIX15_ONE as u16 {
                    solid += 1;
                }
            }
        }
        let mean = sum as f32 / n as f32 / FIX15_ONE as f32;
        assert!(
            (mean - 0.3).abs() < 0.06,
            "a 30 % screen prints ~30 % ink, got {mean}"
        );
        assert!(open > 100 && solid > 100, "dots AND paper: {open}/{solid}");
        // The rim is line ink, untouched by the screen.
        assert_eq!(px_of(&tiles, 128 + 90, 128)[3], FIX15_ONE as u16);
    }

    // --- tail type + bend (row 83) -----------------------------------------

    fn tailed(kind: TailKind, bend: f32) -> BalloonSet {
        let mut b = ellipse(128.0, 90.0, 55.0, 38.0);
        b.tails.push(Tail {
            base: [128.0, 120.0],
            tip: [128.0, 230.0],
            width: 34.0,
            kind,
            bend,
        });
        BalloonSet {
            balloons: vec![b],
            border_px: 4.0,
            pressure_width: false,
        }
    }

    /// `B-005`: the three tail types are three different shapes, and the
    /// default one is byte-for-byte the triangle it always was.
    #[test]
    fn tail_kinds_are_three_shapes() {
        let spoken = tailed(TailKind::Spoken, 0.0).rasterize((256, 256));
        let thought = tailed(TailKind::Thought, 0.0).rasterize((256, 256));
        let spike = tailed(TailKind::Spike, 0.0).rasterize((256, 256));

        // Spoken is the old wedge: solid all the way down the axis.
        for y in [150, 180, 210] {
            assert!(
                px_of(&spoken, 128, y)[3] > 0,
                "the spoken wedge is continuous at y={y}"
            );
        }
        // Thought is a CHAIN: there is at least one gap along the axis where
        // nothing is drawn at all.
        let gaps = (140..230)
            .filter(|&y| px_of(&thought, 128, y)[3] == 0)
            .count();
        assert!(gaps > 5, "the thought tail has clear air between its puffs");
        // And the last puff still exists near the tip.
        assert!(
            (215..235).any(|y| px_of(&thought, 128, y)[3] > 0),
            "the far puff is drawn"
        );

        // The spike's flanks bow inward: measure the inked half-width at the
        // same height and the shout tail must be the thinner one.
        let half_width = |t: &HashMap<TileIdx, Arc<Tile>>, y: i32| -> i32 {
            (0..80)
                .filter(|dx| px_of(t, 128 + dx, y)[3] > 0)
                .max()
                .unwrap_or(0)
        };
        let (hs, hk) = (half_width(&spoken, 170), half_width(&spike, 170));
        assert!(hk < hs, "the shout spike is the needle ({hk} vs {hs})");
        assert!(hk > 0, "but it is still there");
    }

    /// `B-006`: bend moves the tail off the straight base→tip line, which is
    /// how it gets around art instead of through it. Bend 0 changes nothing.
    #[test]
    fn bend_bows_the_tail_sideways() {
        let straight = tailed(TailKind::Spoken, 0.0).rasterize((256, 256));
        let bent = tailed(TailKind::Spoken, 0.45).rasterize((256, 256));

        // Midway down, the straight tail is centred on x = 128 and the bent
        // one has moved bodily to one side.
        let centroid = |t: &HashMap<TileIdx, Arc<Tile>>, y: i32| -> f32 {
            let (mut sum, mut n) = (0.0f32, 0u32);
            for x in 40..220 {
                if px_of(t, x, y)[3] > 0 {
                    sum += x as f32;
                    n += 1;
                }
            }
            if n == 0 { f32::NAN } else { sum / n as f32 }
        };
        let (cs, cb) = (centroid(&straight, 180), centroid(&bent, 180));
        assert!(
            (cs - 128.0).abs() < 2.0,
            "the straight tail is centred: {cs}"
        );
        // A POSITIVE bend goes left of base→tip. This tail points down the
        // page, and on a y-down canvas "left of down" is −x — so the bent
        // centroid must land at a SMALLER x, not a larger one.
        assert!(cs - cb > 15.0, "bend pushes it left: {cs} -> {cb}");
        let back = tailed(TailKind::Spoken, -0.45).rasterize((256, 256));
        assert!(
            centroid(&back, 180) - cs > 15.0,
            "and a negative bend goes the other way"
        );

        // The ends are pinned: base and tip do not move with the bend.
        assert!(px_of(&bent, 128, 228)[3] > 0, "the tip stays where it was");
    }

    /// The new fields survive a save/load and old JSON still lands on the
    /// neutral values (the serde-default contract, spelled out).
    #[test]
    fn new_balloon_fields_roundtrip() {
        let mut b = ellipse(50.0, 50.0, 30.0, 20.0);
        b.line_color = [10, 20, 30];
        b.fill_color = [200, 210, 220];
        b.line_opacity = 0.75;
        b.fill_opacity = 0.25;
        b.fill_tone = Some(BalloonTone::default());
        b.tails.push(Tail {
            base: [50.0, 70.0],
            tip: [50.0, 140.0],
            width: 12.0,
            kind: TailKind::Thought,
            bend: -0.3,
        });
        let set = BalloonSet {
            balloons: vec![b],
            border_px: 3.0,
            pressure_width: false,
        };
        let json = serde_json::to_string(&set).unwrap();
        let back: BalloonSet = serde_json::from_str(&json).unwrap();
        assert_eq!(back, set);

        // The pre-round shape of the JSON, once more with feeling.
        let old = r#"{"balloons":[{"shape":{"Ellipse":{"center":[0,0],"radii":[9,9]}},
            "tails":[{"base":[0,0],"tip":[0,20],"width":4.0}]}],
            "border_px":2.0,"pressure_width":false}"#;
        let loaded: BalloonSet = serde_json::from_str(old).unwrap();
        let d = Balloon::default();
        let got = &loaded.balloons[0];
        assert_eq!(
            (
                got.line_color,
                got.fill_color,
                got.line_opacity,
                got.fill_opacity,
                got.fill_tone,
                got.width_scale
            ),
            (
                d.line_color,
                d.fill_color,
                d.line_opacity,
                d.fill_opacity,
                d.fill_tone,
                d.width_scale
            )
        );
        assert_eq!(got.tails[0].kind, TailKind::Spoken);
        assert_eq!(got.tails[0].bend, 0.0);
    }

    /// `TailGeom` is the shape cache the rasterizer hoists out of its pixel
    /// loop; the default wedge must stay the allocation-free triangle.
    #[test]
    fn default_tails_keep_the_triangle_geometry() {
        let t = Tail {
            base: [0.0, 0.0],
            tip: [0.0, 50.0],
            width: 20.0,
            ..Default::default()
        };
        assert!(matches!(t.geometry(), TailGeom::Tri(_)));
        assert!(matches!(
            Tail { bend: 0.2, ..t }.geometry(),
            TailGeom::Poly(_)
        ));
        assert!(matches!(
            Tail {
                kind: TailKind::Thought,
                ..t
            }
            .geometry(),
            TailGeom::Puffs(_)
        ));
        // The bbox grows with the bend — a bent tail that reported the
        // straight triangle's box would be clipped by tile classification.
        let straight = Balloon {
            tails: vec![t],
            ..ellipse(0.0, 0.0, 10.0, 10.0)
        }
        .bbox();
        let bent = Balloon {
            tails: vec![Tail { bend: 0.6, ..t }],
            ..ellipse(0.0, 0.0, 10.0, 10.0)
        }
        .bbox();
        // The bend is POSITIVE and this tail points down (+y), so it bows to
        // −x: the box grows on its LEFT edge, not its right.
        assert!(bent[0] < straight[0] - 5.0, "{straight:?} vs {bent:?}");
        assert_eq!(bent[2], straight[2], "and not on the side it did not go");
    }

    #[test]
    fn ink_roundtrips_and_leaves_the_shape_alone() {
        let mut b = ellipse(100.0, 100.0, 50.0, 30.0);
        b.tails.push(Tail {
            base: [100.0, 120.0],
            tip: [100.0, 200.0],
            width: 20.0,
            ..Default::default()
        });
        b.width_scale = 2.5;
        let shape = b.shape.clone();

        assert_eq!(b.ink(), BalloonInk::default(), "a fresh bubble is B/W");
        let ink = BalloonInk {
            line_color: [10, 20, 30],
            fill_color: [200, 210, 220],
            line_opacity: 0.5,
            fill_opacity: 0.25,
            fill_tone: Some(BalloonTone {
                cell_px: 7.0,
                ..Default::default()
            }),
        };
        b.set_ink(ink);
        assert_eq!(b.ink(), ink);
        assert_eq!(b.shape, shape, "repaint is not a reshape");
        assert_eq!(b.width_scale, 2.5);
        assert_eq!(b.tails.len(), 1);

        // Out-of-range opacity is clamped on the way in, so the rasterizer
        // never sees a negative alpha or one above 1.
        b.set_ink(BalloonInk {
            line_opacity: 4.0,
            fill_opacity: -1.0,
            ..ink
        });
        assert_eq!(b.line_opacity, 1.0);
        assert_eq!(b.fill_opacity, 0.0);
    }

    #[test]
    fn tail_style_is_shared_and_reported() {
        let mut b = ellipse(100.0, 100.0, 50.0, 30.0);
        assert_eq!(b.tail_style(), None, "no tails, nothing to report");
        for tip in [[100.0, 200.0], [40.0, 40.0]] {
            b.tails.push(Tail {
                base: [100.0, 110.0],
                tip,
                width: 20.0,
                ..Default::default()
            });
        }
        assert_eq!(b.tail_style(), Some((TailKind::Spoken, 0.0)));
        b.set_tail_style(TailKind::Thought, 0.3);
        assert_eq!(b.tail_style(), Some((TailKind::Thought, 0.3)));
        assert!(b.tails.iter().all(|t| t.kind == TailKind::Thought));
        // A hand-mixed pair reports nothing rather than lying about one of
        // them — the panel shows its own default instead.
        b.tails[1].bend = -0.3;
        assert_eq!(b.tail_style(), None);
    }

    /// TRIAGE 134. The point is not that the box moved — it is that the item
    /// is still a text item afterwards.
    #[test]
    fn rotating_a_balloon_carries_its_lettering_editable() {
        use crate::text::{StyleRun, TextItem, TextSet};

        let body = ellipse(100.0, 100.0, 60.0, 40.0);
        let mut inside = TextItem::new([80.0, 90.0], "Gothic".into(), 9.0, [0, 0, 0], true);
        inside.text = "オイ".into();
        inside.runs = vec![StyleRun::plain(2)];
        inside.size = [40.0, 20.0];
        // Something well outside the bubble must not be dragged along.
        let mut outside = TextItem::new([400.0, 400.0], "Gothic".into(), 9.0, [0, 0, 0], true);
        outside.text = "SFX".into();
        outside.size = [40.0, 20.0];
        let (was_out_pos, was_out_rot) = (outside.pos, outside.rotation);

        let mut ts = TextSet {
            texts: vec![inside, outside],
        };
        let quarter = std::f32::consts::FRAC_PI_2;
        let moved = rotate_texts_in(&body, &mut ts, [100.0, 100.0], quarter);
        assert_eq!(moved, vec![0], "only the lettering in the bubble turns");

        let t = &ts.texts[0];
        assert!(
            (t.rotation - quarter).abs() < 1e-4,
            "the item carries the angle"
        );
        // Centre was (100,100)+(0,0) → the pivot itself; a quarter turn about
        // the pivot leaves a centred box where it was.
        let c = t.center();
        assert!(
            (c[0] - 100.0).abs() < 1e-3 && (c[1] - 100.0).abs() < 1e-3,
            "{c:?}"
        );
        // STILL EDITABLE: nothing was flattened into pixels.
        assert_eq!(t.text, "オイ");
        assert_eq!(t.runs.len(), 1);
        assert!(t.vertical, "the column direction survived the turn");
        assert!(t.cache.is_none(), "the stale sprite was dropped, not baked");
        // …and it still takes an edit.
        let mut edited = t.clone();
        edited.insert(2, "！");
        assert_eq!(edited.text, "オイ！");

        let o = &ts.texts[1];
        assert_eq!(o.pos, was_out_pos);
        assert_eq!(o.rotation, was_out_rot);

        // Off-centre lettering swings around the pivot rather than spinning
        // in place: a box left of the pivot ends up above or below it.
        let mut off = TextItem::new([40.0, 90.0], "Gothic".into(), 9.0, [0, 0, 0], false);
        off.text = "a".into();
        off.size = [20.0, 20.0];
        let mut ts2 = TextSet { texts: vec![off] };
        assert_eq!(
            rotate_texts_in(&body, &mut ts2, [100.0, 100.0], quarter).len(),
            1
        );
        let c2 = ts2.texts[0].center();
        assert!(
            (c2[0] - 100.0).abs() < 1e-3,
            "swung onto the pivot's column: {c2:?}"
        );
        assert!(c2[1] < 100.0 - 40.0, "…and up above it: {c2:?}");
    }

    /// The move's half of the carry (CSP manual, moving balloons): a
    /// translated bubble takes its lettering along — pos shifts by the
    /// exact delta, nothing reshapes (rotation and the shaped cache are
    /// untouched), and texts outside the original body stay put.
    #[test]
    fn texts_move_with_a_translated_balloon() {
        use crate::text::{StyleRun, TextItem, TextSet};
        let body = ellipse(100.0, 100.0, 60.0, 40.0);
        let mut inside = TextItem::new([80.0, 90.0], "Gothic".into(), 9.0, [0, 0, 0], true);
        inside.text = "オイ".into();
        inside.runs = vec![StyleRun::plain(2)];
        inside.size = [40.0, 20.0];
        let mut outside = TextItem::new([400.0, 400.0], "Gothic".into(), 9.0, [0, 0, 0], true);
        outside.text = "SFX".into();
        outside.size = [40.0, 20.0];
        let (was_pos, was_out_pos) = (inside.pos, outside.pos);
        let had_cache = inside.cache.is_some();

        let mut ts = TextSet {
            texts: vec![inside, outside],
        };
        let moved = translate_texts_in(&body, &mut ts, [30.0, -12.0]);
        assert_eq!(moved, vec![0], "only the lettering in the bubble moves");
        let t = &ts.texts[0];
        assert_eq!(
            t.pos,
            [was_pos[0] + 30.0, was_pos[1] - 12.0],
            "the exact delta, nothing cleverer"
        );
        assert_eq!(t.rotation, 0.0, "no reshaping");
        assert_eq!(t.cache.is_some(), had_cache, "the cache is untouched");
        assert_eq!(ts.texts[1].pos, was_out_pos, "outside texts stay put");
        assert!(
            translate_texts_in(&body, &mut ts, [0.0, 0.0]).is_empty(),
            "a zero delta is a no-op"
        );
    }

    /// The resize's half of the carry (owner, 2026-08-25): lettering inside
    /// a resized bubble keeps its same RELATIVE position — centre-fraction
    /// of the old bbox, same fraction of the new one — while the type size,
    /// rotation and shaped cache stay untouched, and texts outside the
    /// original body stay put.
    #[test]
    fn texts_keep_their_relative_place_in_a_resized_balloon() {
        use crate::text::{StyleRun, TextItem, TextSet};
        let body = ellipse(100.0, 100.0, 60.0, 40.0); // bbox 40..160 × 60..140
        let mut mid = TextItem::new([80.0, 85.0], "Gothic".into(), 9.0, [0, 0, 0], true);
        mid.text = "オイ".into();
        mid.runs = vec![StyleRun::plain(2)];
        mid.size = [40.0, 30.0]; // centre 100,100 = the 50%/50% point
        let mut off = TextItem::new([65.0, 95.0], "Gothic".into(), 9.0, [0, 0, 0], true);
        off.text = "エッ".into();
        off.runs = vec![StyleRun::plain(2)];
        off.size = [10.0, 10.0]; // centre 70,100 = the 25%/50% point
        let mut outside = TextItem::new([400.0, 400.0], "Gothic".into(), 9.0, [0, 0, 0], true);
        outside.text = "SFX".into();
        outside.size = [40.0, 20.0];
        let (mid_size, off_size, out_pos) = (mid.size, off.size, outside.pos);
        let had_cache = mid.cache.is_some();

        let mut ts = TextSet {
            texts: vec![mid, off, outside],
        };
        // Widen to 40..280 × 60..140: x doubles, y unchanged.
        let moved = scale_texts_in(&body, &mut ts, [40.0, 60.0, 280.0, 140.0]);
        assert_eq!(moved, vec![0, 1], "only the lettering in the bubble moves");
        let c = ts.texts[0].center();
        assert!(
            (c[0] - 160.0).abs() < 1e-3 && (c[1] - 100.0).abs() < 1e-3,
            "the centre keeps its 50%/50% fraction: {c:?}"
        );
        let c = ts.texts[1].center();
        assert!(
            (c[0] - 100.0).abs() < 1e-3 && (c[1] - 100.0).abs() < 1e-3,
            "the quarter-in shout stays a quarter in: {c:?}"
        );
        assert_eq!(
            ts.texts[0].size, mid_size,
            "the type size is the letterer's, not the drag's"
        );
        assert_eq!(ts.texts[1].size, off_size);
        assert_eq!(ts.texts[0].cache.is_some(), had_cache, "cache untouched");
        assert_eq!(ts.texts[2].pos, out_pos, "outside texts stay put");
        assert!(
            scale_texts_in(&body, &mut ts, [40.0, 60.0, 40.0, 140.0]).is_empty(),
            "a degenerate new box carries nothing"
        );
    }

    /// The reason [`BalloonShape::rotates_exactly`] exists: an analytic body
    /// does not tilt, so nothing may be made to follow its "rotation".
    #[test]
    fn only_a_drawn_bubble_actually_turns() {
        let mut e = ellipse(100.0, 100.0, 60.0, 40.0);
        assert!(!e.shape.rotates_exactly());
        let before = e.shape.clone();
        e.transform_around([100.0, 100.0], 1.0, 1.0, std::f32::consts::FRAC_PI_4);
        assert_eq!(e.shape, before, "an ellipse comes out of a turn unchanged");

        let mut r = Balloon {
            shape: BalloonShape::RoundRect {
                rect: [0.0, 0.0, 100.0, 60.0],
                corner: 8.0,
            },
            ..Default::default()
        };
        assert!(!r.shape.rotates_exactly());
        r.transform_around([50.0, 30.0], 1.0, 1.0, std::f32::consts::FRAC_PI_4);
        assert!(
            matches!(r.shape, BalloonShape::RoundRect { rect, .. } if rect[0] < 0.0),
            "…and a rounded rect only grows its axis-aligned extents"
        );

        let mut p = drawn_square();
        assert!(p.shape.rotates_exactly());
        p.transform_around([50.0, 50.0], 1.0, 1.0, std::f32::consts::FRAC_PI_2);
        assert!(
            matches!(&p.shape, BalloonShape::Polygon { points, .. }
                if (points[0][0] - 100.0).abs() < 1e-3 && points[0][1].abs() < 1e-3),
            "a drawn bubble turns exactly: {:?}",
            p.shape
        );
    }

    #[test]
    fn a_zero_turn_is_not_an_edit() {
        use crate::text::{TextItem, TextSet};
        let body = ellipse(100.0, 100.0, 60.0, 40.0);
        let mut t = TextItem::new([80.0, 90.0], "Gothic".into(), 9.0, [0, 0, 0], false);
        t.text = "hi".into();
        t.size = [40.0, 20.0];
        let before = t.clone();
        let mut ts = TextSet { texts: vec![t] };
        assert!(rotate_texts_in(&body, &mut ts, [100.0, 100.0], 0.0).is_empty());
        assert_eq!(ts.texts[0].pos, before.pos);
        assert_eq!(ts.texts[0].rotation, before.rotation);
    }

    // --- fit to text (ROADMAP good-first-issue #1) -------------------------

    /// Lettering with a known box: `pos`/`size` are the layout box the fit
    /// sizes against, and `em` is passed to `fit_to_text` explicitly so these
    /// tests never need DirectWrite.
    fn lettering(pos: [f32; 2], size: [f32; 2], vertical: bool) -> crate::text::TextItem {
        let mut t = crate::text::TextItem::new(pos, "Gothic".into(), 9.0, [0, 0, 0], vertical);
        t.text = "オイ".into();
        t.size = size;
        t.auto_size = false;
        t
    }

    /// Every corner of the lettering, comfortably inside the body — the one
    /// thing "fit" has to mean.
    fn holds(b: &Balloon, t: &crate::text::TextItem) -> bool {
        t.corners().into_iter().all(|p| b.shape.sdf(p) < 0.0)
    }

    const EM: f32 = 12.0; // ⇒ pad = 9 px

    #[test]
    fn fitting_a_balloon_grows_and_shrinks_to_the_text_plus_padding() {
        // Text box (80,90)…(120,110): 40 × 20 centred on (100,100).
        let t = lettering([80.0, 90.0], [40.0, 20.0], false);
        let pad = EM * FIT_PAD_EM;
        // Padded half-extents from the (fixed, coincident) centre.
        let (hx, hy) = (20.0 + pad, 10.0 + pad);
        let want = [hx * std::f32::consts::SQRT_2, hy * std::f32::consts::SQRT_2];

        // GROW: a bubble far too small for the lettering.
        let mut small = ellipse(100.0, 100.0, 12.0, 12.0);
        assert!(!holds(&small, &t), "the premise: it did not fit before");
        assert!(small.fit_to_text(&t, EM));
        let BalloonShape::Ellipse { center, radii } = &small.shape else {
            panic!("still an ellipse");
        };
        assert_eq!(*center, [100.0, 100.0], "the artist's placement is kept");
        assert!(
            (radii[0] - want[0]).abs() < 1e-3 && (radii[1] - want[1]).abs() < 1e-3,
            "sized to the padded box: {radii:?} wanted {want:?}"
        );
        assert!(holds(&small, &t), "…and the lettering is inside it now");

        // SHRINK: a bubble far too big lands on exactly the same answer.
        let mut big = ellipse(100.0, 100.0, 400.0, 300.0);
        assert!(big.fit_to_text(&t, EM));
        let BalloonShape::Ellipse { radii: r2, .. } = &big.shape else {
            panic!("still an ellipse");
        };
        assert!(
            (r2[0] - want[0]).abs() < 1e-3 && (r2[1] - want[1]).abs() < 1e-3,
            "fit is absolute, not a nudge: {r2:?}"
        );

        // Padding is proportional to the TYPE, not a constant: doubling the em
        // must leave a wider margin around the same box.
        let mut a = ellipse(100.0, 100.0, 12.0, 12.0);
        let mut b = ellipse(100.0, 100.0, 12.0, 12.0);
        a.fit_to_text(&t, EM);
        b.fit_to_text(&t, EM * 2.0);
        let (BalloonShape::Ellipse { radii: ra, .. }, BalloonShape::Ellipse { radii: rb, .. }) =
            (&a.shape, &b.shape)
        else {
            panic!("ellipses");
        };
        assert!(rb[0] > ra[0] && rb[1] > ra[1], "{ra:?} vs {rb:?}");
    }

    /// The manga convention: a 縦書き bubble is taller than it is wide.
    #[test]
    fn a_tategaki_balloon_fits_taller_than_wide() {
        // A real column of vertical lettering: narrow and tall.
        let column = lettering([90.0, 60.0], [20.0, 80.0], true);
        let mut b = ellipse(100.0, 100.0, 40.0, 40.0);
        assert!(b.fit_to_text(&column, EM));
        let BalloonShape::Ellipse { radii, .. } = &b.shape else {
            panic!("ellipse")
        };
        assert!(radii[1] > radii[0], "taller than wide: {radii:?}");
        assert!(holds(&b, &column));

        // …and even one lonely square glyph does not round off into a circle,
        // because the writing direction, not just the measured box, decides.
        let one = lettering([85.0, 85.0], [30.0, 30.0], true);
        let mut c = ellipse(100.0, 100.0, 40.0, 40.0);
        assert!(c.fit_to_text(&one, EM));
        let BalloonShape::Ellipse { radii: rv, .. } = &c.shape else {
            panic!("ellipse")
        };
        assert!(rv[1] > rv[0], "vertical stays tall: {rv:?}");

        // The same square box set HORIZONTALLY comes out the other way round.
        let mut d = ellipse(100.0, 100.0, 40.0, 40.0);
        assert!(d.fit_to_text(&lettering([85.0, 85.0], [30.0, 30.0], false), EM));
        let BalloonShape::Ellipse { radii: rh, .. } = &d.shape else {
            panic!("ellipse")
        };
        assert!(rh[0] > rh[1], "horizontal comes out wide: {rh:?}");
    }

    /// A hand-edited outline is SCALED, never replaced: the drawing's
    /// proportions, its anchors and their pressures all survive.
    #[test]
    fn fitting_a_drawn_balloon_keeps_its_shape_ratio() {
        // A 2:1 drawn blob, deliberately not a rectangle.
        let mut b = Balloon {
            shape: BalloonShape::Polygon {
                points: vec![
                    [0.0, 50.0],
                    [40.0, 0.0],
                    [160.0, 0.0],
                    [200.0, 50.0],
                    [160.0, 100.0],
                    [40.0, 100.0],
                ],
                widths: vec![0.3, 0.5, 0.9, 0.5, 0.7, 0.4],
                corners: vec![false, true, false, false, true, false],
            },
            ..Default::default()
        };
        let before = b.shape.raw_extent();
        let ratio = before[0] / before[1];
        let was = b.shape.clone();

        // Lettering centred on the blob's centre (100, 50) but too big for it.
        let t = lettering([10.0, 10.0], [180.0, 80.0], false);
        assert!(!holds(&b, &t), "the premise: it did not fit before");
        assert!(b.fit_to_text(&t, EM));

        let BalloonShape::Polygon {
            points,
            widths,
            corners,
        } = &b.shape
        else {
            panic!("a drawn bubble is never reset to an ellipse");
        };
        assert_eq!(points.len(), 6, "the anchors are still the artist's");
        assert_eq!(
            widths.as_slice(),
            [0.3, 0.5, 0.9, 0.5, 0.7, 0.4],
            "pen pressures kept"
        );
        assert_eq!(
            corners.as_slice(),
            [false, true, false, false, true, false],
            "corner flags kept"
        );
        let after = b.shape.raw_extent();
        assert!(
            (after[0] / after[1] - ratio).abs() < 1e-3,
            "uniform scale — the hand-drawn proportions survive: {before:?} → {after:?}"
        );
        assert!(after[0] > before[0], "and it actually grew: {after:?}");
        assert!(holds(&b, &t), "…around the lettering");
        assert_ne!(&b.shape, &was);
    }

    /// A fit is a resize, so it obeys the resize rule: the tail's base rides
    /// the body, its tip stays where the speaker is.
    #[test]
    fn a_fitted_balloon_keeps_its_tail_and_its_style() {
        let mut b = ellipse(100.0, 100.0, 60.0, 40.0);
        b.tails.push(Tail {
            base: [160.0, 100.0], // on the right edge of the body
            tip: [300.0, 260.0],  // out at the speaker
            width: 20.0,
            kind: TailKind::Thought,
            bend: 0.25,
        });
        b.line_color = [10, 20, 30];
        b.fill_opacity = 0.4;
        b.width_scale = 2.0;
        let style = b.ink();

        let t = lettering([80.0, 90.0], [40.0, 20.0], false);
        assert!(b.fit_to_text(&t, EM));

        assert_eq!(b.tails.len(), 1, "the tail survived the fit");
        let tail = b.tails[0];
        assert_eq!(tail.tip, [300.0, 260.0], "the speaker did not move");
        assert_eq!(tail.width, 20.0);
        assert_eq!(tail.kind, TailKind::Thought);
        assert_eq!(tail.bend, 0.25);
        assert!(
            b.shape.sdf(tail.base).abs() < 1.0,
            "the base is still welded to the boundary: {:?}",
            tail.base
        );
        assert_eq!(b.ink(), style, "the style is not a fit's business");
        assert_eq!(b.width_scale, 2.0);
    }

    /// Off-centre lettering GROWS the bubble; it never slides it.
    #[test]
    fn fitting_never_teleports_the_balloon_off_its_art() {
        let mut b = ellipse(100.0, 100.0, 20.0, 20.0);
        // Text sitting well to the right of the bubble's centre.
        let t = lettering([140.0, 90.0], [40.0, 20.0], false);
        assert!(b.fit_to_text(&t, EM));
        let BalloonShape::Ellipse { center, .. } = &b.shape else {
            panic!("ellipse")
        };
        assert_eq!(
            *center,
            [100.0, 100.0],
            "the bubble stayed where it was put"
        );
        assert!(holds(&b, &t), "and it reached out to hold the lettering");
    }

    /// The pairing rule: geometry, topmost wins, and the tail is not a place
    /// lettering lives.
    #[test]
    fn text_in_picks_the_topmost_item_inside_the_body() {
        use crate::text::TextSet;
        let mut b = ellipse(100.0, 100.0, 60.0, 40.0);
        b.tails.push(Tail {
            base: [100.0, 130.0],
            tip: [100.0, 400.0],
            width: 30.0,
            ..Default::default()
        });
        let ts = TextSet {
            texts: vec![
                lettering([80.0, 90.0], [40.0, 20.0], false), // 0: inside
                lettering([700.0, 700.0], [40.0, 20.0], false), // 1: far away
                lettering([85.0, 95.0], [30.0, 10.0], false), // 2: inside, on top
                lettering([85.0, 330.0], [30.0, 10.0], false), // 3: over the TAIL
            ],
        };
        assert_eq!(text_in(&b, &ts), Some(2), "the topmost hit wins");

        // Lettering that OVERFLOWS a too-small bubble is still its lettering —
        // that is the whole reason to press Fit.
        let small = ellipse(100.0, 100.0, 9.0, 9.0);
        let over = TextSet {
            texts: vec![lettering([20.0, 60.0], [160.0, 80.0], false)],
        };
        assert_eq!(text_in(&small, &over), Some(0));

        // …but an empty page pairs with nothing.
        assert_eq!(
            text_in(&b, &TextSet { texts: Vec::new() }),
            None,
            "nothing to fit around"
        );
    }

    /// `FG-002`'s whole promise: the second stage's pointer is ON the curve,
    /// so what you aim at is what you get. The ends stay where stage one
    /// dragged them, and an unmoved pointer inks the baseline unchanged.
    #[test]
    fn quad_through_passes_through_the_aimed_point() {
        let a = [40.0, 200.0];
        let b = [240.0, 200.0];
        let aim = [140.0, 60.0];
        let path = quad_through(a, b, aim);

        assert!(path.len() > 32, "dense enough to ink: {}", path.len());
        assert_eq!(path[0], a, "starts where the drag started");
        assert_eq!(path[path.len() - 1], b, "ends where the drag ended");

        // The midpoint of the parameter range is the aimed point. The
        // sample list is uniform in t, so index len/2 is t≈½.
        let mid = path[path.len() / 2];
        assert!(
            (mid[0] - aim[0]).hypot(mid[1] - aim[1]) < 1.5,
            "the curve runs through the pointer, got {mid:?} for {aim:?}"
        );

        // And it BENDS: the deepest sample is far off the baseline, on the
        // aimed side, and never overshoots past the aim (a quadratic with
        // this control point peaks exactly at the aim).
        let top = path.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
        assert!(
            (top - aim[1]).abs() < 1.5,
            "peaks at the aimed point, not past it: {top}"
        );

        // Aim at the baseline's own midpoint and the curve IS the baseline —
        // the no-op that makes "release, click" behave like the line tool.
        let flat = quad_through(a, b, [140.0, 200.0]);
        for p in &flat {
            assert!((p[1] - 200.0).abs() < 0.01, "straight, got {p:?}");
        }
    }

    /// A degenerate baseline (press and release without moving) must not
    /// divide by zero or emit an empty path — the caller refuses tiny drags,
    /// but the geometry is not allowed to be the thing that breaks.
    #[test]
    fn quad_through_survives_a_zero_length_baseline() {
        let p = quad_through([10.0, 10.0], [10.0, 10.0], [10.0, 10.0]);
        assert!(p.len() >= 2, "at least a segment: {}", p.len());
        assert!(p.iter().all(|q| q[0].is_finite() && q[1].is_finite()));
    }
}
