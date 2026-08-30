//! `FI-050`/`FI-051` — the FREEFORM gradient: drawn guide lines, and colour
//! that flows between them following their shapes. TWO guides run a ramp from
//! one to the other; THREE OR MORE carry a colour each and blend by
//! proximity, which is CSP's model.
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

//! ## `FI-051` — three guides and up: colour per guide
//!
//! With N ≥ 3 there is no "from" and no "to", so there is no ramp parameter
//! to compute: each guide carries its own colour and a pixel is the
//! INVERSE-DISTANCE-WEIGHTED blend of them ([`Multi::colour_at`]), Shepard's
//! method with p = 2:
//!
//! ```text
//! c(p) = Σ (c_i / d_i²) / Σ (1 / d_i²)
//! ```
//!
//! p = 2 rather than p = 1 for the same reason the two-guide path has a
//! smoothstep: at p = 2 the field's gradient goes to ZERO at each guide, so
//! the colour meets the drawn stroke flat instead of creasing along it. (At
//! p = 1 the weights are exactly the two-guide `d1/(d1+d2)` ratio when N = 2
//! — the raw one, BEFORE the smoothstep, and without the `Ramp` around it.)
//!
//! **N = 2 does not come through here.** The two-line path is the shipped,
//! pinned one: `t = smoothstep(d1/(d1+d2))` through the one
//! [`crate::gradient::Ramp`], which
//! is what carries interior stops, flip, the mixing rate and the edge
//! process. IDW would reproduce neither the easing nor any of those, so
//! `Document::paint_gradient_freeform_multi` ROUTES two guides to
//! [`Freeform`] instead of degenerating into them, and a test paints both
//! ways and compares the tiles byte for byte
//! (`two_guides_still_take_the_pinned_ramp_path`).
//!
//! What a colour-per-guide field cannot carry, and does not pretend to: the
//! ramp's interior stops, `flip`, `EdgeProcess` and the mixing RATE all
//! describe positions along one ramp, and there is no ramp here. The mixing
//! SPACE ([`crate::mix::MixMode`]), the brightness lift and dithering do
//! apply — they are per-mix, not per-position. With `bright > 0` the lift
//! is applied at each pairwise step of the fold rather than once, so it is
//! stronger than on a two-stop ramp; that is a taste knob, not a promise.
//!
//! ## The cull, with N guides
//!
//! Every guide contributes to every pixel — a weight is only ever small, it
//! is never zero — so **no guide may be dropped from a tile**, however far
//! away it is. Dropping the far one is exactly the naive "two nearest"
//! cull, and it shifts the colour of a tile that sits between two near
//! guides while a third pulls on it
//! (`a_far_guide_still_tints_a_tile_so_the_cull_keeps_it`).
//!
//! What IS culled is what was culled before: SEGMENTS inside each guide,
//! because a guide's contribution depends only on its nearest segment. That
//! bound is exact ([`Guide::near`]), so the windowed answer equals the
//! all-segments one to the bit, and a long shaky guide still costs a
//! handful of segments per tile.

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

// --- `FI-051`: N guides, a colour each ------------------------------------

/// One guide of an N-line freeform gradient: the polyline as drawn, and the
/// straight RGBA that lands ON it. The app's gesture stores these directly,
/// so the preview overlay and the paint see the same colours.
#[derive(Clone, Debug, PartialEq)]
pub struct ColourGuide {
    pub pts: Vec<[f32; 2]>,
    pub colour: [f32; 4],
}

impl ColourGuide {
    pub fn new(pts: Vec<[f32; 2]>, colour: [f32; 4]) -> Self {
        Self { pts, colour }
    }
}

