//! Rulers (TODO #3): snapping geometry drawn on the canvas. Part 1 shipped
//! the LINE ruler (straight edge) and the VANISHING POINT (radial fan);
//! part 2 the CURVE ruler + sticky snapping; part 3 the SPECIAL family —
//! PARALLEL line, CONCENTRIC circles, GUIDES and the SYMMETRICAL ruler
//! (which mirrors strokes instead of snapping them — the app reads it to
//! build reflection twins).
//!
//! Snapping is a pure projection: the sample lands on the NEAREST ruler
//! geometry, perpendicularly. The stroke pipeline applies it after the
//! resampler, before the stabilizer/engine — the pen slides along the
//! ruler exactly like Krita/CSP.

/// One ruler's geometry, canvas px.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Ruler {
    /// A straight edge through `a` and `b` (the infinite line, not the
    /// segment — CSP's linear ruler guides along its whole length).
    Line { a: [f32; 2], b: [f32; 2] },
    /// A vanishing point: `rays` lines through `c`, evenly spaced,
    /// starting at `angle0` radians (the creation drag's direction — so
    /// the fan aligns with how it was drawn).
    VanishingPoint { c: [f32; 2], rays: u16, angle0: f32 },
    /// RL-014 special ruler: every stroke comes out PARALLEL to `a`→`b`.
    /// Not a discrete line set — the snap keeps the component along the
    /// direction and drops the perpendicular offset, i.e. it projects onto
    /// the member of the parallel family nearest to the pen.
    Parallel { a: [f32; 2], b: [f32; 2] },
    /// RL-019 special ruler: concentric rings around `c` at `k · dr`,
    /// k = 0, 1, … The snap quantizes the pen's radius to the nearest
    /// ring. (Ellipse W/H ratio is `C-055`, deferred.)
    Concentric { c: [f32; 2], dr: f32 },
    /// RL-020: an axis-aligned guide line. Snaps the one coordinate; kept
    /// as its own kind (not a degenerate `Line`) for rendering.
    Guide { horizontal: bool, pos: f32 },
    /// RL-021 symmetrical ruler: `lines` mirror axes through `c`, evenly
    /// spaced by π/`lines`, first axis at `angle0`. Does NOT snap — the
    /// stroke pipeline reflects strokes into all `2 · lines` wedge images
    /// (the app builds dihedral twins from it).
    Symmetric {
        c: [f32; 2],
        lines: u16,
        angle0: f32,
    },
    /// RL-060/061, P-001..010 (part 4 v1): a TWO-POINT perspective set —
    /// `a`/`b` are the horizon VPs and the line through them is the eye
    /// level. Every stroke binds (by its early direction) to one of three
    /// families — rays through `a`, rays through `b`, or the verticals —
    /// and then rides the member through its first sample for the whole
    /// stroke, exactly like a pen in the groove of a physical perspective
    /// ruler. The families are CONTINUA, so a stateless nearest-snap
    /// cannot express this variant: `snap_pt` declines, and only the
    /// sticky pipeline (`snap_sticky` + [`SnapLock`]) serves it.
    Perspective { a: [f32; 2], b: [f32; 2] },
    /// A ONE-POINT perspective set: `vp` is the single vanishing point and
    /// the eye level runs through it toward `h` — a bare horizon HANDLE,
    /// not a second VP. Three families, the trio 1-pt is drawn with:
    /// orthogonals (rays through `vp`), horizontals (parallel to the eye
    /// level) and verticals (perpendicular to it). A continuum like
    /// [`Ruler::Perspective`], so again only `snap_sticky` serves it.
    Perspective1 { vp: [f32; 2], h: [f32; 2] },
    /// A THREE-POINT perspective set: `a`/`b` are the horizon VPs (the eye
    /// level is the line through them) and `z` is the vertical VP above or
    /// below it. All three families are ray fans — a 3-pt set has no
    /// parallel family at all, which is exactly what makes its verticals
    /// converge.
    Perspective3 {
        a: [f32; 2],
        b: [f32; 2],
        z: [f32; 2],
    },
}

impl Ruler {
    /// Snap `p` to this ruler: the snapped point and its squared distance
    /// (`f32::INFINITY` = no contribution — `Symmetric` never snaps).
    /// Line/VanishingPoint snap to the nearest of their discrete lines;
    /// the special rulers have continuum snaps of their own.
    fn snap_pt(&self, p: [f32; 2]) -> ([f32; 2], f32) {
        match *self {
            Ruler::Line { a, b } => {
                let q = project(p, a, [b[0] - a[0], b[1] - a[1]]);
                (q, d2(q, p))
            }
            Ruler::VanishingPoint { c, rays, angle0 } => {
                let n = rays.max(1) as usize;
                let mut best = p;
                let mut best_d2 = f32::INFINITY;
                for i in 0..n {
                    let ang = angle0 + i as f32 * std::f32::consts::TAU / n as f32;
                    let q = project(p, c, [ang.cos(), ang.sin()]);
                    let d = d2(q, p);
                    if d < best_d2 {
                        best_d2 = d;
                        best = q;
                    }
                }
                (best, best_d2)
            }
            Ruler::Parallel { a, b } => {
                // The nearest member of the family IS the projection onto
                // the line through `a` — every point has one, so this
                // always snaps (like CSP: nothing you draw is ever
                // off-direction).
                let q = project(p, a, [b[0] - a[0], b[1] - a[1]]);
                (q, d2(q, p))
            }
            Ruler::Concentric { c, dr } => {
                if dr <= f32::EPSILON {
                    return (p, f32::INFINITY);
                }
                let v = [p[0] - c[0], p[1] - c[1]];
                let r = (v[0] * v[0] + v[1] * v[1]).sqrt();
                let k = ((r / dr).round() as u32).max(0) as f32;
                let rt = k * dr;
                if r <= f32::EPSILON {
                    return (c, rt * rt);
                }
                let s = rt / r;
                let q = [c[0] + v[0] * s, c[1] + v[1] * s];
                (q, d2(q, p))
            }
            Ruler::Guide { horizontal, pos } => {
                let q = if horizontal { [p[0], pos] } else { [pos, p[1]] };
                (q, d2(q, p))
            }
            Ruler::Symmetric { .. } => (p, f32::INFINITY),
            Ruler::Perspective { .. } | Ruler::Perspective1 { .. } | Ruler::Perspective3 { .. } => {
                (p, f32::INFINITY)
            }
        }
    }
}

/// What a pointer grabbed on a ruler: one of its anchor points, or the
/// body (the whole ruler moves rigidly).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RulerGrab {
    /// An index into [`Ruler::anchors`] / [`CurveRuler::anchors`].
    Anchor(usize),
    Body,
}

/// What an anchor MEANS on its ruler (M3 phase A: self-explaining
/// rulers). The handles are otherwise index-blind points, so the overlay
/// and the status line have nothing to say about them beyond "a handle";
/// the role is what lets them name the thing under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorRole {
    /// An end of a straight-edge line ruler.
    LineEnd,
    /// An end of the PARALLEL ruler's direction segment: it aims the
    /// whole family, it is not one of the lines.
    ParallelEnd,
    /// A horizon vanishing point, numbered from 1 for its tag.
    Vp(u8),
    /// The 1-pt set's bare horizon HANDLE — it tilts the eye level, it is
    /// not a second vanishing point.
    Horizon,
    /// A 3-pt set's vertical vanishing point (the one the verticals
    /// converge to).
    VerticalVp,
    /// The centre of a radial ruler: the fan's point, the rings' middle,
    /// the mirror's origin.
    Center,
}

impl AnchorRole {
    /// The short tag drawn beside the handle. Kept to a couple of words —
    /// it sits on the artwork.
    pub fn tag(self) -> &'static str {
        match self {
            AnchorRole::LineEnd => "edge",
            AnchorRole::ParallelEnd => "direction",
            AnchorRole::Vp(1) => "VP1",
            AnchorRole::Vp(2) => "VP2",
            AnchorRole::Vp(_) => "VP",
            AnchorRole::Horizon => "eye level",
            // The third VP of a 3-pt set: numbered like its horizon pair,
            // because that is how the set is drawn and talked about.
            AnchorRole::VerticalVp => "VP3",
            AnchorRole::Center => "centre",
        }
    }

    /// The handle spoken aloud, for the status line.
    pub fn name(self) -> &'static str {
        match self {
            AnchorRole::LineEnd => "a ruler end",
            AnchorRole::ParallelEnd => "the direction handle",
            AnchorRole::Vp(1) => "vanishing point 1",
            AnchorRole::Vp(2) => "vanishing point 2",
            AnchorRole::Vp(_) => "a vanishing point",
            AnchorRole::Horizon => "the eye level handle",
            AnchorRole::VerticalVp => "the vertical vanishing point",
            AnchorRole::Center => "the ruler's centre",
        }
    }

    /// What dragging it DOES — the half that makes a ruler explain
    /// itself. A vanishing point only means anything through the lines
    /// that run to it.
    pub fn effect(self) -> &'static str {
        match self {
            AnchorRole::LineEnd => "the edge re-aims",
            AnchorRole::ParallelEnd => "every parallel re-aims",
            AnchorRole::Vp(_) => "lines toward it follow",
            AnchorRole::Horizon => "the horizon tilts, and the verticals with it",
            AnchorRole::VerticalVp => "the verticals converge to it",
            AnchorRole::Center => "the whole fan travels with it",
        }
    }

    /// The status line on grab: what it is, then what it does.
    pub fn hint(self) -> String {
        format!("{} — {}", self.name(), self.effect())
    }

    /// The same one-liner while the drag is under way.
    pub fn moving(self) -> String {
        format!("moving {} — {}", self.name(), self.effect())
    }
}

