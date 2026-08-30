//! `FI-050` — the FREEFORM gradient: two drawn guide lines, and colour that
//! flows from one to the other following their shapes.
//!
//! ## The anti-aliasing trap, designed away
//!
//! CSP's freeform gradient reads two lines that are already PIXELS on a
//! layer and walks the raster between them, which is why its own
//! documentation requires the reference lines to be drawn with
//! anti-aliasing OFF — "or it will not fill". A soft-edged line has no
//! single pixel that IS the line, so a raster walk either leaks through the
//! feathered edge or refuses to start. That failure belongs entirely to
//! reading geometry back out of pixels.
//!
//! Here the two guides ARE the gesture: the user draws them inside the
//! gradient tool itself and they never become pixels at all. There is no
//! reference raster to sniff, so there is no anti-aliasing setting to get
//! wrong — and the result is anti-aliased BY CONSTRUCTION, because the
//! parameter below is a continuous function of position rather than a
//! flood fill's discrete membership test. No AA switch, no leak, no refusal.
//!
//! ## The parameter
//!
//! For a point `p`, let `d1` be the distance to guide 1 and `d2` the
//! distance to guide 2 (each a min over the guide's segments). Then
//!
//! ```text
//! t = smoothstep(d1 / (d1 + d2))
//! ```
//!
//! which is 0 exactly on guide 1, 1 exactly on guide 2, and 0.5 on the
//! locus equidistant from both. It is defined everywhere on the canvas, so
//! unlike the linear ramp there is no "outside the drag" — [`crate::gradient::EdgeProcess`]
//! and `start from centre` have nothing to act on and are inert here
//! (documented on [`crate::gradient::Ramp::eval_unit`]). Everything else a
//! ramp carries — interior stops, flip, mixing mode (Perceptual included),
//! mixing rate, dithering — rides along for free, because the colour still
//! comes from the one `Ramp` every gradient in the app evaluates.
//!
//! BEYOND the two guides the ramp turns around rather than clamping: a
//! ratio has its extreme exactly ON each guide, and far away — where the two
//! distances converge — it drifts back toward the middle. That is the right
//! behaviour for a field with no direction of its own (there is no "past the
//! end of the drag" to hold), and it is soft enough to be invisible at page
//! scale. Bracket the area with a selection, as with every other gradient,
//! when it matters.
//!
//! The smoothstep is what makes the colour EASE into each guide instead of
//! creasing at it: the raw ratio is continuous but its gradient jumps
//! across a guide line, which reads as a visible seam along the very stroke
//! the artist drew.
//!
//! ## Cost
//!
//! Distance to a polyline is a min over its segments, and a full page is
//! tens of millions of pixels, so the whole trick is to not look at every
//! segment for every pixel. [`Freeform::window`] culls both guides to the
//! segments that could possibly be the closest one to SOME point of a given
//! box (a tile): a segment further from the box centre than
//! `nearest + 2 * half_diagonal` can never win, because no point of the box
//! is more than a half-diagonal from the centre. That bound is exact —
//! nothing is approximated, so no banding is introduced by the speed-up.
//!
//! Measured on a B4/600 page (6071×8598, 52 Mpx), `freeform_full_page_timing`:
//!
//! | guides | segments kept per tile | full-page apply |
//! |---|---|---|
//! | 80 segments (a normal drawn pair) | 7.6 of 80 | 4.6 s |
//! | 400 segments (deliberately shaky) | 24.6 of 400 | 7.0 s |
//!
//! For scale the LINEAR gradient costs 2.8 s on the same page, and both
//! tools pay it: tile allocation, the undo pre-image, the ramp evaluation
//! and the premultiplied composite. So the distance field itself is about
//! 1.8 s of a normal apply. Single-threaded and CPU-side; the wave-5 GPU
//! tile-kernel seam has a pointwise entry that this would fit, which is a
//! follow-up rather than part of this row.

/// One segment of a guide, pre-chewed for the distance query. `inv_l2` is
/// `1 / |d|²` (0 for a degenerate segment) so the innermost loop — which runs
/// tens of segments deep for every pixel of a page — multiplies instead of
/// dividing. That one substitution took a pathological full-page apply from
/// 8.3 s to 7.0 s.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seg {
    ax: f32,
    ay: f32,
    dx: f32,
    dy: f32,
    inv_l2: f32,
}