/// The inverse-distance blend at one point, given each guide's SQUARED
/// distance and colour. One pass, no allocation: the running mix is exact
/// for any lerp-in-a-fixed-space [`MixMode`], which is what all three modes
/// are (see the module doc for the `bright > 0` caveat).
///
/// A pixel sitting exactly ON a guide takes that guide's colour outright —
/// the limit the weights are heading for anyway, and the only way to avoid
/// dividing by zero without perturbing the field.
fn idw_colour(
    it: impl Iterator<Item = (f32, [f32; 4])>,
    mix: crate::mix::MixMode,
    bright: u8,
) -> [f32; 4] {
    let mut acc = [0.0f32; 4];
    let mut wsum = 0.0f32;
    let mut started = false;
    for (d2, c) in it {
        if !(d2 > 0.0) {
            return c;
        }
        let w = 1.0 / d2;
        if !started {
            acc = c;
            wsum = w;
            started = true;
            continue;
        }
        let s = w / (wsum + w);
        acc = crate::mix::mix_rgba(mix, acc, c, s, bright);
        wsum += w;
    }
    acc
}

/// `FI-051` — three or more guides, each carrying its own colour.
///
/// Two guides never reach this type; see the module doc for why the shipped
/// [`Freeform`] ramp path is kept instead of degenerating into it.
#[derive(Clone, Debug, PartialEq)]
pub struct Multi {
    lines: Vec<(Guide, [f32; 4])>,
}

impl Multi {
    /// `None` when any guide has no usable point — the same rule
    /// [`Freeform::new`] follows, and for the same reason: a gradient with a
    /// missing guide is not a gradient with one fewer.
    pub fn new(lines: &[ColourGuide]) -> Option<Self> {
        if lines.is_empty() {
            return None;
        }
        let mut out = Vec::with_capacity(lines.len());
        for l in lines {
            out.push((Guide::new(&l.pts)?, l.colour));
        }
        Some(Self { lines: out })
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// The colour at `p`, consulting every segment of every guide. Correct
    /// everywhere and the reference [`Self::window`] is tested against; the
    /// painter uses the window.
    pub fn colour_at(&self, p: [f32; 2], mix: crate::mix::MixMode, bright: u8) -> [f32; 4] {
        idw_colour(
            self.lines
                .iter()
                .map(|(g, c)| (list_dist2(&g.segs, p), *c)),
            mix,
            bright,
        )
    }

    /// Every guide culled to one box — the per-tile evaluator. EVERY guide
    /// survives (a far one still tints the tile); what is dropped is the
    /// segments inside each that cannot be its nearest one, which is exact.
    pub fn window(&self, c: [f32; 2], hd: f32) -> MultiWindow {
        MultiWindow {
            lines: self
                .lines
                .iter()
                .map(|(g, col)| (g.near(c, hd), *col))
                .collect(),
        }
    }
}

/// [`Multi`] narrowed to one box. Inside that box `colour_at` agrees with
/// [`Multi::colour_at`] exactly; outside it may not, so a window is used for
/// the box it was built for and thrown away.
#[derive(Clone, Debug, PartialEq)]
pub struct MultiWindow {
    lines: Vec<(Vec<Seg>, [f32; 4])>,
}

impl MultiWindow {
    pub fn colour_at(&self, p: [f32; 2], mix: crate::mix::MixMode, bright: u8) -> [f32; 4] {
        idw_colour(
            self.lines.iter().map(|(s, c)| (list_dist2(s, p), *c)),
            mix,
            bright,
        )
    }