/// Moving a ruler after it exists (ROADMAP "make rulers movable"). The
/// geometry IS the ruler — there is no separate transform — so a move is
/// applied straight to the anchors and every snap after it reads the new
/// position with no invalidation step.
impl Ruler {
    /// The draggable anchor points WITH their meaning, canvas px, in
    /// creation order — the ONE source [`Ruler::anchors`] and
    /// [`Ruler::anchor_roles`] are both derived from, so the two views
    /// cannot drift out of alignment with each other or with
    /// [`Ruler::move_anchor`]'s indices. A [`Ruler::Guide`] has none: it
    /// is a bare coordinate, so the drawn line is its own handle (body
    /// drags only).
    pub fn anchors_with_roles(&self) -> Vec<([f32; 2], AnchorRole)> {
        match *self {
            Ruler::Line { a, b } => vec![(a, AnchorRole::LineEnd), (b, AnchorRole::LineEnd)],
            Ruler::Parallel { a, b } => {
                vec![(a, AnchorRole::ParallelEnd), (b, AnchorRole::ParallelEnd)]
            }
            Ruler::Perspective { a, b } => vec![(a, AnchorRole::Vp(1)), (b, AnchorRole::Vp(2))],
            // The 1-pt set's second anchor is the horizon handle: dragging
            // it TILTS the eye level (and with it the horizontals and
            // verticals) without moving the vanishing point.
            Ruler::Perspective1 { vp, h } => {
                vec![(vp, AnchorRole::Vp(1)), (h, AnchorRole::Horizon)]
            }
            Ruler::Perspective3 { a, b, z } => vec![
                (a, AnchorRole::Vp(1)),
                (b, AnchorRole::Vp(2)),
                (z, AnchorRole::VerticalVp),
            ],
            Ruler::VanishingPoint { c, .. }
            | Ruler::Concentric { c, .. }
            | Ruler::Symmetric { c, .. } => vec![(c, AnchorRole::Center)],
            Ruler::Guide { .. } => Vec::new(),
        }
    }

    /// The draggable anchor points, canvas px, in creation order.
    pub fn anchors(&self) -> Vec<[f32; 2]> {
        self.anchors_with_roles()
            .into_iter()
            .map(|(p, _)| p)
            .collect()
    }

    /// What each anchor means, index-aligned with [`Ruler::anchors`].
    pub fn anchor_roles(&self) -> Vec<AnchorRole> {
        self.anchors_with_roles()
            .into_iter()
            .map(|(_, r)| r)
            .collect()
    }

    /// Move anchor `i` by `d` (an out-of-range index is ignored). Only the
    /// point moves: ray count, ring spacing and symmetry angle are the
    /// ruler's shape, not its position — sliding a vanishing point carries
    /// its fan along instead of re-aiming it.
    pub fn move_anchor(&mut self, i: usize, d: [f32; 2]) {
        fn shift(p: &mut [f32; 2], d: [f32; 2]) {
            p[0] += d[0];
            p[1] += d[1];
        }
        match self {
            Ruler::Line { a, b }
            | Ruler::Parallel { a, b }
            | Ruler::Perspective { a, b }
            | Ruler::Perspective1 { vp: a, h: b } => match i {
                0 => shift(a, d),
                1 => shift(b, d),
                _ => {}
            },
            Ruler::Perspective3 { a, b, z } => match i {
                0 => shift(a, d),
                1 => shift(b, d),
                2 => shift(z, d),
                _ => {}
            },
            Ruler::VanishingPoint { c, .. }
            | Ruler::Concentric { c, .. }
            | Ruler::Symmetric { c, .. } => {
                if i == 0 {
                    shift(c, d)
                }
            }
            Ruler::Guide { .. } => {}
        }
    }

    /// Translate the whole ruler by `d` — every anchor moves EQUALLY, so
    /// the geometry stays rigid (a moved line ruler keeps its direction, a
    /// moved perspective set keeps its horizon length). A guide takes only
    /// the component it lives on.
    pub fn translate(&mut self, d: [f32; 2]) {
        if let Ruler::Guide { horizontal, pos } = self {
            *pos += if *horizontal { d[1] } else { d[0] };
            return;
        }
        for i in 0..self.anchors().len() {
            self.move_anchor(i, d);
        }
    }

    /// Scale the ruler's geometry about the canvas origin — `IO-060`'s
    /// share of the work resample. Ruler geometry is canvas px throughout,
    /// so this is a straight multiply; the ANGLES (`angle0`) and the ray
    /// counts are dimensionless and stay, which is right for the uniform
    /// scale a dpi change is.
    pub fn scale(&mut self, sx: f32, sy: f32, s: f32) {
        let p = |q: &mut [f32; 2]| {
            q[0] *= sx;
            q[1] *= sy;
        };
        match self {
            Ruler::Line { a, b } | Ruler::Parallel { a, b } | Ruler::Perspective { a, b } => {
                p(a);
                p(b);
            }
            Ruler::VanishingPoint { c, .. } | Ruler::Symmetric { c, .. } => p(c),
            Ruler::Concentric { c, dr } => {
                p(c);
                *dr *= s;
            }
            Ruler::Guide { horizontal, pos } => *pos *= if *horizontal { sy } else { sx },
            Ruler::Perspective1 { vp, h } => {
                p(vp);
                p(h);
            }
            Ruler::Perspective3 { a, b, z } => {
                p(a);
                p(b);
                p(z);
            }
        }
    }

    /// Squared distance from `p` to the ruler's DRAWN geometry, canvas px².
    /// For every snapping kind this is exactly the snap distance (same
    /// math — what you grab is what you draw against). The two that never
    /// snap define their own: a symmetric ruler is its axes, a perspective
    /// set is its EYE LEVEL only — the faint ray fans are decoration and
    /// would otherwise make the whole page a grab target.
    pub fn dist2(&self, p: [f32; 2]) -> f32 {
        match *self {
            Ruler::Symmetric { c, lines, angle0 } => {
                let n = lines.max(1) as usize;
                let mut best = f32::INFINITY;
                for k in 0..n {
                    let ang = angle0 + k as f32 * std::f32::consts::PI / n as f32;
                    let q = project(p, c, [ang.cos(), ang.sin()]);
                    best = best.min(d2(q, p));
                }
                best
            }
            // Every perspective set is grabbed by its EYE LEVEL. The 3-pt
            // set's vertical VP is an ANCHOR, which the hit test tries
            // first — so it stays grabbable without widening the body.
            Ruler::Perspective { a, b } | Ruler::Perspective3 { a, b, .. } => {
                let q = project(p, a, [b[0] - a[0], b[1] - a[1]]);
                d2(q, p)
            }
            Ruler::Perspective1 { vp, h } => {
                let q = project(p, vp, [h[0] - vp[0], h[1] - vp[1]]);
                d2(q, p)
            }
            _ => self.snap_pt(p).1,
        }
    }

    /// Hit test for a move grab: an anchor within `tol` wins, else the
    /// body within the SAME `tol`. Canvas px — the caller divides its
    /// screen-px tolerance by the zoom, like every other on-canvas handle.
    pub fn grab_near(&self, p: [f32; 2], tol: f32) -> Option<RulerGrab> {
        let t2 = tol * tol;
        for (i, a) in self.anchors().iter().enumerate() {
            if d2(*a, p) <= t2 {
                return Some(RulerGrab::Anchor(i));
            }
        }
        (self.dist2(p) <= t2).then_some(RulerGrab::Body)
    }
}

/// The perspective family set (the continuum kinds: 1-, 2- and 3-point).
/// They share one binding rule — the stroke's early direction picks a
/// family, the member through the anchor is then fixed for the stroke —
/// so the difference between them is only WHICH families exist.
impl Ruler {
    /// Is this one of the perspective sets? (They decline `snap_pt` and
    /// are served by [`Rulers::snap_sticky`] alone.)
    pub fn is_perspective(&self) -> bool {
        matches!(
            self,
            Ruler::Perspective { .. } | Ruler::Perspective1 { .. } | Ruler::Perspective3 { .. }
        )
    }

    /// The candidate families at anchor `p0`, each a member line
    /// `(origin, unit direction)`. Order IS the tie-break order (earlier
    /// wins ties) and the list is never empty for a perspective set: a
    /// pen sitting exactly ON a vanishing point drops that VP's ray (every
    /// ray passes through it, so it carries no direction) and the
    /// remaining families arbitrate. Non-perspective kinds get nothing.
    fn persp_families(&self, p0: [f32; 2]) -> Vec<([f32; 2], [f32; 2])> {
        // The ray through `vp` and the anchor; None when they coincide.
        fn ray(vp: [f32; 2], p0: [f32; 2]) -> Option<([f32; 2], [f32; 2])> {
            let d = [p0[0] - vp[0], p0[1] - vp[1]];
            let n = (d[0] * d[0] + d[1] * d[1]).sqrt();
            (n >= 1e-3).then(|| (vp, [d[0] / n, d[1] / n]))
        }
        // Unit direction of the eye level through `a`→`b`; a degenerate
        // eye level (a == b) falls back to canvas-horizontal, so its
        // perpendicular is canvas-up as before.
        fn eye(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
            let d = [b[0] - a[0], b[1] - a[1]];
            let n = (d[0] * d[0] + d[1] * d[1]).sqrt();
            if n < 1e-3 {
                [1.0, 0.0]
            } else {
                [d[0] / n, d[1] / n]
            }
        }
        let perp = |u: [f32; 2]| [-u[1], u[0]];
        match *self {
            // 1-pt: orthogonals to the VP, then the eye-level parallels,
            // then the verticals. Both non-ray families run through the
            // anchor — they are parallel families, not fans.
            Ruler::Perspective1 { vp, h } => {
                let u = eye(vp, h);
                ray(vp, p0)
                    .into_iter()
                    .chain([(p0, u), (p0, perp(u))])
                    .collect()
            }
            // 2-pt: the two horizon fans, then the verticals — which are
            // perpendicular TO THE EYE LEVEL, not to the canvas.
            Ruler::Perspective { a, b } => [ray(a, p0), ray(b, p0)]
                .into_iter()
                .flatten()
                .chain([(p0, perp(eye(a, b)))])
                .collect(),
            // 3-pt: three fans and nothing else.
            Ruler::Perspective3 { a, b, z } => {
                let fams: Vec<_> = [ray(a, p0), ray(b, p0), ray(z, p0)]
                    .into_iter()
                    .flatten()
                    .collect();
                if fams.is_empty() {
                    vec![(p0, perp(eye(a, b)))]
                } else {
                    fams
                }
            }
            _ => Vec::new(),
        }
    }