impl Seg {
    fn new(a: [f32; 2], b: [f32; 2]) -> Self {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l2 = dx * dx + dy * dy;
        Self {
            ax: a[0],
            ay: a[1],
            dx,
            dy,
            inv_l2: if l2 > 0.0 { 1.0 / l2 } else { 0.0 },
        }
    }

    /// Squared distance from `p` to this segment.
    #[inline]
    fn dist2(&self, p: [f32; 2]) -> f32 {
        let (wx, wy) = (p[0] - self.ax, p[1] - self.ay);
        let t = ((wx * self.dx + wy * self.dy) * self.inv_l2).clamp(0.0, 1.0);
        let (ex, ey) = (wx - t * self.dx, wy - t * self.dy);
        ex * ex + ey * ey
    }
}

/// One guide line, prepared for distance queries. A single-point guide
/// becomes one degenerate segment so the query has no special case (and a
/// dot guide is a legitimate thing to draw — it makes the ramp radial
/// around it).
#[derive(Clone, Debug, PartialEq)]
pub struct Guide {
    segs: Vec<Seg>,
}

/// Min squared distance from `p` to a segment list. The list is never empty
/// (a `Guide` cannot be built from no points).
#[inline]
fn list_dist2(segs: &[Seg], p: [f32; 2]) -> f32 {
    let mut best = f32::INFINITY;
    for s in segs {
        let d = s.dist2(p);
        if d < best {
            best = d;
        }
    }
    best
}

impl Guide {
    /// Build from a polyline in canvas px. Non-finite points are dropped
    /// (a stray NaN from the pointer plumbing would poison every distance);
    /// `None` when nothing usable is left.
    pub fn new(pts: &[[f32; 2]]) -> Option<Self> {
        let clean: Vec<[f32; 2]> = pts
            .iter()
            .copied()
            .filter(|p| p[0].is_finite() && p[1].is_finite())
            .collect();
        let (first, rest) = clean.split_first()?;
        let mut segs = Vec::with_capacity(rest.len().max(1));
        let mut prev = *first;
        for p in rest {
            segs.push(Seg::new(prev, *p));
            prev = *p;
        }
        if segs.is_empty() {
            // A one-point guide: the ramp goes radial about it.
            segs.push(Seg::new(*first, *first));
        }
        Some(Self { segs })
    }

    pub fn segment_count(&self) -> usize {
        self.segs.len()
    }

    /// Exact distance from `p` to the whole guide.
    pub fn dist(&self, p: [f32; 2]) -> f32 {
        list_dist2(&self.segs, p).sqrt()
    }

    /// The segments that could be the closest one to SOME point of the box
    /// centred on `c` with half-diagonal `hd`. See the module docs for why
    /// the bound is exact.
    pub fn near(&self, c: [f32; 2], hd: f32) -> Vec<Seg> {
        let best = list_dist2(&self.segs, c);
        // The half-pixel is rounding slack, not fudge: the bound itself is
        // tight, so a segment sitting exactly on it must not be dropped by
        // the last bit of a sqrt. It costs a segment or two per tile.
        let keep = best.sqrt() + 2.0 * hd + 0.5;
        let keep2 = keep * keep;
        self.segs
            .iter()
            .copied()
            .filter(|s| s.dist2(c) <= keep2)
            .collect()
    }
}

/// The ramp parameter from a pair of distances: 0 on guide 1, 1 on guide 2,
/// eased so the colour meets each guide flat instead of creasing along it.
///
/// Both distances zero means the two guides cross exactly there — the
/// parameter genuinely has no value, and 0.5 is the only answer that does
/// not favour one guide over the other.
pub fn param(d1: f32, d2: f32) -> f32 {
    let sum = d1 + d2;
    let t = if sum > 0.0 && sum.is_finite() {
        (d1 / sum).clamp(0.0, 1.0)
    } else {
        0.5
    };
    t * t * (3.0 - 2.0 * t)
}

/// The two guides of one freeform gradient.
#[derive(Clone, Debug, PartialEq)]
pub struct Freeform {
    pub a: Guide,
    pub b: Guide,
}

impl Freeform {
    /// `a` is the guide at ramp parameter 0, `b` the one at 1. `None` when
    /// either polyline has no usable point.
    pub fn new(a: &[[f32; 2]], b: &[[f32; 2]]) -> Option<Self> {
        Some(Self {
            a: Guide::new(a)?,
            b: Guide::new(b)?,
        })
    }

    /// The ramp parameter at `p`, consulting every segment. Correct
    /// everywhere and the reference the windowed form is tested against;
    /// the painter uses [`Self::window`] instead.
    pub fn t_at(&self, p: [f32; 2]) -> f32 {
        param(self.a.dist(p), self.b.dist(p))
    }

