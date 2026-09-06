//! The effect-line SUB TOOLS: what a drag generates with, in the units a
//! manga tutorial states them in, plus the drag→spec geometry.
//!
//! This lived in `mn-app`'s `FigureLineOpts` until the parity round
//! (plan `2026-09-06-effect-lines-parity`). It moved down here for two
//! reasons: the render harness has to draw the shipped presets without
//! an app, and the tuning loop that follows edits numbers in ONE file
//! rather than chasing them through the UI layer.
//!
//! Everything is stated in MILLIMETRES and DEGREES, never pixels: those
//! are the units that mean the same thing on a 600 dpi B4 and a 72 dpi
//! draft. A count and a pixel width are not, which is half of why the
//! generated sets never looked like a printed page.

use serde::{Deserialize, Serialize};

use super::GenLinesSpec;

/// Which generator a preset arms. Mirrors the app's `FigureMode`
/// generator arms; the app keeps its own enum for the tool palette and
/// converts, because a Figure tool is also five things that are not
/// effect lines.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum LineKind {
    /// 流線 — drag along the motion.
    Stream,
    /// 集中線 — drag from the convergence point outward.
    Focus,
    /// ウニフラッシュ — filled teeth.
    Urchin,
    /// ベタフラッシュ — the same teeth cut out of a solid ring.
    Solid,
}

impl LineKind {
    /// Centre-out drags (everything but Stream).
    pub fn radial(self) -> bool {
        !matches!(self, LineKind::Stream)
    }

    /// The [`GenLinesSpec`] `kind` discriminant this places.
    pub fn gen_kind(self) -> u8 {
        match self {
            LineKind::Urchin => 1,
            LineKind::Solid => 2,
            _ => 0,
        }
    }
}

/// Tool-side parameters for one effect-line sub tool — what the NEXT
/// drag generates with. The drag itself supplies the geometry (centre
/// and radius, or angle and length); [`LineOpts::place`] is where the
/// two meet.
///
/// One struct serves every kind, so a row can carry a whole set of
/// values instead of the two or three its own mode reads — a
/// half-written preset is how the sets drifted apart in the first place.
/// `seed` bumps after every placement so consecutive drags differ
/// without losing determinism.
///
/// `#[serde(default)]` at the container: Lane A3 saves user sub tools as
/// JSON in `ui.txt`, and a row written by an older build must load
/// rather than take the whole line down with it.
#[derive(Clone, Copy, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LineOpts {
    pub count: u32,
    /// Line width in canvas px (built from mm at the page's dpi).
    pub width: f32,
    /// The legacy single jitter — only read as a fallback when the split
    /// wobbles below are all 0 (see `GenLinesSpec::jit`).
    pub jitter: f32,
    /// Radial only: the empty middle, as a fraction of the drag length.
    pub r_in_frac: f32,
    /// How far a line thins toward its far end, 0..1.
    pub taper: f32,
    /// Radial: the angular gap in degrees. >0 drives the ray count.
    pub gap_deg: f32,
    /// Stream: the spacing between runs in canvas px.
    pub gap_px: f32,
    /// まとまり: bundle this many lines, then leave a hole.
    pub group: u32,
    /// The hole between bundles, in multiples of the gap.
    pub group_gap: f32,
    /// Positional wobble, as a fraction of the gap.
    pub jit_gap: f32,
    /// Length wobble, as a fraction pulled off the drawn length.
    pub jit_len: f32,
    /// Width wobble, as a fraction pulled off the width.
    pub jit_width: f32,
    /// Radial: the OUTER end's own length wobble (see
    /// [`super::FocusLinesParams::jit_len_out`]). Keep it small — a
    /// stroke that stops inside the frame ends in a round cap.
    pub jit_len_out: f32,
    /// 0..1 — how much a bundle's size and the hole after it wobble (the
    /// renderer's `walk_step`). 0 = every bundle the same size, which the
    /// eye reads as a comb.
    pub group_jit: f32,
    /// Stream: per-run direction wobble in DEGREES (see
    /// [`super::SpeedLinesParams::jit_angle`]). Under a degree.
    pub jit_angle: f32,
    /// 0..1 — the fraction of lines drawn heavy (see
    /// [`super::Mix::accent_frac`]).
    pub accent_frac: f32,
    /// The TOP of an accent's width multiplier — each accent draws its
    /// own from `1.5 .. accent_mul` (see [`super::Mix::accent_mul`]), so
    /// the set is a continuum rather than two weights.
    pub accent_mul: f32,
    /// Radial: 0..1 — how far below the hole radius an inner end may
    /// fall, so the white core is a ragged band and not a compass circle
    /// (see [`super::FocusLinesParams::core_jit`]).
    pub core_jit: f32,
    /// 入り: the fraction of the length that ramps up from a point at the
    /// base, giving a spindle when combined with `taper`.
    pub entry: f32,
    /// The exponent on the taper ramp. >1 = a long thin needle.
    pub needle: f32,
    /// 0..1 — bias every length draw toward the long end.
    pub len_skew: f32,
    /// Radial: limit the burst to this arc, centred on the drag
    /// direction. 0 = the full circle.
    pub sweep_deg: f32,
    /// Stream: 0 = scatter along the direction, 1 = hang off the line
    /// through the drag's start.
    pub start_mode: u8,
    /// Stream, `start_mode` 1: start wobble as a fraction of the length.
    pub jit_start: f32,
    /// Stream, `start_mode` 1: push the reference line this many canvas
    /// px BACK along the drag, so the runs begin outside the panel and
    /// the frame cuts them. ref-09's drips are all cut hard by the top
    /// border; a run that begins exactly on the line still shows its own
    /// base cap, and a run that begins a hair inside shows a floating
    /// end (critic, round 3). 0 = start on the line, the pre-round-3
    /// placement.
    pub start_back: f32,
    /// Stream: 0 = parallel runs. >0 = aim every run at a point this
    /// many drag-lengths beyond the drag's end, along the drag — the
    /// perspective streaks of ref-07's and ref-08's second panels, where
    /// the block converges on the impact instead of sliding past it.
    pub converge_far: f32,
    pub seed: u64,
}