    /// Bind a stroke travelling in unit direction `nd` from anchor `p0` to
    /// one family: the best |cos| wins (a ray and its opposite are the
    /// same line, so the sign of the travel does not matter).
    fn persp_bind(&self, p0: [f32; 2], nd: [f32; 2]) -> Option<([f32; 2], [f32; 2])> {
        let mut best = -1.0;
        let mut line = None;
        for (o, d) in self.persp_families(p0) {
            let dot = (nd[0] * d[0] + nd[1] * d[1]).abs();
            if dot > best {
                best = dot;
                line = Some((o, d));
            }
        }
        line
    }
}

/// Project `p` onto the line (point o, direction d). Degenerate lines
/// (zero direction) snap to `o`.
fn project(p: [f32; 2], o: [f32; 2], d: [f32; 2]) -> [f32; 2] {
    let dd = d[0] * d[0] + d[1] * d[1];
    if dd <= f32::EPSILON {
        return o;
    }
    let t = ((p[0] - o[0]) * d[0] + (p[1] - o[1]) * d[1]) / dd;
    [o[0] + t * d[0], o[1] + t * d[1]]
}

fn d2(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
}

/// The active ruler set: `snap` returns the nearest-geometry projection of
/// a canvas point. Empty set (or snap off) = the point unchanged.
#[derive(Clone, Debug, PartialEq)]
pub struct Rulers {
    pub items: Vec<Ruler>,
    /// Row 149 (CSP ruler layer attachment): per-item layer binding,
    /// index-paired with `items` (None = page-wide, the historic
    /// behavior). An attached ruler snaps, draws and grabs only while
    /// ITS layer is the active one — per-panel guides on a multi-panel
    /// page without cross-panel snapping. Layer INDEX v1: stable within
    /// a file, re-attached by hand if the stack is restructured.
    pub attach: Vec<Option<usize>>,
    /// Part 2: curve rulers live separately (their snap is segment-wise).
    /// Page-wide in v1 (attachment is the `Ruler` family's cut).
    pub curves: Vec<CurveRuler>,
    /// RL-030: the master snap switch (governs every ruler kind).
    pub on: bool,
    /// RL-031: the special-family switch (parallel/concentric/guide/
    /// symmetric). `on` is still required — this only adds a veto.
    pub special_on: bool,
}

impl Default for Rulers {
    fn default() -> Self {
        Rulers {
            items: Vec::new(),
            attach: Vec::new(),
            curves: Vec::new(),
            on: false,
            special_on: true,
        }
    }
}

/// The `mnc/rulers.json` envelope. Items and curves are raw values so a
/// file written by a NEWER build with an unknown ruler kind loses that one
/// ruler here, not the whole set (the actions.json lesson).
#[derive(serde::Serialize, serde::Deserialize)]
struct RulersFile {
    #[serde(default)]
    on: bool,
    #[serde(default = "yes")]
    special_on: bool,
    #[serde(default)]
    items: Vec<serde_json::Value>,
    #[serde(default)]
    curves: Vec<serde_json::Value>,
    #[serde(default)]
    attach: Vec<Option<usize>>,
}

fn yes() -> bool {
    true
}

impl Rulers {
    /// Scale every ruler and curve ruler — `IO-060`. A perspective grid
    /// built for the page must still land on the page after the work
    /// changes resolution.
    pub fn scale(&mut self, sx: f32, sy: f32) {
        let s = 0.5 * (sx + sy);
        for r in &mut self.items {
            r.scale(sx, sy, s);
        }
        for c in &mut self.curves {
            for p in &mut c.pts {
                p[0] *= sx;
                p[1] *= sy;
            }
        }
    }

    /// Is this ruler's family currently snappable? (`Symmetric` is not a
    /// snap source at all, but the app gates its mirroring on the same
    /// special-family switch.)
    pub fn special_active(&self) -> bool {
        self.on && self.special_on
    }

    /// Serialize for the `mnc/rulers.json` zip entry. Deterministic:
    /// struct field order + Vec order, no maps.
    pub fn to_json(&self) -> String {
        let f = RulersFile {
            on: self.on,
            special_on: self.special_on,
            items: self
                .items
                .iter()
                .filter_map(|r| serde_json::to_value(r).ok())
                .collect(),
            curves: self
                .curves
                .iter()
                .filter_map(|c| serde_json::to_value(c).ok())
                .collect(),
            attach: self.attach.clone(),
        };
        serde_json::to_string(&f).unwrap_or_else(|_| "{}".into())
    }

    /// Parse the entry back. Never fails the load: garbage gives the
    /// default set, an unknown ruler kind is skipped item-by-item.
    pub fn from_json(s: &str) -> Rulers {
        let Ok(f) = serde_json::from_str::<RulersFile>(s) else {
            return Rulers::default();
        };
        let mut r = Rulers {
            items: f
                .items
                .into_iter()
                .filter_map(|v| serde_json::from_value(v).ok())
                .collect(),
            curves: f
                .curves
                .into_iter()
                .filter_map(|v| serde_json::from_value(v).ok())
                .collect(),
            on: f.on,
            special_on: f.special_on,
            attach: f.attach,
        };
        r.fix_len();
        r
    }

    /// Pad/truncate `attach` to `items.len()` — the belt under every
    /// index-pairing site (old files load with no attach vec; a direct
    /// `items.push` without a matching attach push degrades to
    /// page-wide rather than panicking).
    pub fn fix_len(&mut self) {
        self.attach.resize(self.items.len(), None);
        self.attach.truncate(self.items.len());
    }

    /// The rulers ACTIVE on `active_layer`: page-wide ones plus the
    /// ones attached to that layer. Snap, draw and grab all consult the
    /// view, never the raw set.
    pub fn for_layer(&self, active_layer: usize) -> Rulers {
        let mut v = self.clone();
        let mut items = Vec::with_capacity(self.items.len());
        let mut attach = Vec::with_capacity(self.items.len());
        // `chain(repeat)` is the belt: a ruler pushed without its attach
        // entry reads as page-wide here instead of vanishing from views.
        let page_wide = &None;
        for (r, a) in self
            .items
            .iter()
            .zip(self.attach.iter().chain(std::iter::repeat(page_wide)))
        {
            if a.is_none_or(|l| l == active_layer) {
                items.push(*r);
                attach.push(None);
            }
        }
        v.items = items;
        v.attach = attach;
        v
    }

    /// Row 149's menu bulk: bind every ruler to one layer (`None` =
    /// page-wide again).
    pub fn set_all_attach(&mut self, layer: Option<usize>) {
        self.attach.clear();
        self.attach.resize(self.items.len(), layer);
    }

    /// The count of rulers bound to a layer (status lines).
    pub fn attached_count(&self) -> usize {
        self.attach.iter().flatten().count()
    }


    /// Anything worth writing to disk? (`on` alone is not — snap state
    /// with no geometry is not a ruler set.)
    pub fn has_geometry(&self) -> bool {
        !self.items.is_empty() || !self.curves.is_empty()
    }

    /// RL-030 vs RL-031: the master `on` gates every ruler; the special
    /// family additionally needs `special_on`.
    fn governed(&self, r: &Ruler) -> bool {
        match r {
            Ruler::Line { .. } | Ruler::VanishingPoint { .. } => true,
            r if r.is_perspective() => true,
            _ => self.special_on,
        }
    }

    pub fn snap(&self, p: [f32; 2]) -> [f32; 2] {
        if !self.on {
            return p;
        }
        let mut best = p;
        let mut best_d2 = f32::INFINITY;
        for r in &self.items {
            if !self.governed(r) {
                continue;
            }
            let (q, d) = r.snap_pt(p);
            if d < best_d2 {
                best_d2 = d;
                best = q;
            }
        }
        best
    }

    /// Hit test the whole set for a move grab, TOPMOST first (later rulers
    /// draw over earlier ones, and curves draw over the line family). The
    /// index is the [`SnapLock`]-style combined index: `items` first,
    /// `curves` offset by `items.len()`.
    ///
    /// Snapping being off does not hide a ruler (they stay drawn), so it
    /// does not block a grab either.
    pub fn grab_near(&self, p: [f32; 2], tol: f32) -> Option<(usize, RulerGrab)> {
        for (j, c) in self.curves.iter().enumerate().rev() {
            if let Some(g) = c.grab_near(p, tol) {
                return Some((self.items.len() + j, g));
            }
        }
        for (i, r) in self.items.iter().enumerate().rev() {
            if let Some(g) = r.grab_near(p, tol) {
                return Some((i, g));
            }
        }
        None
    }

    /// Apply one move delta to the ruler at a combined index (see
    /// [`Rulers::grab_near`]). Deltas, not absolute points: the grab
    /// offset is preserved and a drag is just the sum of its steps.
    pub fn move_by(&mut self, k: usize, grab: RulerGrab, d: [f32; 2]) {
        let n = self.items.len();
        if let Some(r) = self.items.get_mut(k) {
            match grab {
                RulerGrab::Anchor(i) => r.move_anchor(i, d),
                RulerGrab::Body => r.translate(d),
            }
        } else if let Some(c) = self.curves.get_mut(k - n) {
            match grab {
                RulerGrab::Anchor(i) => c.move_anchor(i, d),
                RulerGrab::Body => c.translate(d),
            }
        }
    }
}

#[cfg(test)]
mod tests {    use super::*;