    /// Both guides culled to one box — the per-tile evaluator.
    pub fn window(&self, c: [f32; 2], hd: f32) -> Window {
        Window {
            a: self.a.near(c, hd),
            b: self.b.near(c, hd),
        }
    }
}

/// [`Freeform`] narrowed to one box. Inside that box `t_at` agrees with
/// [`Freeform::t_at`] exactly; outside it may not, so a window is used for
/// the box it was built for and thrown away.
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    a: Vec<Seg>,
    b: Vec<Seg>,
}

impl Window {
    pub fn t_at(&self, p: [f32; 2]) -> f32 {
        param(list_dist2(&self.a, p).sqrt(), list_dist2(&self.b, p).sqrt())
    }

    /// How many segments survived, per guide — the speed-up, measurable.
    pub fn segment_counts(&self) -> (usize, usize) {
        (self.a.len(), self.b.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<[f32; 2]> {
        vec![[x0, y0], [x1, y1]]
    }

    /// The defining property: 0 ON guide 1, 1 ON guide 2, 0.5 halfway, and
    /// monotone in between. Two vertical lines, so the answer is known in
    /// closed form.
    #[test]
    fn the_parameter_pins_both_guides_and_the_midline() {
        let f = Freeform::new(&line(10.0, 0.0, 10.0, 100.0), &line(90.0, 0.0, 90.0, 100.0))
            .expect("two real guides");
        assert!(f.t_at([10.0, 50.0]).abs() < 1e-6, "0 on guide 1");
        assert!((f.t_at([90.0, 50.0]) - 1.0).abs() < 1e-6, "1 on guide 2");
        assert!(
            (f.t_at([50.0, 50.0]) - 0.5).abs() < 1e-6,
            "0.5 on the midline"
        );
        // Monotone across, and it never leaves 0..=1.
        let mut prev = -1.0;
        for i in 0..=80 {
            let x = 10.0 + i as f32;
            let t = f.t_at([x, 50.0]);
            assert!((0.0..=1.0).contains(&t), "x={x} t={t}");
            assert!(t >= prev - 1e-6, "must not go backwards at x={x}");
            prev = t;
        }
        // Beyond a guide the parameter stays pinned at that guide's end
        // (there is no "outside the drag" to run off).
        assert!(f.t_at([-500.0, 50.0]) < 0.5, "past guide 1 stays guide 1's");
        assert!(f.t_at([500.0, 50.0]) > 0.5, "and past guide 2, guide 2's");
    }

    /// `FI-050`'s whole point: the gradient FOLLOWS the shapes. With guide 1
    /// bent into a V, the equidistant locus bends with it — an off-axis
    /// point that would read 0.5 under two straight lines does not.
    #[test]
    fn a_curved_guide_bends_the_parameter() {
        let straight = line(90.0, -500.0, 90.0, 500.0);
        let flat = Freeform::new(&line(10.0, -500.0, 10.0, 500.0), &straight).unwrap();
        // The SAME guide with one bump: it reaches out to x=50 around y=0
        // and is the identical straight line everywhere else.
        let bump = vec![
            [10.0, -500.0],
            [10.0, -40.0],
            [50.0, 0.0],
            [10.0, 40.0],
            [10.0, 500.0],
        ];
        let bent = Freeform::new(&bump, &straight).unwrap();

        // Level with the bump, guide 1 has come 40px nearer, so the same
        // point sits much EARLIER in the ramp than it did (0.5 — it is now
        // equidistant — instead of most of the way to guide 2).
        let p = [70.0, 0.0];
        let (t_flat, t_bent) = (flat.t_at(p), bent.t_at(p));
        assert!(
            t_bent < t_flat - 0.15,
            "the bend must move the ramp off-axis: {t_flat} -> {t_bent}"
        );
        assert!((t_bent - 0.5).abs() < 1e-6, "equidistant there: {t_bent}");
        // Far from the bump the two guides ARE the same line, and the
        // parameter is untouched — the bend is local, as a drawn guide's
        // shape should be.
        let q = [70.0, -300.0];
        assert_eq!(bent.t_at(q), flat.t_at(q), "away from the bend, unchanged");
        // The guides themselves still pin exactly, bent or not.
        assert!(bent.t_at([50.0, 0.0]).abs() < 1e-6, "the tip is still 0");
        assert!((bent.t_at([90.0, 10.0]) - 1.0).abs() < 1e-6);
    }

    /// Continuity is what makes this anti-aliased by construction: no step
    /// anywhere, and specifically none ACROSS a guide, where a raster fill
    /// would have its hard membership edge.
    #[test]
    fn the_parameter_is_continuous_across_a_guide() {
        let f = Freeform::new(&line(10.0, 0.0, 10.0, 100.0), &line(90.0, 0.0, 90.0, 100.0))
            .unwrap();
        let mut prev = f.t_at([-20.0, 50.0]);
        let mut x = -20.0;
        while x <= 120.0 {
            let t = f.t_at([x, 50.0]);
            assert!(
                (t - prev).abs() < 0.02,
                "a 0.25px step must not jump the parameter at x={x}"
            );
            prev = t;
            x += 0.25;
        }
        // The eased ends are FLAT, which is the visible difference from the
        // raw ratio: approaching a guide, the colour stops changing.
        let near = f.t_at([10.5, 50.0]);
        assert!(near < 0.002, "the ramp meets guide 1 flat, not creased");
    }

    /// The per-tile cull is EXACT: inside the box it was built for, the
    /// windowed parameter equals the all-segments one to the bit — while
    /// actually throwing most of a long guide away.
    #[test]
    fn the_window_is_exact_inside_its_box_and_still_culls() {
        // Two long zig-zags, ~200 segments each.
        let zig = |x: f32, phase: f32| -> Vec<[f32; 2]> {
            (0..=200)
                .map(|i| {
                    let y = i as f32 * 30.0;
                    [x + (i as f32 * 0.7 + phase).sin() * 12.0, y]
                })
                .collect()
        };
        let f = Freeform::new(&zig(400.0, 0.0), &zig(1200.0, 2.0)).unwrap();
        let hd = 32.0 * std::f32::consts::SQRT_2;
        for (cx, cy) in [(800.0f32, 3000.0f32), (420.0, 120.0), (2000.0, 5800.0)] {
            let w = f.window([cx, cy], hd);
            let (na, nb) = w.segment_counts();
            assert!(na >= 1 && nb >= 1, "a window is never empty");
            for dx in [-31.5f32, -10.0, 0.0, 17.0, 31.5] {
                for dy in [-31.5f32, -3.0, 0.0, 22.0, 31.5] {
                    let p = [cx + dx, cy + dy];
                    assert_eq!(
                        w.t_at(p),
                        f.t_at(p),
                        "the cull changed the answer at {p:?} (box {cx},{cy})"
                    );
                }
            }
        }
        // And it is a real speed-up in the middle of the page, where almost
        // nothing of either guide can possibly be nearest.
        let (na, nb) = f.window([800.0, 3000.0], hd).segment_counts();
        assert!(
            na < 40 && nb < 40,
            "the cull must actually drop segments: {na} {nb} of 200"
        );
    }

    /// Degenerate input does not panic and does not poison the field: a
    /// one-point guide is radial, a NaN is dropped, an empty guide refuses.
    #[test]
    fn degenerate_guides_are_handled_not_crashed() {
        assert!(Guide::new(&[]).is_none(), "no points, no guide");
        assert!(
            Guide::new(&[[f32::NAN, 0.0]]).is_none(),
            "and none once the junk is dropped"
        );
        assert!(Freeform::new(&[], &line(0.0, 0.0, 1.0, 1.0)).is_none());

        let dot = Guide::new(&[[50.0, 50.0]]).expect("a dot is a guide");
        assert_eq!(dot.segment_count(), 1);
        assert!((dot.dist([50.0, 60.0]) - 10.0).abs() < 1e-4, "radial");
        assert!((dot.dist([60.0, 50.0]) - 10.0).abs() < 1e-4);

        // A NaN in the middle of a real stroke is dropped, not propagated.
        let g = Guide::new(&[[0.0, 0.0], [f32::NAN, 5.0], [0.0, 10.0]]).unwrap();
        assert!(g.dist([3.0, 5.0]).is_finite());
        assert!((g.dist([3.0, 5.0]) - 3.0).abs() < 1e-4);

        // Coincident guides: every distance ratio is 0/0, and the answer is
        // the neutral middle rather than a NaN painted across the page.
        let same = line(0.0, 0.0, 10.0, 0.0);
        let f = Freeform::new(&same, &same).unwrap();
        assert_eq!(f.t_at([5.0, 0.0]), 0.5);
        assert!(f.t_at([5.0, 20.0]).is_finite());
    }
}