impl LineOpts {
    /// The shared base: `width_mm` wide lines `gap_mm` apart at `dpi`.
    ///
    /// `dpi` is the caller's `tone_dpi()` — the page's, or the manga
    /// standard 600 for a pixel canvas (at 96 a 0.2 mm line rounds to
    /// under one pixel and the whole set turns to hairline noise).
    ///
    /// Tapering is ON by default and that is deliberate: a flat-width
    /// effect line is the "flat noise field" the pro-page audit flagged,
    /// and printed 流線/集中線 thin to needles. Tool defaults are free to
    /// be right — nothing saved regenerates through them, the
    /// 0-means-legacy rule guards SPECS, not sub tools.
    ///
    /// `taper` is 0.9, not 1.0 (gauntlet round 1). A full taper drives
    /// the ramp to zero, so with any needle exponent most of a stroke's
    /// length sits under two pixels and the whole set measures as a wall
    /// of hairlines — the critic's "p50 = 1 px at 600 dpi", which is
    /// 0.04 mm, a quarter of what a G-pen can hold. 0.9 keeps weight
    /// along the stroke and still ends in a point.
    fn from_mm(dpi: u32, width_mm: f32, gap_mm: f32) -> Self {
        let px = |mm: f32| mm / 25.4 * dpi as f32;
        Self {
            count: 60,
            width: px(width_mm).max(0.5),
            gap_px: px(gap_mm),
            taper: 0.9,
            accent_mul: 1.0,
            seed: 1,
            ..Self::default()
        }
    }

    /// 流線, the everyday one: 1 mm between runs, 0.20 mm wide, bundles
    /// of 4 with a two-and-a-half-gap hole. The spindle (`entry` 0.35
    /// against the 0.9 taper) is what ref-07's streak block is made of —
    /// thin, thick, thin, no round cap anywhere on the page.
    pub fn stream(dpi: u32) -> Self {
        Self {
            group: 5,
            group_gap: 2.5,
            group_jit: 0.7,
            jit_gap: 0.25,
            jit_len: 0.5,
            jit_width: 0.4,
            // ref-07's streak block is not "hairlines plus one stray": a
            // few strokes in every handful are RAILS, ~10× the hairline,
            // and they are what gives the block its speed. 0.15 × a
            // 1.5..8 spread is a continuum topping out at ~1.6 mm.
            accent_frac: 0.15,
            accent_mul: 8.0,
            entry: 0.35,
            needle: 0.9,
            len_skew: 0.4,
            // TWO degrees of hand (round 3). One measured as 0.9° of
            // spread across a 25 mm crop, which the critic could see was
            // deliberate but read as "a call, not a fault"; 2° is the
            // number they named for "unmistakably hand-ruled". The
            // per-run wobble is ±jit_angle/2, so this is ±1°.
            jit_angle: 2.0,
            ..Self::from_mm(dpi, 0.20, 1.0)
        }
    }

    /// A tighter block: 0.6 mm gap, 0.17 mm lines, bundles of 6 with a
    /// two-and-a-half-gap hole (a 2× hole vanished under the 0.25 gap
    /// wobble — the critic saw "no bundling anywhere").
    pub fn dense_stream(dpi: u32) -> Self {
        Self {
            group: 6,
            group_gap: 2.5,
            jit_len: 0.4,
            entry: 0.3,
            accent_frac: 0.12,
            // NOT the parent's 2°. At a 0.6 mm gap a ±1° lean drifts
            // 3.5 mm over a panel-crossing run — six lanes — so the
            // block would cross itself into a mesh. This row is the
            // tight one; its hand shows in the bundling, not the lean.
            jit_angle: 1.0,
            ..Self::stream(dpi)
        }
        .with_mm(dpi, 0.17, 0.6)
    }