    /// Row 149: an attached ruler snaps ONLY on its layer; page-wide
    /// ones snap everywhere; the json round-trip keeps the bindings.
    #[test]
    fn attached_rulers_snap_only_on_their_layer() {
        let mut rs = Rulers {
            items: vec![Ruler::Guide {
                horizontal: true,
                pos: 100.0,
            }],
            attach: vec![Some(1)],
            ..Rulers::default()
        };
        rs.on = true;
        // On another layer: the view is empty — no snap.
        let off = rs.for_layer(0);
        assert_eq!(off.items.len(), 0, "hidden on other layers");
        assert_eq!(off.snap([100.0, 400.0])[1], 400.0, "no snap off-layer");
        // On its own layer: it snaps.
        let on = rs.for_layer(1);
        assert_eq!(on.items.len(), 1);
        assert_eq!(on.snap([100.0, 400.0])[1], 100.0, "snaps on its layer");
        // Page-wide (None) rulers stay everywhere.
        rs.attach = vec![None];
        assert_eq!(rs.for_layer(0).snap([100.0, 400.0])[1], 100.0);
        assert_eq!(rs.for_layer(9).snap([100.0, 400.0])[1], 100.0);

        // Round-trip: the binding survives to_json/from_json.
        rs.attach = vec![Some(3)];
        let back = Rulers::from_json(&rs.to_json());
        assert_eq!(back.attach, vec![Some(3)], "attachment persisted");
        // An old file (no attach field) degrades to page-wide.
        let old = r#"{"on":true,"special_on":true,"items":[],"curves":[]}"#;
        assert_eq!(Rulers::from_json(old).attach, Vec::<Option<usize>>::new());

        // Bulk + counts.
        rs.set_all_attach(Some(2));
        assert_eq!(rs.attached_count(), 1);
        rs.set_all_attach(None);
        assert_eq!(rs.attached_count(), 0);
    }

    #[test]
    fn line_ruler_projects_perpendicular() {
        let rs = Rulers {
            attach: Vec::new(),
            curves: Vec::new(),
            special_on: true,
            items: vec![Ruler::Line {
                a: [0.0, 100.0],
                b: [100.0, 100.0],
            }],
            on: true,
        };
        let p = rs.snap([50.0, 130.0]);
        assert!((p[1] - 100.0).abs() < 1e-4, "y snaps to the line");
        assert!((p[0] - 50.0).abs() < 1e-4, "x is preserved (perpendicular)");
        // Beyond the segment endpoints still snaps (the infinite line).
        let p = rs.snap([500.0, 0.0]);
        assert!((p[1] - 100.0).abs() < 1e-4);
    }

    #[test]
    fn diagonal_projection_math() {
        let rs = Rulers {
            attach: Vec::new(),
            curves: Vec::new(),
            special_on: true,
            items: vec![Ruler::Line {
                a: [0.0, 0.0],
                b: [10.0, 10.0],
            }],
            on: true,
        };
        // (0,10) projects to (5,5) on y=x.
        let p = rs.snap([0.0, 10.0]);
        assert!((p[0] - 5.0).abs() < 1e-3 && (p[1] - 5.0).abs() < 1e-3);
    }

    #[test]
    fn vanishing_point_snaps_to_the_nearest_ray() {
        let rs = Rulers {
            attach: Vec::new(),
            curves: Vec::new(),
            special_on: true,
            items: vec![Ruler::VanishingPoint {
                c: [0.0, 0.0],
                rays: 4,
                angle0: 0.0,
            }],
            on: true,
        };
        // 4 rays from angle0=0: 0°, 90°, 180°, 270° — the axes.
        let p = rs.snap([30.0, 5.0]);
        assert!(p[1].abs() < 1e-3, "near the x-axis snaps onto it: {p:?}");
        assert!((p[0] - 30.0).abs() < 1e-3);
        let p = rs.snap([5.0, -30.0]);
        assert!(p[0].abs() < 1e-3, "near the negative y-axis: {p:?}");
    }

    #[test]
    fn nearest_of_two_rulers_wins_and_off_means_off() {
        let rs = Rulers {
            attach: Vec::new(),
            curves: Vec::new(),
            special_on: true,
            items: vec![
                Ruler::Line {
                    a: [0.0, 10.0],
                    b: [10.0, 10.0],
                },
                Ruler::Line {
                    a: [0.0, 200.0],
                    b: [10.0, 200.0],
                },
            ],
            on: true,
        };
        assert!((rs.snap([5.0, 30.0])[1] - 10.0).abs() < 1e-3);
        let mut off = rs.clone();
        off.on = false;
        assert_eq!(off.snap([5.0, 30.0]), [5.0, 30.0], "snap off = untouched");
    }
}

#[cfg(test)]
mod move_tests {
    use super::*;

    /// A body drag translates the ruler RIGIDLY — every anchor by the same
    /// delta — and the snap afterwards is the moved line's, not a stale
    /// one: the geometry is the ruler, so there is nothing to invalidate.
    #[test]
    fn body_move_translates_every_anchor_and_the_snap_follows() {
        let mut rs = Rulers {
            attach: Vec::new(),
            items: vec![Ruler::Line {
                a: [0.0, 100.0],
                b: [100.0, 100.0],
            }],
            on: true,
            ..Default::default()
        };
        assert!((rs.snap([50.0, 130.0])[1] - 100.0).abs() < 1e-4);
        rs.move_by(0, RulerGrab::Body, [10.0, 50.0]);
        assert_eq!(
            rs.items[0],
            Ruler::Line {
                a: [10.0, 150.0],
                b: [110.0, 150.0]
            },
            "both anchors moved equally"
        );
        // Same direction (rigid), new position.
        let p = rs.snap([50.0, 130.0]);
        assert!(
            (p[1] - 150.0).abs() < 1e-4,
            "snaps to the moved line: {p:?}"
        );
        assert!((p[0] - 50.0).abs() < 1e-4, "still perpendicular");
    }

    /// An anchor drag moves ONLY that anchor — which re-aims the line, and
    /// the snap direction follows the new geometry.
    #[test]
    fn anchor_move_re_aims_and_the_snap_direction_follows() {
        let mut rs = Rulers {
            attach: Vec::new(),
            items: vec![Ruler::Line {
                a: [0.0, 0.0],
                b: [100.0, 0.0],
            }],
            on: true,
            ..Default::default()
        };
        // Drag b down by 100: the ruler is now the diagonal y = x.
        rs.move_by(0, RulerGrab::Anchor(1), [0.0, 100.0]);
        assert_eq!(
            rs.items[0],
            Ruler::Line {
                a: [0.0, 0.0],
                b: [100.0, 100.0]
            },
            "anchor 0 stayed put"
        );
        // (0,10) projects onto y = x at (5,5) — the diagonal's answer.
        let p = rs.snap([0.0, 10.0]);
        assert!(
            (p[0] - 5.0).abs() < 1e-3 && (p[1] - 5.0).abs() < 1e-3,
            "{p:?}"
        );
    }

    /// The centre family (vanishing point, symmetric, concentric) carries
    /// its SHAPE — ray count, angle, ring spacing — when it moves; only
    /// the centre travels. A guide takes the component it lives on.
    #[test]
    fn centre_rulers_and_guides_move_without_reshaping() {
        let mut rs = Rulers {
            attach: Vec::new(),
            items: vec![
                Ruler::VanishingPoint {
                    c: [0.0, 0.0],
                    rays: 4,
                    angle0: 0.0,
                },
                Ruler::Symmetric {
                    c: [10.0, 10.0],
                    lines: 3,
                    angle0: 0.5,
                },
                Ruler::Concentric {
                    c: [0.0, 0.0],
                    dr: 50.0,
                },
                Ruler::Guide {
                    horizontal: true,
                    pos: 200.0,
                },
                Ruler::Guide {
                    horizontal: false,
                    pos: 100.0,
                },
            ],
            on: true,
            ..Default::default()
        };
        for k in 0..rs.items.len() {
            rs.move_by(k, RulerGrab::Body, [30.0, 20.0]);
        }
        assert_eq!(
            rs.items[0],
            Ruler::VanishingPoint {
                c: [30.0, 20.0],
                rays: 4,
                angle0: 0.0
            }
        );
        assert_eq!(
            rs.items[1],
            Ruler::Symmetric {
                c: [40.0, 30.0],
                lines: 3,
                angle0: 0.5
            }
        );
        assert_eq!(
            rs.items[2],
            Ruler::Concentric {
                c: [30.0, 20.0],
                dr: 50.0
            }
        );
        assert_eq!(
            rs.items[3],
            Ruler::Guide {
                horizontal: true,
                pos: 220.0
            },
            "a horizontal guide takes dy only"
        );
        assert_eq!(
            rs.items[4],
            Ruler::Guide {
                horizontal: false,
                pos: 130.0
            },
            "a vertical guide takes dx only"
        );
        // The moved fan still snaps to its axes, now through (30, 20).
        let only_vp = Rulers {
            attach: Vec::new(),
            items: vec![rs.items[0]],
            on: true,
            ..Default::default()
        };
        let p = only_vp.snap([90.0, 25.0]);
        assert!((p[1] - 20.0).abs() < 1e-3, "on the moved x-axis ray: {p:?}");
    }

    /// The hit test: an anchor inside the tolerance beats the body, the
    /// body is the drawn geometry, and beyond the tolerance nothing is
    /// grabbed. The tolerance is canvas px — the caller divides screen px
    /// by the zoom, so a 10 px handle stays 10 SCREEN px at any zoom.
    #[test]
    fn grab_prefers_the_anchor_then_the_body_then_nothing() {
        let line = Ruler::Line {
            a: [0.0, 100.0],
            b: [100.0, 100.0],
        };
        assert_eq!(
            line.grab_near([3.0, 101.0], 10.0),
            Some(RulerGrab::Anchor(0))
        );
        assert_eq!(
            line.grab_near([98.0, 104.0], 10.0),
            Some(RulerGrab::Anchor(1))
        );
        // On the line, far from both ends: the body.
        assert_eq!(line.grab_near([400.0, 104.0], 10.0), Some(RulerGrab::Body));
        // Off the line by more than the tolerance: nothing.
        assert_eq!(line.grab_near([400.0, 120.0], 10.0), None);
        // The SAME canvas point, at the tolerance a 4x zoom would produce
        // (10 screen px / 4) and at a 0.25x zoom (10 / 0.25).
        assert_eq!(line.grab_near([400.0, 115.0], 10.0 / 4.0), None);
        assert_eq!(
            line.grab_near([400.0, 115.0], 10.0 / 0.25),
            Some(RulerGrab::Body),
            "zoomed out, the same 10 screen px reach further in canvas px"
        );
    }