    /// How many segments survived, per guide — the speed-up, measurable,
    /// and the proof that no GUIDE was dropped (the length is the guide
    /// count, always).
    pub fn segment_counts(&self) -> Vec<usize> {
        self.lines.iter().map(|(s, _)| s.len()).collect()
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

    // --- `FI-051`: N guides, a colour each --------------------------------

    use crate::mix::MixMode;

    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
    const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

    fn cg(pts: Vec<[f32; 2]>, colour: [f32; 4]) -> ColourGuide {
        ColourGuide::new(pts, colour)
    }

    /// Three vertical guides at x = 0, 200 and 400 — red, blue, green.
    fn three() -> Multi {
        Multi::new(&[
            cg(line(0.0, -500.0, 0.0, 500.0), RED),
            cg(line(200.0, -500.0, 200.0, 500.0), BLUE),
            cg(line(400.0, -500.0, 400.0, 500.0), GREEN),
        ])
        .expect("three real guides")
    }

    /// The defining property, the N-guide version: ON a guide you get that
    /// guide's colour EXACTLY — not nearly, which is what an unguarded
    /// 1/d² would give (a division by zero, and a NaN across the page).
    #[test]
    fn each_guide_owns_its_colour_exactly_on_the_line() {
        let m = three();
        assert_eq!(m.colour_at([0.0, 0.0], MixMode::Standard, 0), RED);
        assert_eq!(m.colour_at([200.0, 123.0], MixMode::Standard, 0), BLUE);
        assert_eq!(m.colour_at([400.0, -80.0], MixMode::Standard, 0), GREEN);
        // …and just off a guide it is still overwhelmingly that colour,
        // which is the p = 2 "meets the stroke flat" property.
        let near = m.colour_at([1.0, 0.0], MixMode::Standard, 0);
        assert!(near[0] > 0.99, "a pixel off the line is still red: {near:?}");
    }

    /// Between two guides the blend leans toward the nearer one, and every
    /// channel stays a real colour — no NaN, nothing outside 0..1.
    #[test]
    fn the_blend_leans_toward_the_nearer_guide() {
        let m = three();
        let close_to_red = m.colour_at([40.0, 0.0], MixMode::Standard, 0);
        let close_to_blue = m.colour_at([160.0, 0.0], MixMode::Standard, 0);
        assert!(
            close_to_red[0] > close_to_red[2],
            "nearer red than blue: {close_to_red:?}"
        );
        assert!(
            close_to_blue[2] > close_to_blue[0],
            "nearer blue than red: {close_to_blue:?}"
        );
        for c in [close_to_red, close_to_blue] {
            assert!(c.iter().all(|v| (0.0..=1.0).contains(v)), "{c:?}");
        }
        // Alpha rides the same blend: a transparent guide really is
        // transparent on its own line.
        let fade = Multi::new(&[
            cg(line(0.0, -50.0, 0.0, 50.0), RED),
            cg(line(100.0, -50.0, 100.0, 50.0), [1.0, 0.0, 0.0, 0.0]),
            cg(line(200.0, -50.0, 200.0, 50.0), BLUE),
        ])
        .unwrap();
        assert_eq!(fade.colour_at([100.0, 0.0], MixMode::Standard, 0)[3], 0.0);
    }

    /// **The cull rule, stated as a test.** A guide is never dropped from a
    /// tile however far away it is: its weight is small, not zero. A naive
    /// "keep the two nearest guides" cull would change this pixel, and the
    /// window must not.
    #[test]
    fn a_far_guide_still_tints_a_tile_so_the_cull_keeps_it() {
        let m = three();
        // Halfway between red and blue, with green 300 px further out.
        let p = [100.0, 0.0];
        let all = m.colour_at(p, MixMode::Standard, 0);
        let two_nearest = Multi::new(&[
            cg(line(0.0, -500.0, 0.0, 500.0), RED),
            cg(line(200.0, -500.0, 200.0, 500.0), BLUE),
        ])
        .unwrap()
        .colour_at(p, MixMode::Standard, 0);
        assert!(
            all[1] > 0.01,
            "the far guide really does tint this pixel: {all:?}"
        );
        assert!(
            (all[1] - two_nearest[1]).abs() > 0.01,
            "dropping it changes the answer, so a 2-nearest cull is wrong: \
             {all:?} vs {two_nearest:?}"
        );

        // The window keeps EVERY guide — the count, and the same answer.
        let hd = 32.0 * std::f32::consts::SQRT_2;
        let w = m.window(p, hd);
        assert_eq!(w.segment_counts().len(), 3, "no guide may be culled away");
        assert_eq!(w.colour_at(p, MixMode::Standard, 0), all);
    }

    /// The per-tile cull is exact inside its box — the same claim the
    /// two-line window makes, now per guide — and still throws away most of
    /// a long shaky guide.
    #[test]
    fn the_multi_window_is_exact_inside_its_box_and_still_culls_segments() {
        let zig = |x: f32, phase: f32| -> Vec<[f32; 2]> {
            (0..=200)
                .map(|i| {
                    let y = i as f32 * 30.0;
                    [x + (i as f32 * 0.7 + phase).sin() * 12.0, y]
                })
                .collect()
        };
        let m = Multi::new(&[
            cg(zig(400.0, 0.0), RED),
            cg(zig(1200.0, 2.0), BLUE),
            cg(zig(2000.0, 4.0), GREEN),
        ])
        .unwrap();
        let hd = 32.0 * std::f32::consts::SQRT_2;
        for (cx, cy) in [(800.0f32, 3000.0f32), (420.0, 120.0), (2600.0, 5800.0)] {
            let w = m.window([cx, cy], hd);
            assert_eq!(w.segment_counts().len(), 3);
            for dx in [-31.5f32, -10.0, 0.0, 17.0, 31.5] {
                for dy in [-31.5f32, -3.0, 0.0, 22.0, 31.5] {
                    let p = [cx + dx, cy + dy];
                    assert_eq!(
                        w.colour_at(p, MixMode::Perceptual, 0),
                        m.colour_at(p, MixMode::Perceptual, 0),
                        "the cull changed the answer at {p:?} (box {cx},{cy})"
                    );
                }
            }
        }
        let kept = m.window([800.0, 3000.0], hd).segment_counts();
        assert!(
            kept.iter().all(|&n| n >= 1 && n < 40),
            "every guide keeps a few of its 200 segments: {kept:?}"
        );
    }

    /// Degenerate N-guide input: a guide with nothing usable refuses the
    /// whole field (a gradient missing a guide is not a gradient with one
    /// fewer), a dot is legal, and coincident guides do not NaN.
    #[test]
    fn degenerate_multi_guides_are_handled_not_crashed() {
        assert!(Multi::new(&[]).is_none(), "no guides, no field");
        assert!(
            Multi::new(&[
                cg(line(0.0, 0.0, 10.0, 0.0), RED),
                cg(vec![[f32::NAN, 0.0]], BLUE),
            ])
            .is_none(),
            "one unusable guide refuses the field"
        );
        // A dot guide is radial and legal, mixed with real lines.
        let m = Multi::new(&[
            cg(vec![[50.0, 50.0]], RED),
            cg(line(200.0, 0.0, 200.0, 100.0), BLUE),
            cg(line(0.0, 200.0, 100.0, 200.0), GREEN),
        ])
        .unwrap();
        assert_eq!(m.len(), 3);
        assert!(!m.is_empty());
        assert_eq!(m.colour_at([50.0, 50.0], MixMode::Standard, 0), RED);
        assert!(
            m.colour_at([500.0, 500.0], MixMode::Standard, 0)
                .iter()
                .all(|v| v.is_finite())
        );
        // Two guides in the same place with different colours: a pixel ON
        // them takes the first rather than dividing by zero twice.
        let same = line(0.0, 0.0, 10.0, 0.0);
        let dup = Multi::new(&[
            cg(same.clone(), RED),
            cg(same.clone(), BLUE),
            cg(line(0.0, 100.0, 10.0, 100.0), GREEN),
        ])
        .unwrap();
        assert_eq!(dup.colour_at([5.0, 0.0], MixMode::Standard, 0), RED);
        assert!(
            dup.colour_at([5.0, 50.0], MixMode::Standard, 0)
                .iter()
                .all(|v| v.is_finite())
        );
    }
}