    /// The same rule read the other way: gaps you can see between the
    /// runs. HANDFULS rather than the stream's tight blocks — ref-07 and
    /// ref-09 both put two or three strokes nearly touching and then a
    /// wide hole, and an even sprinkle is the thing that reads as
    /// generated.
    ///
    /// It was bundles of exactly TWO until round 2, and that turned out
    /// worse than no bundling: a repeated pair at a fixed pitch is a
    /// picket fence, and the critic could see the period. `group_jit`
    /// 0.8 against a group of 4 draws 1..4 with a hole that wobbles by a
    /// fifth of itself, so the run of the eye never finds a unit.
    pub fn sparse_stream(dpi: u32) -> Self {
        Self {
            group: 4,
            group_gap: 6.0,
            group_jit: 0.8,
            jit_gap: 0.3,
            jit_width: 0.3,
            // 0.22, not the streams' 0.15: this row only puts ~21 runs on
            // a panel, and a 0.15 chance on 21 draws is TWO rails — the
            // critic counted exactly one and scored the weight mix 2/5.
            // A fraction has to be read against the count it applies to.
            accent_frac: 0.22,
            entry: 0.4,
            needle: 0.9,
            jit_len: 0.6,
            len_skew: 0.3,
            ..Self::stream(dpi)
        }
        // 1.1 mm inside a bundle, ~6.6 mm between bundles — a MEAN pitch
        // of ~3 mm once the size wobble is priced in, which is what
        // "sparse" was asking for. Bundling on top of an already 2.5 mm
        // gap would have halved the count instead.
        .with_mm(dpi, 0.30, 1.1)
    }

    /// The perspective block: the same streaks, aimed at a point beyond
    /// the drag so they fan into the impact (ref-08's second panel).
    /// ref-08 puts SOLID RAILS beside the hairlines, so this row carries
    /// the widest accent spread of the stream group.
    pub fn perspective_stream(dpi: u32) -> Self {
        Self {
            entry: 0.3,
            accent_frac: 0.15,
            // Round 3: the vanishing-side quarter was the emptiest patch
            // on the whole sheet (4.9 % ink against a 9.9 % panel mean —
            // it passed with no margin at all). Converged runs all point
            // AT that corner, so the only thing that fills it is runs
            // long enough to arrive: 0.45/0.7 instead of 0.6/0.5 puts the
            // mean length at ~0.85 of the panel fit instead of ~0.7.
            jit_len: 0.45,
            len_skew: 0.7,
            converge_far: 2.5,
            // No hand wobble on this one: the convergence already fans
            // every run by its own amount, and a second wobble on top of
            // it only softens the vanishing point.
            jit_angle: 0.0,
            ..Self::stream(dpi)
        }
    }

    /// ref-09's ゴ… drips: sparse thin verticals hanging off one edge,
    /// each a different length, in near-touching pairs. The `start_mode`
    /// is the whole preset — scattered, half of them would float in
    /// mid-air.
    pub fn drip_lines(dpi: u32) -> Self {
        Self {
            // Ones, twos and threes nearly touching, then a wide hole —
            // ref-09's spacing, which an even comb cannot state, and
            // which bundles of exactly two stated as a picket fence
            // (critic, round 2). `group_jit` 0.9 on a group of 3 is the
            // widest wobble in the set: these are the most exposed lines
            // on the page, nothing else in the panel to hide the period.
            group: 3,
            group_gap: 6.0,
            group_jit: 0.9,
            jit_gap: 0.4,
            jit_width: 0.3,
            // ref-09 has several clearly bolder verticals among the
            // hairlines; round 1 shipped zero, and the critic scored the
            // weight mix 2/5 for it. A fraction has to be read against
            // the count it applies to, and the count DOUBLED this round
            // (see the gap below), so 0.28 came back to 0.20 for the
            // same ~17 rails on a much denser curtain.
            accent_frac: 0.20,
            // …and 5×, not the stream family's 8×. At a 0.35 mm gap an
            // 8× accent on a 0.16 mm nib is 1.3 mm wide and swallows the
            // three lanes either side of it; 5× is 0.8 mm, which is what
            // ref-09's bold verticals measure.
            accent_mul: 5.0,
            // No 入り on a drip: the whole point is that the frame cuts
            // it, and a ramp at the base is a point aimed at the border.
            entry: 0.0,
            needle: 1.0,
            // Half a degree — a drip is closer to ruled than a streak is,
            // but "closer to" is not "exactly".
            jit_angle: 0.5,
            // Depths from a stub to the full panel: `place` gives an
            // anchored run the distance from the reference line to the
            // far edge, so `jit_len` 0.6 spans 2/5 of it to all of it
            // and the long bias keeps most of them deep. Round 3 pulled
            // it in from 0.8/0.3: the bottom-right sixth measured 1.1 %
            // ink against a 3.8 % panel mean — the emptiest corner on the
            // sheet — and short drips are why.
            jit_len: 0.6,
            len_skew: 0.4,
            // A SHALLOWER taper than the family's 0.9, and the exit
            // run-out in `width_at` is what pays for it. A drip's whole
            // job is to reach the bottom of the frame, and at 0.9 it
            // arrives carrying a tenth of its own weight — the bottom
            // half of the panel measured a quarter of the top half's ink.
            // 0.7 keeps the line readable the whole way down, and the tip
            // is still a point, because the point no longer depends on
            // the taper reaching 1.
            taper: 0.7,
            start_mode: 1,
            jit_start: 0.1,
            // 3 mm of overshoot. The runs begin outside the frame and
            // the border cuts them, which is the one thing you see
            // instantly next to ref-09: theirs hang from the frame line,
            // ours hung from nothing (critic, round 3 — 3 of ~40 drips
            // reached row 0). Backward `jit_start` alone would put most
            // starts off the panel; the overshoot covers the rest.
            start_back: 3.0 / 25.4 * dpi as f32,
            ..Self::stream(dpi)
        }
        // 0.35 mm inside a bundle, ~2.1 mm between bundles — a mean pitch
        // of ~1.2 mm, so ~0.8 lines per mm. Round 3 halved it: ref-09
        // runs at ~0.9 lines per mm and 24 % ink, ours at 0.40 and 3.8 %,
        // and "sparse" does not mean you can count them.
        .with_mm(dpi, 0.16, 0.35)
    }