    /// A perspective set is grabbed by its EYE LEVEL and its two VPs — not
    /// by the ray fans, which cover the page and would make every press a
    /// ruler grab.
    #[test]
    fn perspective_grabs_by_the_eye_level_and_its_vps() {
        let p = Ruler::Perspective {
            a: [-600.0, 100.0],
            b: [700.0, 100.0],
        };
        assert_eq!(
            p.grab_near([-598.0, 103.0], 10.0),
            Some(RulerGrab::Anchor(0))
        );
        assert_eq!(p.grab_near([700.0, 95.0], 10.0), Some(RulerGrab::Anchor(1)));
        assert_eq!(p.grab_near([0.0, 104.0], 10.0), Some(RulerGrab::Body));
        // Squarely on a ray through VP-a, but nowhere near the horizon.
        assert_eq!(p.grab_near([0.0, 400.0], 10.0), None);
        // A symmetric ruler, by contrast, IS its axes.
        let s = Ruler::Symmetric {
            c: [0.0, 0.0],
            lines: 2,
            angle0: 0.0,
        };
        assert_eq!(s.grab_near([500.0, 3.0], 10.0), Some(RulerGrab::Body));
        assert_eq!(s.grab_near([3.0, -500.0], 10.0), Some(RulerGrab::Body));
        assert_eq!(s.grab_near([300.0, 300.0], 10.0), None);
    }

    /// Curve rulers move like the rest: a vertex reshapes the path, the
    /// body carries the whole polyline, and the moved path snaps at its
    /// new place (clamped at its ends, as ever).
    #[test]
    fn curve_ruler_vertex_and_body_moves() {
        let mut rs = Rulers {
            items: Vec::new(),
            attach: Vec::new(),
            curves: vec![CurveRuler {
                pts: vec![[0.0, 0.0], [100.0, 0.0]],
            }],
            on: true,
            special_on: true,
        };
        // Combined index: curves start at items.len() == 0.
        assert_eq!(
            rs.grab_near([100.0, 4.0], 10.0),
            Some((0, RulerGrab::Anchor(1)))
        );
        assert_eq!(rs.grab_near([50.0, 6.0], 10.0), Some((0, RulerGrab::Body)));
        assert_eq!(rs.grab_near([50.0, 60.0], 10.0), None);
        rs.move_by(0, RulerGrab::Anchor(1), [0.0, 100.0]);
        assert_eq!(rs.curves[0].pts, vec![[0.0, 0.0], [100.0, 100.0]]);
        rs.move_by(0, RulerGrab::Body, [5.0, 5.0]);
        assert_eq!(rs.curves[0].pts, vec![[5.0, 5.0], [105.0, 105.0]]);
        let mut lock = SnapLock::default();
        let q = rs.snap_sticky([55.0, 55.0], &mut lock);
        assert!(
            (q[0] - 55.0).abs() < 1e-3 && (q[1] - 55.0).abs() < 1e-3,
            "the moved path passes through (55,55): {q:?}"
        );
    }

    /// Topmost wins: curves are drawn over the line family, and later
    /// rulers over earlier ones, so the grab order matches what the eye
    /// sees on top.
    #[test]
    fn grab_picks_the_topmost_ruler() {
        let rs = Rulers {
            attach: Vec::new(),
            items: vec![
                Ruler::Guide {
                    horizontal: true,
                    pos: 50.0,
                },
                Ruler::Guide {
                    horizontal: true,
                    pos: 51.0,
                },
            ],
            curves: vec![CurveRuler {
                pts: vec![[0.0, 52.0], [100.0, 52.0]],
            }],
            on: true,
            special_on: true,
        };
        assert_eq!(rs.grab_near([50.0, 50.0], 10.0), Some((2, RulerGrab::Body)));
        let no_curve = Rulers {
            attach: Vec::new(),
            curves: Vec::new(),
            ..rs.clone()
        };
        assert_eq!(
            no_curve.grab_near([50.0, 50.0], 10.0),
            Some((1, RulerGrab::Body)),
            "the later guide is on top"
        );
    }
}

/// A curve ruler (part 2): a drawn polyline; snaps onto the nearest
/// SEGMENT (finite, unlike the line ruler — the curve is the path).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CurveRuler {
    pub pts: Vec<[f32; 2]>,
}

impl CurveRuler {
    fn snap(&self, p: [f32; 2]) -> ([f32; 2], f32) {
        let mut best = self.pts.first().copied().unwrap_or(p);
        let mut best_d2 = f32::INFINITY;
        for w in self.pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let d = [b[0] - a[0], b[1] - a[1]];
            let dd = d[0] * d[0] + d[1] * d[1];
            if dd <= f32::EPSILON {
                continue;
            }
            // Clamped projection: the segment, not its infinite line.
            let t = (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / dd).clamp(0.0, 1.0);
            let q = [a[0] + t * d[0], a[1] + t * d[1]];
            let dx = q[0] - p[0];
            let dy = q[1] - p[1];
            let d2 = dx * dx + dy * dy;
            if d2 < best_d2 {
                best_d2 = d2;
                best = q;
            }
        }
        (best, best_d2)
    }

    /// The vertices — every one is draggable (moving one RESHAPES the
    /// path; the line rulers' anchors behave the same way).
    pub fn anchors(&self) -> &[[f32; 2]] {
        &self.pts
    }

    pub fn move_anchor(&mut self, i: usize, d: [f32; 2]) {
        if let Some(p) = self.pts.get_mut(i) {
            p[0] += d[0];
            p[1] += d[1];
        }
    }

    pub fn translate(&mut self, d: [f32; 2]) {
        for p in &mut self.pts {
            p[0] += d[0];
            p[1] += d[1];
        }
    }

    /// As [`Ruler::grab_near`]; the body is the finite path (a curve ruler
    /// snaps clamped, so past its ends there is nothing to grab either).
    pub fn grab_near(&self, p: [f32; 2], tol: f32) -> Option<RulerGrab> {
        let t2 = tol * tol;
        for (i, a) in self.pts.iter().enumerate() {
            if d2(*a, p) <= t2 {
                return Some(RulerGrab::Anchor(i));
            }
        }
        (self.snap(p).1 <= t2).then_some(RulerGrab::Body)
    }
}

/// Per-stroke state for the sticky snap pipeline. `ruler` is the locked
/// ruler index (line rulers by index, curve rulers offset by
/// `items.len()`); `anchor`/`line` carry the PERSPECTIVE binding — the
/// first sample anchors, the early travel direction picks the family,
/// and `line` (origin + direction) is the fixed member the whole stroke
/// then projects onto. Reset at `begin_stroke` and `RulerClear`.
#[derive(Default, Clone, Debug)]
pub struct SnapLock {
    pub ruler: Option<usize>,
    /// The perspective acquisition sample (family not yet chosen).
    pub anchor: Option<[f32; 2]>,
    /// The bound perspective member: `(point on line, direction)`.
    pub line: Option<([f32; 2], [f32; 2])>,
}

/// A perspective set only claims a stroke when the pen is NOT on a
/// discrete ruler: nearer than this (≤ 8 px) the discrete ruler wins
/// acquisition as before.
const PERSP_CAPTURE_D2: f32 = 64.0;
/// Travel (px) before the stroke's direction is trusted to pick the
/// perspective family — under it the anchor just updates.
const PERSP_MIN_TRAVEL: f32 = 2.0;

impl Rulers {
    /// Part 2 sticky snapping, extended by part 4: once a stroke locks
    /// onto a ruler (line set, curve, or perspective family) it STAYS on
    /// it for the whole stroke (CSP behavior: crossing rulers do not
    /// flicker mid-stroke).
    ///
    /// A [`Ruler::Perspective`] set is a continuum (every point lies on
    /// some ray through each VP), so distance cannot arbitrate: it claims
    /// the stroke unless the pen is ON a discrete ruler (≤ 8 px), and the
    /// stroke's early direction then binds the family — the member
    /// through the anchor is fixed for the stroke.
    pub fn snap_sticky(&self, p: [f32; 2], lock: &mut SnapLock) -> [f32; 2] {
        if !self.on || (self.items.is_empty() && self.curves.is_empty()) {
            return p;
        }
        // A bound perspective line holds for the whole stroke.
        if let Some((o, d)) = lock.line {
            return project(p, o, d);
        }
        // Already locked to a discrete ruler: snap only against it, with
        // a generous re-across threshold (the pen may wander far; the
        // lock holds until the stroke ends).
        if let Some(k) = lock.ruler {
            return self.snap_locked(k, p);
        }
        // Acquisition: nearest across everything, as in part 2.
        let mut best = p;
        let mut best_d2 = f32::INFINITY;
        let mut best_k = None;
        for (i, r) in self.items.iter().enumerate() {
            if !self.governed(r) {
                continue;
            }
            let (q, d) = r.snap_pt(p);
            if d < best_d2 {
                best_d2 = d;
                best = q;
                best_k = Some(i);
            }
        }
        for (j, c) in self.curves.iter().enumerate() {
            let (q, d2) = c.snap(p);
            if d2 < best_d2 {
                best_d2 = d2;
                best = q;
                best_k = Some(self.items.len() + j);
            }
        }
        // The perspective claim.
        let persp = self
            .items
            .iter()
            .position(|r| r.is_perspective() && self.governed(r));
        if let Some(pi) = persp {
            if best_d2 > PERSP_CAPTURE_D2 {
                let Some(p0) = lock.anchor else {
                    lock.anchor = Some(p);
                    return p;
                };
                let (dx, dy) = (p[0] - p0[0], p[1] - p0[1]);
                let travel = (dx * dx + dy * dy).sqrt();
                if travel < PERSP_MIN_TRAVEL {
                    return p;
                }
                let nd = [dx / travel, dy / travel];
                // Best |cos| between the stroke's direction and each
                // family's direction at the anchor — which families exist
                // is the set's own business (1-pt adds the eye-level
                // parallels, 3-pt drops the verticals for a third fan).
                let Some(line) = self.items[pi].persp_bind(p0, nd) else {
                    return p; // unreachable: a perspective set always has families
                };
                lock.ruler = Some(pi);
                lock.line = Some(line);
                return project(p, line.0, line.1);
            }
        }
        lock.ruler = best_k;
        best
    }

    fn snap_locked(&self, k: usize, p: [f32; 2]) -> [f32; 2] {
        if k < self.items.len() {
            self.items[k].snap_pt(p).0
        } else if let Some(c) = self.curves.get(k - self.items.len()) {
            c.snap(p).0
        } else {
            p
        }
    }
}

