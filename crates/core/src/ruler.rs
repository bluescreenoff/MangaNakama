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
#[derive(Clone, Copy, Debug, PartialEq)]
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
            Ruler::Perspective { .. } => (p, f32::INFINITY),
        }
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
#[derive(Clone, Debug)]
pub struct Rulers {
    pub items: Vec<Ruler>,
    /// Part 2: curve rulers live separately (their snap is segment-wise).
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
            curves: Vec::new(),
            on: false,
            special_on: true,
        }
    }
}

impl Rulers {
    /// Is this ruler's family currently snappable? (`Symmetric` is not a
    /// snap source at all, but the app gates its mirroring on the same
    /// special-family switch.)
    pub fn special_active(&self) -> bool {
        self.on && self.special_on
    }

    /// RL-030 vs RL-031: the master `on` gates every ruler; the special
    /// family additionally needs `special_on`.
    fn governed(&self, r: &Ruler) -> bool {
        match r {
            Ruler::Line { .. } | Ruler::VanishingPoint { .. } | Ruler::Perspective { .. } => true,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_ruler_projects_perpendicular() {
        let rs = Rulers {
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

/// A curve ruler (part 2): a drawn polyline; snaps onto the nearest
/// SEGMENT (finite, unlike the line ruler — the curve is the path).
#[derive(Clone, Debug, PartialEq)]
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
            .position(|r| matches!(r, Ruler::Perspective { .. }) && self.governed(r));
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
                let Ruler::Perspective { a, b } = self.items[pi] else {
                    unreachable!("position() found a Perspective");
                };
                // The third family is perpendicular TO THE EYE LEVEL, not
                // to the canvas — a tilted horizon tilts its verticals with
                // it. Degenerate a == b has no eye level: canvas-up.
                let ed = [b[0] - a[0], b[1] - a[1]];
                let en = (ed[0] * ed[0] + ed[1] * ed[1]).sqrt();
                let vert = if en < 1e-3 {
                    [0.0, 1.0]
                } else {
                    [-ed[1] / en, ed[0] / en]
                };
                // Best |cos| between the stroke's direction and each
                // family's direction at the anchor; the verticals are
                // always a candidate.
                let mut best_dot = -1.0;
                let mut line = (p0, vert);
                for vp in [a, b] {
                    let vd = [p0[0] - vp[0], p0[1] - vp[1]];
                    let vn = (vd[0] * vd[0] + vd[1] * vd[1]).sqrt();
                    if vn < 1e-3 {
                        continue; // pen starts ON the VP — every ray; vertical wins by default
                    }
                    let dot = (nd[0] * vd[0] + nd[1] * vd[1]) / vn;
                    if dot.abs() > best_dot {
                        best_dot = dot.abs();
                        line = (vp, [vd[0] / vn, vd[1] / vn]);
                    }
                }
                if (nd[0] * vert[0] + nd[1] * vert[1]).abs() > best_dot {
                    line = (p0, vert);
                }
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
}