    /// 集中線: a 2.2° gap, 0.35 mm rays needling to the convergence, a
    /// 35 % hole for the art, and bundles of 4 — the rays come in
    /// clumps on every reference sheet, never at one even pitch.
    ///
    /// `entry` 0.25 is NOT a second decorative point. `segment` caps a
    /// stroke with a half-disc, and the outer end of a ray that stopped
    /// short of the frame sat INSIDE the panel, so a heavy accent ended
    /// in a semicircular blob — a felt-tip dot, not a G-pen exit, and the
    /// one defect that failed the two best presets (critic, round 2).
    /// Two answers together: `jit_len_out` 0.15 keeps almost every outer
    /// end past the frame, where the border hides it, and `entry` thins
    /// whatever still lands inside so it exits as a needle. On a ray that
    /// does run off the page the ramp is spent off-page and invisible.
    /// The gap is 2.2°, not the 3° a CSP tutorial quotes, because the
    /// HOLE is what a tutorial does not price in: a bundle of four with a
    /// 3× hole spends 18° per bundle, so at 3° the ray count would drop
    /// from 108 to 80 and the burst would thin out exactly where the
    /// critic asked for more. 2.2° buys the hole back (109 rays) and the
    /// bundle is what the eye reads, not the pitch.
    ///
    /// The old 1.5× hole was invisible under a 0.35 position wobble — the
    /// wobble was half the hole. The hole went to 3× and the wobble down
    /// to 0.25.
    pub fn focus(dpi: u32) -> Self {
        Self {
            gap_deg: 2.2,
            gap_px: 0.0,
            r_in_frac: 0.35,
            group: 4,
            group_gap: 3.0,
            group_jit: 0.5,
            jit_gap: 0.25,
            jit_len: 0.9,
            jit_len_out: 0.15,
            jit_width: 0.4,
            accent_frac: 0.20,
            accent_mul: 4.0,
            entry: 0.25,
            needle: 0.8,
            len_skew: 0.6,
            core_jit: 0.3,
            ..Self::from_mm(dpi, 0.35, 0.0)
        }
    }

    /// The dense end of CSP's 3°/10° rule, priced the same way: a 1.6°
    /// gap in bundles of five, on 0.30 mm rays.
    pub fn dense_focus(dpi: u32) -> Self {
        Self {
            gap_deg: 1.6,
            group: 5,
            group_gap: 2.5,
            accent_frac: 0.18,
            accent_mul: 4.0,
            ..Self::focus(dpi)
        }
        .with_mm(dpi, 0.30, 0.0)
    }

    /// A black burst: rays at a 1° gap, nearly twice the weight, a
    /// quarter of them heavy, small hole (ref-08's top panel). The widest
    /// accent spread in the set — this is the row that has to put a 2 mm
    /// wedge next to a hairline.
    pub fn dark_burst(dpi: u32) -> Self {
        Self {
            gap_deg: 1.0,
            r_in_frac: 0.2,
            group: 3,
            group_gap: 3.0,
            jit_gap: 0.3,
            accent_frac: 0.25,
            accent_mul: 5.0,
            needle: 0.8,
            len_skew: 0.7,
            core_jit: 0.35,
            ..Self::focus(dpi)
        }
        .with_mm(dpi, 0.50, 0.0)
    }