#[cfg(test)]
mod part2_tests {
    use super::*;

    /// A curve ruler snaps onto the nearest SEGMENT — clamped at the
    /// polyline's ends (unlike the infinite line ruler).
    #[test]
    fn curve_ruler_snaps_clamped_to_the_path() {
        let c = CurveRuler {
            pts: vec![[0.0, 0.0], [100.0, 0.0], [100.0, 100.0]],
        };
        // Onto the middle of the first segment.
        let (q, _) = c.snap([50.0, 20.0]);
        assert!((q[1]).abs() < 1e-3 && (q[0] - 50.0).abs() < 1e-3);
        // Around the corner: onto the vertical segment.
        let (q, _) = c.snap([120.0, 50.0]);
        assert!((q[0] - 100.0).abs() < 1e-3 && (q[1] - 50.0).abs() < 1e-3);
        // Beyond the END: clamped to the last point, not extrapolated.
        let (q, _) = c.snap([200.0, 200.0]);
        assert_eq!(q, [100.0, 100.0]);
        // Before the START: clamped to the first point.
        let (q, _) = c.snap([-50.0, -50.0]);
        assert_eq!(q, [0.0, 0.0]);
    }

    /// Part 2 hysteresis: once a stroke locks a ruler it STAYS there even
    /// when a different ruler comes nearer — the crossing-rulers flicker
    /// part 1 could produce.
    #[test]
    fn sticky_snapping_locks_for_the_stroke() {
        let rs = Rulers {
            attach: Vec::new(),
            items: vec![
                Ruler::Line {
                    a: [0.0, 0.0],
                    b: [10.0, 0.0],
                },
                Ruler::Line {
                    a: [0.0, 100.0],
                    b: [10.0, 100.0],
                },
            ],
            curves: Vec::new(),
            special_on: true,
            on: true,
        };
        let mut lock = SnapLock::default();
        // Start near ruler 0 → locks 0.
        let p = rs.snap_sticky([5.0, 5.0], &mut lock);
        assert_eq!(lock.ruler, Some(0));
        assert!(p[1].abs() < 1e-3);
        // Wander near ruler 1 — still locked to 0 (CSP stickiness).
        let p = rs.snap_sticky([5.0, 96.0], &mut lock);
        assert_eq!(lock.ruler, Some(0), "the lock holds");
        assert!(p[1].abs() < 1e-3, "still snapped to ruler 0: {p:?}");
        // A fresh stroke (lock reset) near ruler 1 locks 1.
        let mut lock2 = SnapLock::default();
        let p = rs.snap_sticky([5.0, 96.0], &mut lock2);
        assert_eq!(lock2.ruler, Some(1));
        assert!((p[1] - 100.0).abs() < 1e-3);
        // Locked to a curve ruler: stays on the path.
        let rs = Rulers {
            attach: Vec::new(),
            items: Vec::new(),
            special_on: true,
            curves: vec![CurveRuler {
                pts: vec![[0.0, 50.0], [100.0, 50.0]],
            }],
            on: true,
        };
        let mut lock3 = SnapLock::default();
        let p = rs.snap_sticky([50.0, 60.0], &mut lock3);
        assert_eq!(
            lock3.ruler,
            Some(0),
            "locked to the curve (offset items.len()=0)"
        );
        assert!(p[1] < 51.0);
        // Off = untouched even with a stale lock.
        let mut off = rs.clone();
        off.on = false;
        assert_eq!(off.snap_sticky([7.0, 9.0], &mut lock3), [7.0, 9.0]);
    }
}

#[cfg(test)]
mod part3_tests {
    use super::*;

    /// RL-014: a parallel ruler flattens any point onto the family — the
    /// direction component is preserved, the perpendicular offset dropped.
    #[test]
    fn parallel_ruler_keeps_direction_drops_offset() {
        let rs = Rulers {
            attach: Vec::new(),
            items: vec![Ruler::Parallel {
                a: [0.0, 0.0],
                b: [10.0, 0.0],
            }],
            ..Default::default()
        };
        let mut rs = rs;
        rs.on = true;
        let p = rs.snap([50.0, 73.0]);
        assert!((p[0] - 50.0).abs() < 1e-3, "x preserved: {p:?}");
        assert!(p[1].abs() < 1e-3, "perpendicular offset dropped: {p:?}");
        // Diagonal family: y = x direction.
        let rs = Rulers {
            attach: Vec::new(),
            items: vec![Ruler::Parallel {
                a: [0.0, 0.0],
                b: [10.0, 10.0],
            }],
            on: true,
            ..Default::default()
        };
        let p = rs.snap([10.0, 0.0]);
        assert!((p[0] - 5.0).abs() < 1e-3 && (p[1] - 5.0).abs() < 1e-3);
    }

    /// RL-019: concentric rings quantize the radius, keeping the angle.
    #[test]
    fn concentric_ruler_quantizes_radius() {
        let rs = Rulers {
            attach: Vec::new(),
            items: vec![Ruler::Concentric {
                c: [0.0, 0.0],
                dr: 50.0,
            }],
            on: true,
            ..Default::default()
        };
        let p = rs.snap([70.0, 0.0]); // r=70 → nearest ring r=50
        assert!((p[0] - 50.0).abs() < 1e-3 && p[1].abs() < 1e-3, "{p:?}");
        let p = rs.snap([0.0, 60.0]); // r=60 → ring 50
        assert!(p[0].abs() < 1e-3 && (p[1] - 50.0).abs() < 1e-3, "{p:?}");
        let p = rs.snap([30.0, 40.0]); // r=50 exactly — stays
        assert!((p[0] - 30.0).abs() < 1e-3 && (p[1] - 40.0).abs() < 1e-3);
        let p = rs.snap([1.0, 0.0]); // r=1 → ring 0 = the centre
        assert!(p[0].abs() < 1e-3 && p[1].abs() < 1e-3, "{p:?}");
    }

    /// RL-020: guides snap exactly one coordinate.
    #[test]
    fn guide_snaps_one_coordinate() {
        let rs = Rulers {
            attach: Vec::new(),
            items: vec![
                Ruler::Guide {
                    horizontal: true,
                    pos: 200.0,
                },
                Ruler::Guide {
                    horizontal: false,
                    pos: 100.0,
                },
            ],
            on: true,
            ..Default::default()
        };
        // Nearer the horizontal guide (distance 4 vs 40).
        let p = rs.snap([140.0, 204.0]);
        assert!((p[0] - 140.0).abs() < 1e-3 && (p[1] - 200.0).abs() < 1e-3);
        // Nearer the vertical guide.
        let p = rs.snap([101.0, 500.0]);
        assert!((p[0] - 100.0).abs() < 1e-3 && (p[1] - 500.0).abs() < 1e-3);
    }

    /// RL-031: the special-family switch vetoes special rulers but never
    /// the line family; the master `on` still gates everything.
    #[test]
    fn special_switch_vetoes_only_special_rulers() {
        let mut rs = Rulers {
            attach: Vec::new(),
            items: vec![
                Ruler::Line {
                    a: [0.0, 10.0],
                    b: [10.0, 10.0],
                },
                Ruler::Parallel {
                    a: [0.0, 200.0],
                    b: [10.0, 200.0],
                },
            ],
            on: true,
            special_on: true,
            curves: Vec::new(),
        };
        // Both live: nearer the parallel (dy=3 vs dy=7).
        let p = rs.snap([50.0, 203.0]);
        assert!((p[1] - 200.0).abs() < 1e-3, "{p:?}");
        // Special off: only the line ruler remains.
        rs.special_on = false;
        let p = rs.snap([50.0, 203.0]);
        assert!((p[1] - 10.0).abs() < 1e-3, "{p:?}");
        // Master off: untouched.
        rs.on = false;
        assert_eq!(rs.snap([50.0, 203.0]), [50.0, 203.0]);
        assert!(!rs.special_active());
    }

    /// RL-021: a symmetric ruler never snaps — it mirrors (the app's twin
    /// path), so it must not fight the snap pipeline for the lock.
    #[test]
    fn symmetric_ruler_never_snaps() {
        let rs = Rulers {
            attach: Vec::new(),
            items: vec![Ruler::Symmetric {
                c: [128.0, 128.0],
                lines: 4,
                angle0: 0.0,
            }],
            on: true,
            ..Default::default()
        };
        let mut lock = SnapLock::default();
        let p = [50.0, 60.0];
        assert_eq!(rs.snap_sticky(p, &mut lock), p, "untouched");
        assert!(lock.ruler.is_none(), "no lock taken");
    }

    /// A helper: feed a stroke's samples through the sticky pipeline.
    fn stroke(rs: &Rulers, pts: &[[f32; 2]]) -> Vec<[f32; 2]> {
        let mut lock = SnapLock::default();
        pts.iter().map(|&p| rs.snap_sticky(p, &mut lock)).collect()
    }

    /// Is `q` on the line through `o` with direction `d`? Perpendicular
    /// distance in px (the raw cross is px² — scale by |d|; the stored
    /// direction is normalized with f32 rounding).
    fn on_line(q: [f32; 2], o: [f32; 2], d: [f32; 2]) -> bool {
        let cross = (q[0] - o[0]) * d[1] - (q[1] - o[1]) * d[0];
        let n = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1.0);
        cross.abs() / n < 1e-2
    }

    /// Part 4 v1: a stroke whose early travel aims at VP-a rides the ray
    /// through a and its first sample for the WHOLE stroke — even when the
    /// raw path swings toward b or off-ray.
    #[test]
    fn perspective_binds_the_ray_by_direction() {
        let rs = Rulers {
            attach: Vec::new(),
            items: vec![Ruler::Perspective {
                a: [-600.0, 100.0],
                b: [700.0, 120.0],
            }],
            on: true,
            ..Default::default()
        };
        // Anchor at (100, 300); travel toward a = left and slightly up.
        let out = stroke(
            &rs,
            &[
                [100.0, 300.0],
                [80.0, 295.0],
                [40.0, 290.0],
                [0.0, 300.0],
                [-40.0, 250.0],
            ],
        );
        let dir = [100.0 - (-600.0), 300.0 - 100.0]; // anchor - a
        for q in &out[1..] {
            assert!(
                on_line(*q, [-600.0, 100.0], dir),
                "sample {q:?} rides the a-ray"
            );
        }
        // A stroke aiming at b instead rides b's ray.
        let out = stroke(
            &rs,
            &[
                [100.0, 300.0],
                [140.0, 295.0],
                [300.0, 270.0],
                [500.0, 400.0],
            ],
        );
        let dir = [100.0 - 700.0, 300.0 - 120.0];
        for q in &out[1..] {
            assert!(
                on_line(*q, [700.0, 120.0], dir),
                "sample {q:?} rides the b-ray"
            );
        }
    }

    /// Vertical early travel binds the verticals: x is pinned to the
    /// anchor's, whatever the later path does.
    #[test]
    fn perspective_vertical_family_pins_x() {
        let rs = Rulers {
            attach: Vec::new(),
            items: vec![Ruler::Perspective {
                a: [-600.0, 100.0],
                b: [700.0, 100.0],
            }],
            on: true,
            ..Default::default()
        };
        let out = stroke(
            &rs,
            &[
                [100.0, 300.0],
                [100.0, 340.0],
                [130.0, 420.0],
                [60.0, 600.0],
            ],
        );
        for q in &out[1..] {
            assert!((q[0] - 100.0).abs() < 1e-3, "x pinned: {q:?}");
        }
    }

    /// The third family is PERPENDICULAR TO THE EYE LEVEL, not to the
    /// canvas: tilt the horizon (Dutch angle) and the verticals tilt with
    /// it. Canvas-vertical would draw a sheared pseudo-vertical here.
    #[test]
    fn perspective_verticals_follow_a_tilted_horizon() {
        let (a, b) = ([0.0f32, 0.0], [100.0f32, 30.0]);
        let rs = Rulers {
            attach: Vec::new(),
            items: vec![Ruler::Perspective { a, b }],
            on: true,
            ..Default::default()
        };
        let ed = [b[0] - a[0], b[1] - a[1]];
        let en = (ed[0] * ed[0] + ed[1] * ed[1]).sqrt();
        let perp = [-ed[1] / en, ed[0] / en];
        let anchor = [200.0, 200.0];
        // Travel along the perpendicular, then wander: the bind holds.
        let out = stroke(
            &rs,
            &[
                anchor,
                [anchor[0] + perp[0] * 40.0, anchor[1] + perp[1] * 40.0],
                [170.0, 300.0],
                [230.0, 400.0],
            ],
        );
        for q in &out[1..] {
            assert!(
                on_line(*q, anchor, perp),
                "sample {q:?} rides the perpendicular-to-horizon member"
            );
        }
        // And the drawn direction really is perpendicular to a→b.
        let seg = [out[3][0] - out[2][0], out[3][1] - out[2][1]];
        let sn = (seg[0] * seg[0] + seg[1] * seg[1]).sqrt();
        let cos = (seg[0] * ed[0] + seg[1] * ed[1]) / (sn * en);
        assert!(cos.abs() < 1e-3, "cos to the eye level = {cos}");
    }

    /// Acquisition arbitration: a pen ON a discrete ruler (≤8 px) keeps
    /// it even while a perspective set exists; a FAR ruler loses to the
    /// set (part 2 locked nearest-wins unconditionally — the set changes
    /// that, by design).
    #[test]
    fn discrete_rulers_win_near_perspective_claims_far() {
        let rs = Rulers {
            attach: Vec::new(),
            items: vec![
                Ruler::Line {
                    a: [0.0, 300.0],
                    b: [10.0, 300.0],
                },
                Ruler::Perspective {
                    a: [-600.0, 100.0],
                    b: [700.0, 100.0],
                },
            ],
            on: true,
            ..Default::default()
        };
        // Start 4 px off the line ruler, travel along it: the LINE wins.
        let out = stroke(&rs, &[[50.0, 304.0], [80.0, 303.0], [120.0, 306.0]]);
        for q in &out[1..] {
            assert!(
                (q[1] - 300.0).abs() < 1e-3,
                "the line ruler keeps a pen on it: {q:?}"
            );
        }
        // Start 100 px away from everything, travel down: the set claims
        // (vertical family), not the far line.
        let out = stroke(&rs, &[[50.0, 400.0], [50.0, 450.0], [80.0, 600.0]]);
        for q in &out[1..] {
            assert!(
                (q[0] - 50.0).abs() < 1e-3,
                "the perspective set claims: {q:?}"
            );
        }
    }

    /// The master switch governs the set like every ruler; a fresh stroke
    /// re-binds (no sticky residue across strokes).
    #[test]
    fn perspective_respects_master_switch_and_rebinds() {
        let mut rs = Rulers {
            attach: Vec::new(),
            items: vec![Ruler::Perspective {
                a: [-600.0, 100.0],
                b: [700.0, 100.0],
            }],
            on: true,
            ..Default::default()
        };
        let a = stroke(&rs, &[[100.0, 300.0], [60.0, 290.0]]); // toward a
        assert!(on_line(a[1], [-600.0, 100.0], [700.0, 200.0]));
        rs.on = false;
        let p = [100.0, 300.0];
        let mut lock = SnapLock::default();
        assert_eq!(rs.snap_sticky(p, &mut lock), p, "off = untouched");
        rs.on = true;
        // Fresh stroke toward b now binds b's ray.
        let b = stroke(&rs, &[[100.0, 300.0], [160.0, 292.0]]);
        assert!(on_line(b[1], [700.0, 100.0], [-600.0, 200.0]));
    }

    /// ROADMAP good-first-issue #2, 1-point: ONE vanishing point plus the
    /// two parallel families every 1-pt drawing is made of — orthogonals
    /// converging on the VP, horizontals along the eye level, verticals
    /// across it. The stroke's early direction picks between them.
    #[test]
    fn one_point_binds_orthogonals_horizontals_and_verticals() {
        let vp = [300.0f32, 100.0];
        let rs = Rulers {
            attach: Vec::new(),
            items: vec![Ruler::Perspective1 {
                vp,
                h: [800.0, 100.0],
            }],
            on: true,
            ..Default::default()
        };
        let anchor = [100.0f32, 300.0];
        // Aimed at the VP, then wandering: rides the orthogonal.
        let dir = [vp[0] - anchor[0], vp[1] - anchor[1]];
        let out = stroke(
            &rs,
            &[
                anchor,
                [anchor[0] + dir[0] * 0.1, anchor[1] + dir[1] * 0.1],
                [200.0, 100.0],
                [260.0, 260.0],
            ],
        );
        for q in &out[1..] {
            assert!(on_line(*q, vp, dir), "sample {q:?} rides the orthogonal");
        }
        // Travelling along the eye level: the horizontal family through
        // the anchor — y is pinned, and it does NOT converge on the VP.
        let out = stroke(&rs, &[anchor, [160.0, 302.0], [400.0, 260.0]]);
        for q in &out[1..] {
            assert!((q[1] - 300.0).abs() < 1e-3, "y pinned to the anchor: {q:?}");
        }
        // Travelling across it: the vertical family — x pinned.
        let out = stroke(&rs, &[anchor, [102.0, 360.0], [160.0, 600.0]]);
        for q in &out[1..] {
            assert!((q[0] - 100.0).abs() < 1e-3, "x pinned: {q:?}");
        }
    }

    /// A tilted 1-pt eye level tilts BOTH parallel families with it (same
    /// rule as the 2-pt verticals): the horizon handle is what tilts, and
    /// dragging that anchor re-aims the families without moving the VP.
    #[test]
    fn one_point_families_follow_the_horizon_handle() {
        let vp = [0.0f32, 0.0];
        let mut rs = Rulers {
            attach: Vec::new(),
            items: vec![Ruler::Perspective1 {
                vp,
                h: [100.0, 0.0],
            }],
            on: true,
            ..Default::default()
        };
        // Tilt the eye level by dragging the HANDLE down 30 px.
        rs.move_by(0, RulerGrab::Anchor(1), [0.0, 30.0]);
        assert_eq!(
            rs.items[0],
            Ruler::Perspective1 {
                vp: [0.0, 0.0],
                h: [100.0, 30.0]
            },
            "the vanishing point stayed put"
        );
        let u = {
            let d = [100.0f32, 30.0];
            let n = (d[0] * d[0] + d[1] * d[1]).sqrt();
            [d[0] / n, d[1] / n]
        };
        let anchor = [200.0f32, 400.0];
        // Travel along the tilted eye level: the horizontal family now
        // runs at the tilt, not canvas-level.
        let out = stroke(
            &rs,
            &[
                anchor,
                [anchor[0] + u[0] * 40.0, anchor[1] + u[1] * 40.0],
                [400.0, 500.0],
            ],
        );
        for q in &out[1..] {
            assert!(on_line(*q, anchor, u), "sample {q:?} rides the tilt");
        }
        let seg = [out[2][0] - out[1][0], out[2][1] - out[1][1]];
        let sn = (seg[0] * seg[0] + seg[1] * seg[1]).sqrt();
        let cos = (seg[0] * u[0] + seg[1] * u[1]) / sn;
        assert!(cos.abs() > 1.0 - 1e-3, "parallel to the eye level: {cos}");
    }

    /// ROADMAP good-first-issue #2, 3-point: three fans and no parallel
    /// family. The winner is chosen exactly as 2-pt chooses — best |cos|
    /// between the early travel and each family at the anchor.
    #[test]
    fn three_point_binds_each_of_its_three_vps() {
        let (a, b, z) = ([-600.0f32, 100.0], [700.0f32, 100.0], [50.0f32, 900.0]);
        let rs = Rulers {
            attach: Vec::new(),
            items: vec![Ruler::Perspective3 { a, b, z }],
            on: true,
            ..Default::default()
        };
        let anchor = [100.0f32, 300.0];
        for vp in [a, b, z] {
            let dir = [anchor[0] - vp[0], anchor[1] - vp[1]];
            let n = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt();
            let step = [dir[0] / n * 30.0, dir[1] / n * 30.0];
            let out = stroke(
                &rs,
                &[
                    anchor,
                    [anchor[0] + step[0], anchor[1] + step[1]],
                    // Wander off: the bind holds for the stroke.
                    [
                        anchor[0] + step[0] * 6.0 + 40.0,
                        anchor[1] + step[1] * 6.0 - 25.0,
                    ],
                ],
            );
            for q in &out[1..] {
                assert!(on_line(*q, vp, dir), "sample {q:?} rides the {vp:?} ray");
            }
        }
        // The discriminator against a 2-pt set: travelling straight DOWN
        // converges on the vertical VP instead of pinning x.
        let out = stroke(&rs, &[anchor, [100.0, 360.0], [100.0, 800.0]]);
        let dir = [anchor[0] - z[0], anchor[1] - z[1]];
        for q in &out[1..] {
            assert!(on_line(*q, z, dir), "sample {q:?} rides the vertical VP");
        }
        assert!(
            (out[2][0] - anchor[0]).abs() > 1.0,
            "the verticals CONVERGE (a 2-pt set would pin x): {:?}",
            out[2]
        );
    }

    /// Movability: the new variants expose their control points, an anchor
    /// drag re-aims (only that point moves), a body drag translates every
    /// one rigidly, and the snap afterwards reads the moved geometry.
    #[test]
    fn perspective_variants_anchors_round_trip_through_moves() {
        let mut rs = Rulers {
            attach: Vec::new(),
            items: vec![
                Ruler::Perspective1 {
                    vp: [300.0, 100.0],
                    h: [800.0, 100.0],
                },
                Ruler::Perspective3 {
                    a: [-600.0, 100.0],
                    b: [700.0, 100.0],
                    z: [50.0, 900.0],
                },
            ],
            on: true,
            ..Default::default()
        };
        assert_eq!(rs.items[0].anchors(), vec![[300.0, 100.0], [800.0, 100.0]]);
        assert_eq!(
            rs.items[1].anchors(),
            vec![[-600.0, 100.0], [700.0, 100.0], [50.0, 900.0]]
        );
        for k in 0..rs.items.len() {
            rs.move_by(k, RulerGrab::Body, [10.0, 20.0]);
        }
        assert_eq!(
            rs.items[0],
            Ruler::Perspective1 {
                vp: [310.0, 120.0],
                h: [810.0, 120.0]
            }
        );
        assert_eq!(
            rs.items[1],
            Ruler::Perspective3 {
                a: [-590.0, 120.0],
                b: [710.0, 120.0],
                z: [60.0, 920.0]
            },
            "all three moved equally — the set stays rigid"
        );
        // Drag ONLY the vertical VP: the horizon is untouched.
        rs.move_by(1, RulerGrab::Anchor(2), [200.0, -1500.0]);
        assert_eq!(
            rs.items[1],
            Ruler::Perspective3 {
                a: [-590.0, 120.0],
                b: [710.0, 120.0],
                z: [260.0, -580.0]
            },
            "a worm's-eye set: the third VP above the horizon"
        );
        // And the strokes follow it — no invalidation step, the geometry
        // IS the ruler.
        let only3 = Rulers {
            attach: Vec::new(),
            items: vec![rs.items[1]],
            on: true,
            ..Default::default()
        };
        let anchor = [100.0f32, 400.0];
        let z = [260.0f32, -580.0];
        let dir = [anchor[0] - z[0], anchor[1] - z[1]];
        let mut lock = SnapLock::default();
        only3.snap_sticky(anchor, &mut lock);
        let q = only3.snap_sticky([100.0, 340.0], &mut lock);
        assert!(on_line(q, z, dir), "rides the MOVED vertical VP: {q:?}");
    }

    /// The hit test: every vanishing point is an anchor (including the
    /// 3-pt set's vertical VP, which sits far off the horizon), the body
    /// is the EYE LEVEL only, and the ray fans are not grab targets.
    #[test]
    fn perspective_variants_grab_their_vps_and_eye_levels() {
        let p1 = Ruler::Perspective1 {
            vp: [300.0, 100.0],
            h: [800.0, 100.0],
        };
        assert_eq!(
            p1.grab_near([302.0, 103.0], 10.0),
            Some(RulerGrab::Anchor(0))
        );
        assert_eq!(
            p1.grab_near([800.0, 95.0], 10.0),
            Some(RulerGrab::Anchor(1)),
            "the horizon handle is draggable too"
        );
        assert_eq!(p1.grab_near([-400.0, 104.0], 10.0), Some(RulerGrab::Body));
        assert_eq!(
            p1.grab_near([300.0, 400.0], 10.0),
            None,
            "on an orthogonal, far from the eye level: nothing"
        );
        let p3 = Ruler::Perspective3 {
            a: [-600.0, 100.0],
            b: [700.0, 100.0],
            z: [50.0, 900.0],
        };
        assert_eq!(
            p3.grab_near([-598.0, 103.0], 10.0),
            Some(RulerGrab::Anchor(0))
        );
        assert_eq!(
            p3.grab_near([700.0, 95.0], 10.0),
            Some(RulerGrab::Anchor(1))
        );
        assert_eq!(
            p3.grab_near([52.0, 903.0], 10.0),
            Some(RulerGrab::Anchor(2)),
            "the vertical VP grabs even though it is off the eye level"
        );
        assert_eq!(p3.grab_near([0.0, 104.0], 10.0), Some(RulerGrab::Body));
        assert_eq!(p3.grab_near([0.0, 400.0], 10.0), None);
        // Through the set, with the combined index: topmost (later) first,
        // and an anchor of a lower ruler still wins when nothing above it
        // is in reach.
        let rs = Rulers {
            attach: Vec::new(),
            items: vec![p3, p1],
            on: true,
            ..Default::default()
        };
        assert_eq!(
            rs.grab_near([302.0, 103.0], 10.0),
            Some((1, RulerGrab::Anchor(0))),
            "the 1-pt set is on top"
        );
        assert_eq!(
            rs.grab_near([52.0, 903.0], 10.0),
            Some((0, RulerGrab::Anchor(2)))
        );
    }

    /// Every ruler kind, one of each — the roster the role tests loop. A
    /// NEW variant fails to compile inside `anchors_with_roles` first, so
    /// this list is a reminder, not the guard.
    fn one_of_every_variant() -> Vec<Ruler> {
        vec![
            Ruler::Line {
                a: [0.0, 0.0],
                b: [100.0, 0.0],
            },
            Ruler::VanishingPoint {
                c: [50.0, 50.0],
                rays: 12,
                angle0: 0.3,
            },
            Ruler::Parallel {
                a: [0.0, 0.0],
                b: [40.0, 60.0],
            },
            Ruler::Concentric {
                c: [10.0, 20.0],
                dr: 25.0,
            },
            Ruler::Guide {
                horizontal: true,
                pos: 200.0,
            },
            Ruler::Symmetric {
                c: [30.0, 40.0],
                lines: 4,
                angle0: 0.1,
            },
            Ruler::Perspective {
                a: [-300.0, 100.0],
                b: [500.0, 100.0],
            },
            Ruler::Perspective1 {
                vp: [300.0, 100.0],
                h: [800.0, 100.0],
            },
            Ruler::Perspective3 {
                a: [-600.0, 100.0],
                b: [700.0, 100.0],
                z: [50.0, 900.0],
            },
        ]
    }

    /// M3 phase A: the roles are index-aligned with the anchors for EVERY
    /// kind, because both views come off one array. The alignment that
    /// actually bites is with `move_anchor` — role i names the point
    /// `Anchor(i)` drags — so the test drags each index and checks the
    /// point at that index, and nothing else, moved.
    #[test]
    fn anchor_roles_align_with_anchors_for_every_variant() {
        for r in one_of_every_variant() {
            let pairs = r.anchors_with_roles();
            let pts = r.anchors();
            let roles = r.anchor_roles();
            assert_eq!(roles.len(), pts.len(), "{r:?}");
            assert_eq!(pairs.len(), pts.len(), "{r:?}");
            for (i, (p, role)) in pairs.iter().enumerate() {
                assert_eq!(*p, pts[i], "point {i} of {r:?}");
                assert_eq!(*role, roles[i], "role {i} of {r:?}");
                assert!(!role.tag().is_empty() && !role.hint().is_empty());
            }
            for i in 0..pts.len() {
                let mut moved = r;
                moved.move_anchor(i, [7.0, -3.0]);
                for (j, q) in moved.anchors().iter().enumerate() {
                    let want = if j == i {
                        [pts[j][0] + 7.0, pts[j][1] - 3.0]
                    } else {
                        pts[j]
                    };
                    assert_eq!(*q, want, "moving anchor {i} of {r:?} moved index {j}");
                }
            }
        }
    }

    /// The naming itself, since the labels ARE the feature: the
    /// perspective sets say which vanishing point is which, the 1-pt
    /// set's far handle is the eye level and not a VP, and a guide has
    /// nothing to name.
    #[test]
    fn anchor_roles_name_each_perspective_handle() {
        let roles = |r: Ruler| r.anchor_roles();
        assert_eq!(
            roles(Ruler::Perspective {
                a: [0.0, 0.0],
                b: [1.0, 0.0]
            }),
            vec![AnchorRole::Vp(1), AnchorRole::Vp(2)]
        );
        assert_eq!(
            roles(Ruler::Perspective1 {
                vp: [0.0, 0.0],
                h: [1.0, 0.0]
            }),
            vec![AnchorRole::Vp(1), AnchorRole::Horizon]
        );
        assert_eq!(
            roles(Ruler::Perspective3 {
                a: [0.0, 0.0],
                b: [1.0, 0.0],
                z: [0.0, 9.0]
            }),
            vec![AnchorRole::Vp(1), AnchorRole::Vp(2), AnchorRole::VerticalVp]
        );
        assert_eq!(
            roles(Ruler::VanishingPoint {
                c: [0.0, 0.0],
                rays: 8,
                angle0: 0.0
            }),
            vec![AnchorRole::Center]
        );
        assert!(
            roles(Ruler::Guide {
                horizontal: false,
                pos: 10.0
            })
            .is_empty(),
            "a guide is its own handle"
        );
        assert_eq!(AnchorRole::Vp(2).tag(), "VP2");
        assert_eq!(AnchorRole::VerticalVp.tag(), "VP3");
        assert_eq!(AnchorRole::Horizon.tag(), "eye level");
    }
}