    /// The two flash kinds ride the same centre-out gesture, but their
    /// `width` is a spike BASE in px and their teeth are counted, not
    /// gapped — so they keep the count-driven preset, and the parity
    /// knobs stay off: the teeth carry their own shape.
    ///
    /// The jitters are the flashes' ONLY irregularity, and at 0.25 they
    /// were not enough: the critic read both rows as "a polar zoom
    /// filter" — same length, same spacing, one circle. Angle 0.35 (the
    /// renderer's cap is 0.5), length 0.5, and `core_jit` 0.3 so the
    /// filled variant's teeth do not all start on one circle.
    pub fn flash(dpi: u32, count: u32, width_mm: f32, r_in_frac: f32) -> Self {
        Self {
            count,
            gap_deg: 0.0,
            jitter: 0.25,
            jit_gap: 0.35,
            jit_len: 0.5,
            core_jit: 0.3,
            r_in_frac,
            taper: 0.0,
            ..Self::from_mm(dpi, width_mm, 0.0)
        }
    }

    /// Restate the width and the gap in mm on top of an inherited
    /// preset. The `..Self::stream(dpi)` shorthand carries the PARENT's
    /// px width across, and a child that meant 0.15 mm would silently
    /// keep 0.20 — the exact way a copied preset goes wrong.
    fn with_mm(mut self, dpi: u32, width_mm: f32, gap_mm: f32) -> Self {
        let px = |mm: f32| mm / 25.4 * dpi as f32;
        self.width = px(width_mm).max(0.5);
        if gap_mm > 0.0 {
            self.gap_px = px(gap_mm);
        }
        self
    }

    /// Do two presets describe the same set? The seed rerolls on every
    /// placement, so it can never take part in "is this row armed".
    pub fn same_as(&self, other: &Self) -> bool {
        Self { seed: 0, ..*self } == Self { seed: 0, ..*other }
    }

    /// Turn one drag into a placeable [`GenLinesSpec`].
    ///
    /// `a` is where the press landed, `b` the release; `bounds` is the
    /// panel the press was in, or the page. This is the whole of the
    /// app's old `finish_figure_lines` maths, moved down so the harness
    /// and the tool draw the same thing from the same code.
    ///
    /// CSP default lengths (owner, 2026-08-24): the lines CROSS the
    /// panel — from the ring past the border, protrusions hidden by the
    /// frame folder's coverage. The gesture keeps centre, hole and
    /// angle; the drag distance does not cap the length.
    pub fn place(
        &self,
        kind: LineKind,
        a: [f32; 2],
        b: [f32; 2],
        bounds: [f32; 4],
        seed: u64,
    ) -> GenLinesSpec {
        let radial = kind.radial();
        let gen_kind = kind.gen_kind();
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        let dir_deg = (b[1] - a[1]).atan2(b[0] - a[0]).to_degrees();
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
            .map(|c| (c[0] - a[0]).hypot(c[1] - a[1]))
            .fold(0.0f32, f32::max);
            let r_out = (far + (far * 0.05).max(32.0)).max(len);
            (a[0], a[1], len * self.r_in_frac.clamp(0.0, 0.95), r_out)
        } else {
            // Angle from the drag direction; the runs cross the whole
            // panel edge to edge (the AABB diagonal outruns any crossing
            // at any angle), protruding past both sides until the panel
            // clips them.
            let cross = ((bounds[2] - bounds[0]).hypot(bounds[3] - bounds[1]) * 1.05).max(len);
            // An ANCHORED set (ref-09's drips) is different: the runs all
            // start on the drag's line, so their longest useful length is
            // the distance from that line to the far edge — the panel
            // HEIGHT for drips off the top edge, not the diagonal. Given
            // the diagonal instead, `jit_len` had to eat 45 % before a run
            // even stopped inside the panel, so the depths bunched and
            // the bottom quarter stayed blank (critic, round 1).
            // …and the runs start `start_back` px BEFORE that line, so
            // the reach has to cover the overshoot too or the deepest
            // drip stops `start_back` short of the far edge.
            let reach = if self.start_mode == 1 && len > 1e-3 {
                let d = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
                let base = a[0] * d[0] + a[1] * d[1] - self.start_back.max(0.0);
                [
                    [bounds[0], bounds[1]],
                    [bounds[2], bounds[1]],
                    [bounds[0], bounds[3]],
                    [bounds[2], bounds[3]],
                ]
                .iter()
                .map(|c| c[0] * d[0] + c[1] * d[1] - base)
                .fold(0.0f32, f32::max)
                .max(len)
            } else {
                cross
            };
            (dir_deg, reach, reach, 0.0)
        };
        GenLinesSpec {
            // Kinds 1/2 keep focus = true: the Object tool's driver
            // handles and their clamps key on it (GenLinesSpec's doc).
            focus: radial,
            kind: gen_kind,
            a: pa,
            b: pb,
            c: pc,
            d: pd,
            count: self.count,
            width: self.width,
            jitter: self.jitter,
            // Focus rays taper toward the convergence like Stream tails
            // (the renderer swaps endpoints for that); the flash kinds'
            // teeth carry their own shape and ignore it.
            taper: match kind {
                LineKind::Focus | LineKind::Stream => self.taper,
                _ => 0.0,
            },
            // Density: the radial kinds are gap-driven in DEGREES, the
            // stream in px — a flash counts its teeth and takes neither
            // (its `width` is a spike base, and gapping it would fight
            // the renderer's own neighbour clamp).
            gap_deg: if radial && gen_kind == 0 {
                self.gap_deg
            } else {
                0.0
            },
            gap_px: if radial { 0.0 } else { self.gap_px },
            // Bundling: the stream walks in px and the focus rays walk
            // in DEGREES (the owner's missing grouping setting, added
            // this round). The flashes take neither, same reason as the
            // gap.
            group: if gen_kind == 0 { self.group } else { 0 },
            group_gap: if gen_kind == 0 { self.group_gap } else { 0.0 },
            group_jit: if gen_kind == 0 { self.group_jit } else { 0.0 },
            jit_gap: self.jit_gap,
            jit_len: self.jit_len,
            jit_width: self.jit_width,
            // The outer end is a RADIAL idea (a stream's far end is its
            // taper, not a rim); the direction wobble is a STREAM idea (a
            // ray's angle already wobbles through `jit_gap`).
            jit_len_out: if radial { self.jit_len_out } else { 0.0 },
            jit_angle: if radial { 0.0 } else { self.jit_angle },
            accent_frac: self.accent_frac,
            accent_mul: self.accent_mul,
            entry: self.entry,
            needle: self.needle,
            len_skew: self.len_skew,
            sweep_deg: if radial { self.sweep_deg } else { 0.0 },
            // The ragged core is a radial idea: a stream has no hole to
            // stagger the ends around.
            core_jit: if radial { self.core_jit } else { 0.0 },
            start_mode: if radial { 0 } else { self.start_mode },
            jit_start: if radial { 0.0 } else { self.jit_start },
            // WHERE THE DRIVER HANDLES GO — and, since the parity round,
            // where a sweep points and where an anchored run starts.
            // Without them a burst placed near the right edge put its
            // radius handles off the page and a stream's reference line
            // sat at the canvas centre instead of on the run you just
            // drew: nothing to aim at, which is half of "I cannot
            // re-select them" (owner, 2026-08-23).
            hand_deg: if radial { dir_deg } else { 0.0 },
            // `start_mode` 1 hangs the runs off the line through the
            // drag's START (that is the gesture: you draw the edge the
            // drips fall from). Everything else anchors at the midpoint,
            // where the reference line has always been.
            // …pushed `start_back` px back along the drag, so the runs
            // begin off the panel and the border clips them (see the
            // field).
            anchor: (!radial).then(|| {
                if self.start_mode == 1 {
                    let back = self.start_back.max(0.0);
                    if back > 0.0 && len > 1e-3 {
                        [
                            a[0] - (b[0] - a[0]) / len * back,
                            a[1] - (b[1] - a[1]) / len * back,
                        ]
                    } else {
                        a
                    }
                } else {
                    [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5]
                }
            }),
            converge: (!radial && self.converge_far > 0.0 && len > 1e-3).then(|| {
                let reach = self.converge_far * len;
                [
                    b[0] + (b[0] - a[0]) / len * reach,
                    b[1] + (b[1] - a[1]) / len * reach,
                ]
            }),
            color: [0, 0, 0],
            seed,
        }
    }
}

/// One shipped sub tool row: a name, the generator it arms, and the
/// knobs it writes. `opts` is a fn of dpi rather than a value because
/// every number in a preset is a millimetre — the row cannot be a
/// constant without picking a page resolution for the owner.
pub struct LinePreset {
    pub name: &'static str,
    pub kind: LineKind,
    pub opts: fn(u32) -> LineOpts,
}

/// The shipped rows, in palette order: the 流線 group, then the 集中線
/// group with the two flashes at its end (same centre-out gesture).
///
/// The sub tool palette draws this verbatim, so the order here IS the
/// order on screen.
pub fn builtin_presets() -> &'static [LinePreset] {
    &[
        LinePreset {
            name: "Stream line",
            kind: LineKind::Stream,
            opts: LineOpts::stream,
        },
        LinePreset {
            name: "Dense stream",
            kind: LineKind::Stream,
            opts: LineOpts::dense_stream,
        },
        LinePreset {
            name: "Sparse stream",
            kind: LineKind::Stream,
            opts: LineOpts::sparse_stream,
        },
        LinePreset {
            name: "Perspective stream",
            kind: LineKind::Stream,
            opts: LineOpts::perspective_stream,
        },
        LinePreset {
            name: "Drip lines",
            kind: LineKind::Stream,
            opts: LineOpts::drip_lines,
        },
        LinePreset {
            name: "Saturated line",
            kind: LineKind::Focus,
            opts: LineOpts::focus,
        },
        LinePreset {
            name: "Dense saturated line",
            kind: LineKind::Focus,
            opts: LineOpts::dense_focus,
        },
        LinePreset {
            name: "Dark burst",
            kind: LineKind::Focus,
            opts: LineOpts::dark_burst,
        },
        LinePreset {
            name: "Sea urchin flash",
            kind: LineKind::Urchin,
            opts: |dpi| LineOpts::flash(dpi, 64, 0.85, 0.3),
        },
        LinePreset {
            name: "Solid flash",
            kind: LineKind::Solid,
            opts: |dpi| LineOpts::flash(dpi, 64, 0.95, 0.45),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `finish_figure_lines` EXACTLY as `crates/app/src/app/canvas_input.rs`
    /// had it on 2026-09-06, transcribed before a line of it moved.
    ///
    /// This is the ORACLE, not a second implementation: Lane A2 deletes
    /// the app's copy and calls [`LineOpts::place`] instead, and the only
    /// honest way to promise "same drag, same spec" is to keep the code
    /// that was there and compare against it. It reads only the fields
    /// the app's `FigureLineOpts` had — the parity knobs did not exist,
    /// so their pass-through is 0 on both sides.
    fn app_finish_figure_lines(
        opts: &LineOpts,
        kind: LineKind,
        a: (f32, f32),
        b: (f32, f32),
        bounds: [f32; 4],
    ) -> GenLinesSpec {
        let radial = kind.radial();
        let len = (b.0 - a.0).hypot(b.1 - a.1);
        let (pa, pb, pc, pd) = if radial {
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
            let angle = (b.1 - a.1).atan2(b.0 - a.0).to_degrees();
            let cross = ((bounds[2] - bounds[0]).hypot(bounds[3] - bounds[1]) * 1.05).max(len);
            (angle, cross, cross, 0.0)
        };
        let gen_kind = kind.gen_kind();
        GenLinesSpec {
            focus: radial,
            kind: gen_kind,
            a: pa,
            b: pb,
            c: pc,
            d: pd,
            count: opts.count,
            width: opts.width,
            jitter: opts.jitter,
            taper: match kind {
                LineKind::Focus | LineKind::Stream => opts.taper,
                _ => 0.0,
            },
            gap_deg: if radial && gen_kind == 0 {
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
            hand_deg: if radial {
                (b.1 - a.1).atan2(b.0 - a.0).to_degrees()
            } else {
                0.0
            },
            anchor: (!radial).then_some([(a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5]),
            converge: None,
            color: [0, 0, 0],
            seed: opts.seed,
            ..Default::default()
        }
    }

    /// The app's `FigureLineOpts::from_mm`, same date, same reason.
    fn legacy_from_mm(dpi: u32, width_mm: f32, gap_mm: f32) -> LineOpts {
        let px = |mm: f32| mm / 25.4 * dpi as f32;
        LineOpts {
            count: 60,
            width: px(width_mm).max(0.5),
            taper: 0.5,
            gap_px: px(gap_mm),
            seed: 1,
            ..Default::default()
        }
    }

    /// The app's shipped presets on 2026-09-06 — the values whose
    /// placement this test claims did not move.
    fn legacy_presets() -> Vec<(LineKind, LineOpts)> {
        vec![
            (
                LineKind::Stream,
                LineOpts {
                    group: 4,
                    group_gap: 2.5,
                    jit_gap: 0.25,
                    jit_len: 0.3,
                    jit_width: 0.25,
                    ..legacy_from_mm(600, 0.20, 1.0)
                },
            ),
            (
                LineKind::Stream,
                LineOpts {
                    jit_gap: 0.3,
                    jit_len: 0.35,
                    jit_width: 0.25,
                    ..legacy_from_mm(600, 0.30, 2.5)
                },
            ),
            (
                LineKind::Focus,
                LineOpts {
                    gap_deg: 3.5,
                    gap_px: 0.0,
                    taper: 0.6,
                    r_in_frac: 0.40,
                    jitter: 0.25,
                    jit_gap: 0.25,
                    jit_len: 0.25,
                    jit_width: 0.3,
                    ..legacy_from_mm(600, 0.35, 0.0)
                },
            ),
            (
                LineKind::Urchin,
                LineOpts {
                    count: 64,
                    jitter: 0.25,
                    r_in_frac: 0.3,
                    taper: 0.0,
                    ..legacy_from_mm(600, 0.85, 0.0)
                },
            ),
            (
                LineKind::Solid,
                LineOpts {
                    count: 64,
                    jitter: 0.25,
                    r_in_frac: 0.45,
                    taper: 0.0,
                    ..legacy_from_mm(600, 0.95, 0.0)
                },
            ),
        ]
    }

    /// A2 swaps the app's body for `place(..)`. This is the promise it
    /// rests on: for every shipped preset the app had, and for a spread
    /// of drags and panels, the spec that comes out is the same one.
    #[test]
    fn place_matches_the_app_drag_maths() {
        let drags = [
            ([100.0f32, 100.0f32], [400.0f32, 260.0f32]),
            ([900.0, 700.0], [400.0, 260.0]),
            ([50.0, 900.0], [51.0, 908.0]),
            ([600.0, 300.0], [600.0, 40.0]),
        ];
        let panels = [[0.0f32, 0.0, 1200.0, 1700.0], [180.0, 240.0, 1010.0, 900.0]];
        for (kind, opts) in legacy_presets() {
            for (a, b) in drags {
                for bounds in panels {
                    let want =
                        app_finish_figure_lines(&opts, kind, (a[0], a[1]), (b[0], b[1]), bounds);
                    let got = opts.place(kind, a, b, bounds, opts.seed);
                    assert_eq!(got, want, "{kind:?} at {a:?}->{b:?} in {bounds:?}");
                    // And the raster, not only the numbers: a field that
                    // READ differently would still land here.
                    assert_eq!(
                        super::super::tests::fingerprint(&got.render((600, 850))),
                        super::super::tests::fingerprint(&want.render((600, 850))),
                        "{kind:?}: same spec, same pixels"
                    );
                }
            }
        }
    }

    /// Every shipped row places and inks something, at both the 600 dpi
    /// print page and the 96 dpi screen canvas the mm→px conversion has
    /// to survive. A preset that renders an empty layer is a row the
    /// owner clicks and nothing happens.
    #[test]
    fn builtin_presets_all_render_without_panic() {
        let size = (1200u32, 900u32);
        let bounds = [0.0f32, 0.0, size.0 as f32, size.1 as f32];
        for p in builtin_presets() {
            for dpi in [96u32, 600] {
                let o = (p.opts)(dpi);
                let (a, b) = if p.kind.radial() {
                    ([660.0f32, 405.0f32], [900.0f32, 405.0f32])
                } else {
                    ([180.0, 495.0], [1020.0, 405.0])
                };
                let spec = o.place(p.kind, a, b, bounds, 7);
                let m = spec.render(size);
                assert!(!m.is_empty(), "{} at {dpi} dpi inked nothing", p.name);
                assert!(
                    spec.width >= 0.5 && spec.width.is_finite(),
                    "{} at {dpi} dpi has a silly width ({})",
                    p.name,
                    spec.width
                );
            }
        }
    }

    /// The density a preset states in millimetres and degrees has to
    /// SURVIVE the drag. Round 1 of the gauntlet opened with "dense-stream
    /// draws 8 strokes per 25 mm where its 0.6 mm gap asks for 36", and
    /// the first suspect was `place` quietly dropping `gap_px` / `group`
    /// on the way to the spec. It does not — the loss was the
    /// along-the-direction scatter in `render_speed`, fixed there — but a
    /// preset whose numbers never reach the renderer is a silent failure
    /// with no symptom except a set that looks thin, so it gets a guard.
    #[test]
    fn placed_presets_keep_their_density() {
        let bounds = [0.0f32, 0.0, 2362.0, 1653.0];
        let row = |name: &str| {
            builtin_presets()
                .iter()
                .find(|p| p.name == name)
                .unwrap_or_else(|| panic!("no preset called {name}"))
        };

        // 流線: the gap is a MILLIMETRE figure and must land in px at the
        // page's dpi, bundles and hole intact.
        let (k, o) = {
            let p = row("Dense stream");
            (p.kind, (p.opts)(600))
        };
        let s = o.place(k, [354.0, 909.0], [2008.0, 744.0], bounds, 1);
        let want_px = 0.6 / 25.4 * 600.0;
        assert!(
            (s.gap_px - want_px).abs() < 0.05,
            "dense stream keeps its 0.6 mm gap ({} px, wanted {want_px:.2})",
            s.gap_px
        );
        assert_eq!((s.group, s.group_gap), (6, 2.5), "and its bundle of six");

        // 集中線: the gap is in DEGREES and drives the ray count through
        // the angular walk, so the walk has to produce the bundles too.
        let (k, o) = {
            let p = row("Saturated line");
            (p.kind, (p.opts)(600))
        };
        let s = o.place(k, [1299.0, 744.0], [1819.0, 744.0], bounds, 1);
        assert_eq!(
            (s.gap_deg, s.group, s.group_gap),
            (o.gap_deg, o.group, o.group_gap),
            "the preset's angular density reaches the spec"
        );
        // The shipped tuning, spelled out: 2.2° in bundles of four with a
        // 3× hole. (It was 3° / 1.5× before round 1 — the hole had to grow
        // to be visible at all, and the gap shrank to pay for it.)
        assert_eq!((s.gap_deg, s.group, s.group_gap), (2.2, 4, 3.0));
        assert!(
            (100..=120).contains(&s.ray_count()),
            "which is ~109 rays over the circle, not 80 ({})",
            s.ray_count()
        );
    }

    /// `same_as` ignores the seed and nothing else — it is what tells a
    /// sub tool row it is the armed one.
    #[test]
    fn same_as_ignores_only_the_seed() {
        let a = LineOpts::stream(600);
        assert!(a.same_as(&LineOpts { seed: 99, ..a }));
        assert!(!a.same_as(&LineOpts { group: 7, ..a }));
        assert!(!a.same_as(&LineOpts::dense_stream(600)));
    }
}
