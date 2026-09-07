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
    /// Flash kinds: the tooth's fat end as a fraction of the angular
    /// PITCH where that end sits (see [`super::UrchinParams::width_frac`]).
    /// 0 = the `width` in px. Near 1 the fat ends of neighbouring teeth
    /// just touch, which is the whole difference between a ウニフラ (they
    /// do not quite fuse — a chewed hole) and a ベタフラ (they do — a
    /// solid core).
    pub width_frac: f32,
    /// Flash kinds: the tooth's point faces OUTWARD, fat end at the hole
    /// (see [`super::UrchinParams::tip_out`]). Both flash rows want it —
    /// it is the one construction every Japanese source describes.
    pub tip_out: bool,
    /// Solid flash: how far the BLACK runs past the teeth, as a multiple
    /// of the reach (see [`super::UrchinParams::field`]). 0 = it stops
    /// with them. ref-20/21's ベタフラ is a black field with a white burst
    /// punched through it, and the field is what makes the corners solid.
    pub field: f32,
    /// Radial: the outer radius as a multiple of the DRAG LENGTH. 0 = the
    /// panel's far corner plus a margin, which is what every other radial
    /// row wants and what every saved layer was placed with.
    ///
    /// The flashes want the other thing. Every Japanese source calls a
    /// ウニフラ/ベタフラ フキダシの一種 — "a kind of speech balloon" — and
    /// every reference in the pack is a balloon-sized object with its own
    /// silhouette sitting inside a panel, not a burst that fills one.
    /// With the corner reach the outer edge is always off the page, so
    /// the effect has no outline at all: `solid-flash.png` before Lane F
    /// was 84 % ink with a compass-circle hole, which is the shape you
    /// get when the only visible edge is the one you did not draw.
    pub reach_frac: f32,
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

    /// The two flash kinds ride the same centre-out gesture. Everything
    /// they share is here; the two rows below differ in three numbers.
    ///
    /// Rebuilt for Lane F (2026-09-07) against a reference pack of real
    /// ウニフラ/ベタフラ (`docs/plans/refs/effect-lines/REFS.md`). What
    /// the old rows got wrong, in order of how much it showed:
    ///
    /// - **They were rays, not BUNDLES.** Clip Studio's own flash tool
    ///   has a まとまり setting and labels its spikes 2本/3本/4本
    ///   (ref-24); a close-up of a professionally inked analog flash
    ///   resolves each spike into 4–10 slivers that peak together
    ///   (ref-22); a manga school budgets 4–6 lines per big peak, 2–3 per
    ///   small (ref-23). An evenly spaced ring fails at ANY count, which
    ///   is why raising `count` never helped. `group` 5 with a 5× hole
    ///   and a 0.5 size wobble draws packs of three to five.
    /// - **They filled the panel.** Every source calls these フキダシの一種
    ///   — a kind of speech balloon — and every reference is a
    ///   balloon-sized object with its own silhouette. The corner reach
    ///   put the outer edge off the page, so the effect had no outline at
    ///   all. `reach_frac` 1.4 makes the drag state the size.
    /// - **The teeth were upside down** for the filled row: see
    ///   `tip_out`.
    /// - **`width` in mm cannot state a cut.** See `width_frac`.
    ///
    /// The jitters (retuned by Builder B after critic 1): angle 0.35 (the
    /// renderer caps at 0.5) is the within-bundle wobble; `core_jit` now
    /// carries the whole of the hole's raggedness and does it in LOBES
    /// around `r_in` rather than as a per-bundle pull-in, so `r_in` still
    /// means the mean hole radius and the ring's span still means the
    /// stroke length. `jit_len` — the apex's own white-noise push — drops
    /// to a tenth: at 0.90 it scattered the fat starts over 360 px of a
    /// 364 px hole radius, so nothing stacked at the hole and the band
    /// read mid-grey instead of black (critic 1, "the band never reads
    /// black", 6–8 % ink).
    pub fn flash(dpi: u32, count: u32, width_frac: f32, r_in_frac: f32) -> Self {
        Self {
            count,
            gap_deg: 0.0,
            jitter: 0.25,
            jit_gap: 0.35,
            jit_len: 0.10,
            jit_len_out: 0.05,
            core_jit: 0.55,
            // Packs of 3–5 slivers, then a hole four pitches wide: the
            // 3–8× between/within ratio the pack calls for (ref-22 puts
            // the between-bundle gap at 4–8 sliver widths).
            group: 5,
            group_gap: 4.0,
            group_jit: 0.5,
            // A few teeth much fatter than the rest — ref-13 puts "fat
            // wedges next to hairline slivers, 4–6x in the same
            // neighbourhood".
            accent_frac: 0.16,
            accent_mul: 4.5,
            // Most teeth long, a few stubs.
            len_skew: 0.45,
            // BOTH rows draw the stroke the same way up: fat at the hole,
            // whipping out to a needle. That is the one construction
            // every source describes, and the only difference between the
            // two effects is whether those fat starts fuse (see the rows).
            tip_out: true,
            width_frac,
            r_in_frac,
            taper: 0.0,
            ..Self::from_mm(dpi, 0.3, 0.0)
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
            // …unless the row states its own reach in drag lengths (the
            // flashes do — see `reach_frac`). Still floored at the drag,
            // so a long drag is never shortened by a small multiple.
            let r_out = if self.reach_frac > 0.0 {
                (len * self.reach_frac).max(len)
            } else {
                (far + (far * 0.05).max(32.0)).max(len)
            };
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
            // Bundling: the stream walks in px, the focus rays walk in
            // DEGREES, and (Lane F) so do the flashes — a flash IS a set
            // of bundles, which is the one structural fact three
            // independent Japanese sources agree on (see
            // `render_urchin`). The flash's pitch comes from its count,
            // so it still takes no `gap_deg`.
            group: self.group,
            group_gap: self.group_gap,
            group_jit: self.group_jit,
            // The tooth width against the pitch, and which way up the
            // tooth is — flash ideas only; a ray's width is a nib, and
            // nibs are stated in millimetres.
            width_frac: if gen_kind == 0 { 0.0 } else { self.width_frac },
            tip_out: gen_kind != 0 && self.tip_out,
            // The black field is the SOLID kind's alone: it is the thing
            // a ベタフラ punches its burst out of, and on a filled row it
            // would just be a black disc over the drawing.
            field: if gen_kind == 2 { self.field } else { 0.0 },
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
            // ウニフラ: a BALLOON-sized ring of fine hairs whose fat
            // starts nearly, but not quite, fuse round the hole. ref-12's
            // hole is about half its outer radius, so the reach is 1.4
            // drag lengths against a 0.70 hole; 0.80 of the pitch AT THE
            // HOLE leaves the starts near-touching inside a bundle and
            // clearly apart between bundles, which is what chews the hole
            // edge instead of turning it into a compass circle.
            opts: |dpi| LineOpts {
                // ref-12's hole is a little over half its outer radius,
                // so a 22 mm drag becomes a 13 mm hole inside a 24 mm
                // balloon. Both numbers are the MEAN: the hole's lobes
                // swing it ±60 %, which is ref-12's own 51–132 px spread
                // about an 85 px mean.
                reach_frac: 1.09,
                // PACKS OF 5–9, not 3–5. ref-12 is the dense member of
                // the family — "packs of roughly 8–15 near-parallel
                // needles sitting shoulder to shoulder, then a visible
                // white gap" — and the pack size is also the duty cycle:
                // at 5 teeth against a 4-pitch hole only 43 % of the ring
                // carries ink and the band cannot read black however fine
                // the hairs are. 8 against 5 is 61 %.
                group: 12,
                group_gap: 3.2,
                group_jit: 0.55,
                // The lobed hole, at ref-12's own roughness (σ 20 % of
                // the mean hole radius).
                core_jit: 0.60,
                // The nib. `needle` under 1 keeps the belly and whips the
                // last stretch to a point (the band's ink rises with it);
                // `entry` gives the FAT end a short ramp so it arrives at
                // full width from a point instead of a square cut.
                needle: 0.5,
                entry: 0.26,
                // Fat ends WIDER than the pitch, so inside a pack they
                // overlap and fuse: ref-18's ウニフラ has a 70° arc "fused
                // into a solid black patch where the fat stroke-starts
                // ran together", and REFS calls that partial fusion
                // correct — the state between ウニフラ and ベタフラ. The
                // gaps BETWEEN packs are what keeps it from being a
                // ベタフラ.
                ..LineOpts::flash(dpi, 508, 1.10, 0.59)
            },
        },
        LinePreset {
            name: "Solid flash",
            kind: LineKind::Solid,
            // ベタフラ: the same stroke drawn as the CUT, so the white
            // slivers are fat at the hole and needle outward and the
            // black between them is the mark. ref-20/21 exactly: "an
            // all-black rectangle with a white oval burst punched through
            // the middle … everything you read as the flash is the WHITE
            // shape".
            //
            // The two radii are different questions, and Lane F Builder A
            // answered them with one number. `reach_frac` says how long a
            // white sliver is (a balloon: 1.55 drag lengths against a
            // 0.85 hole, so the burst stops well inside the frame);
            // `field` says how far the black goes (4× that, i.e. past any
            // panel the balloon fits in). Sharing one radius meant the
            // slivers were jittered as a fraction of a corner-sized span,
            // so all of them ran off the frame and the panel had no black
            // field at all — corner ink 0 %, critic 1's biggest fail.
            //
            // `width_frac` 0.55, not 0.90: a cut that is nine tenths of
            // the pitch merges with its neighbours, so 170 slivers
            // printed as ~40 huge white wedges ("a cracked-window star").
            // At 0.55 the black thread between two slivers is as wide as
            // the sliver, which is ref-22's own measurement.
            opts: |dpi| LineOpts {
                jit_len_out: 0.85,
                field: 4.0,
                // The cut keeps its belly and whips out at the end, same
                // as the urchin.s tooth. A straight wedge is a hair for
                // most of its length, and a hair is where the black
                // between two of them pinches under a pixel and dots.
                needle: 0.6,
                reach_frac: 1.70,
                // Packs of 4–6 with a 3.5-pitch valley: ref-22's own
                // count ("each spike resolves into a stack of 4–10
                // parallel white slivers … the gap between bundles is
                // 4–8 sliver widths").
                group: 6,
                group_gap: 3.5,
                group_jit: 0.55,
                // A LOT less accent spread than the urchin's, and a
                // tighter angle wobble. A cut that is several pitches
                // wide crosses the black threads either side of it and
                // strands the black beyond their tips — 246 free-floating
                // black islands at `accent_mul` 4.5 (critic 1 found two
                // of them by eye at 1:1 and called them "detached
                // triangular islands that never join the black mass").
                accent_frac: 0.14,
                accent_mul: 1.9,
                jit_gap: 0.22,
                // Most slivers short, a few long — the other way up from
                // the urchin. REFS target 4 wants the longest white spike
                // at 1.5–2.0× the MEDIAN white radius, and a long bias
                // pulls the median up to meet the longest.
                len_skew: 0.15,
                // The core is a letterable middle, so its edge wobbles
                // but does not lobe as deep as a ウニフラ's chewed hole.
                core_jit: 0.42,
                ..LineOpts::flash(dpi, 380, 0.34, 0.85)
            },
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

    /// THE FLASH TARGET TABLE (Lane F, 2026-09-07).
    ///
    /// Every number here is measured off the rendered panel by
    /// [`super::super::metrics`] — the same code the `effect_metrics`
    /// example prints — so a preset that drifts fails CI instead of
    /// failing a critic three rounds later. That is the whole point of
    /// the lean loop: the last gauntlet paid a builder round for a defect
    /// the builder had eyeballed wrong.
    ///
    /// The targets come from two places, and where they disagree the
    /// stricter one wins:
    ///
    /// - the brief (`docs/plans/2026-09-07-lane-F-brief.md`): accents
    ///   ≥ 8 % of strokes, base-width p95/p50 ≥ 2.5, round caps 0, and a
    ///   hole/inner-edge σ of at least 0.4 mm;
    /// - the reference pack (`docs/plans/refs/effect-lines/REFS.md`,
    ///   "Targets I'd set"), which measured five real ウニフラ/ベタフラ:
    ///   the hole σ ≥ 15 % of its own mean, the fattest stroke ≥ 4× the
    ///   thinnest, and for the solid row a white region of σ 20–31 % with
    ///   its longest reach 1.5–2.0× the median.
    ///
    /// It measures the SHIPPED rows on the same drag the render harness
    /// uses (`crates/core/examples/common/mod.rs`), because a target that
    /// is not the picture the critic looks at is a target for nothing.
    /// The panel is deliberately the harness's 100 × 70 mm at 600 dpi.
    #[test]
    fn flash_presets_hit_their_measured_targets() {
        let dpi = 600;
        let px = |mm: f32| mm / 25.4 * dpi as f32;
        let size = (px(100.0) as u32, px(70.0) as u32);
        let (w, h) = (size.0 as f32, size.1 as f32);
        let bounds = [0.0, 0.0, w, h];
        let row = |name: &str| {
            builtin_presets()
                .iter()
                .find(|p| p.name == name)
                .unwrap_or_else(|| panic!("no preset called {name}"))
        };

        for (name, drag, long) in [
            ("Sea urchin flash", 0.22f32, false),
            ("Solid flash", 0.176, true),
        ] {
            let p = row(name);
            let c = if long {
                [w * 0.50, h * 0.48]
            } else {
                [w * 0.55, h * 0.45]
            };
            let spec = (p.opts)(dpi).place(
                p.kind,
                c,
                [c[0] + w * drag, c[1]],
                bounds,
                if long { 1_063 } else { 1_056 },
            );
            let m = super::super::metrics::measure(&spec, size, dpi);
            let ratio = m.w_p95 / m.w_p50.max(1e-6);
            let hole_rel = m.hole_rel().unwrap_or(0.0);

            // --- BUILDER B's four, from critic 1's ranked list.

            // 1. BUNDLES THAT FORM PEAKS. Critic 1: "almost every stroke
            //    is a loner … bundle structure is absent everywhere".
            //    Most of the strokes on the cut have to sit in a pack of
            //    three or more (REFS target 1: bundles of 2–6 with the
            //    between-bundle gap 3–8× the within-bundle one).
            //    THREE QUARTERS, not half. An even comb still measures a
            //    few clusters — its strokes carry accents, so on a cut
            //    circle some neighbours merge and some gaps close by
            //    chance. Turning `group` off on this very preset and
            //    re-measuring puts 55 % of its strokes in packs of 3+
            //    (`1×74 2×60 3×33 4×14 …`, a histogram whose mass is at
            //    the SMALL end); the walk puts 96–99 % there, with the
            //    mass at the group size. 75 % sits in the empty middle.
            let total: usize = m.bundles.iter().map(|(s, n)| s * n).sum();
            let packed: usize = m.bundles.iter().filter(|(s, _)| *s >= 3).map(|(s, n)| s * n).sum();
            assert!(
                total > 0 && packed * 4 >= total * 3,
                "{name}: only {packed} of {total} strokes are in a pack of 3+ \
                 — that is a comb ({:?})",
                m.bundles
            );

            // 2. NOTHING FLOATS. Dirt specks anywhere, plus (solid only)
            //    black islands clear of the field. Critic 1 found both by
            //    eye at 1:1 and the old probe reported neither.
            assert_eq!(m.frags, 0, "{name}: free-floating marks");

            // 3. EVERY STROKE RUNS OUT TO A POINT. See `caps` — the probe
            //    is Builder B's: strokes down to 5 px, and the excuse for
            //    a straight end is that it is BURIED in ink, not that it
            //    happens to sit near the middle of the burst.
            //
            //    The solid row is allowed a dozen. Its black spikes end
            //    on the white core, and the core's boundary is pushed a
            //    little BELOW the cuts' fat ends on purpose (`core_lut`):
            //    parked exactly on them the black pinches under a pixel
            //    and a 1-bit rasteriser prints that as a line of dirt —
            //    355 specks, measured. Below them the black is wide and
            //    clean, and the price is that a few spikes meet the core
            //    with a flat end instead of a point. Ten ends out of
            //    ~500 is the trade, and it is the honest one for a
            //    pipeline with no anti-aliasing anywhere in it.
            let cap_budget = if p.kind == LineKind::Urchin { 0 } else { 12 };
            assert!(
                m.caps <= cap_budget,
                "{name}: {} ends stop dead instead of running out to a point \
                 (budget {cap_budget})",
                m.caps
            );

            // The brief's four, both rows.
            assert!(
                m.accent_pct >= 8.0,
                "{name}: accents are {:.0} % of the strokes, wanted 8 %",
                m.accent_pct
            );
            assert!(
                ratio >= 2.5,
                "{name}: base width p95/p50 is {ratio:.2}, wanted 2.5 \
                 (1.9 with nothing above twice the median is what the round-4 \
                 critic called machine-made)"
            );
            assert!(
                m.hole_sigma_mm.unwrap_or(0.0) >= 0.4,
                "{name}: the hole edge is a circle ({:?} mm σ)",
                m.hole_sigma_mm
            );

            // REFS, where it is stricter.
            assert!(
                m.w_p95 >= 4.0 * m.w_p5,
                "{name}: fattest stroke {:.2} mm against thinnest {:.2} mm, \
                 wanted 4× (REFS target 2)",
                m.w_p95,
                m.w_p5
            );
            if p.kind == LineKind::Urchin {
                // REFS target 3, the pack's sharpest discriminator: a hole
                // smoother than ~12 % of its own radius is a 密フラッシュ,
                // not a ウニフラ.
                assert!(
                    hole_rel >= 15.0,
                    "{name}: the hole is only {hole_rel:.1} % ragged, wanted 15 % \
                     (ref-12 measures 20 %; under 12 % is a 密フラッシュ)"
                );
                // 4. …and the SECOND half of REFS target 3, which Builder
                //    A could not get and critic 1 called "the single
                //    sharpest fail in the pack": the hole must be rougher
                //    than the OUTLINE. It was 21.7 % hole against 27.2 %
                //    outline, which is ref-16 密フラッシュ the wrong way
                //    round. ref-12 measures 20 % against 11 %.
                let outer_rel = m.outer_rel().unwrap_or(99.9);
                assert!(
                    hole_rel > outer_rel,
                    "{name}: hole {hole_rel:.1} % against outline {outer_rel:.1} % \
                     — a 密フラッシュ has it that way round, a ウニフラ does not"
                );
            } else {
                // 2b. THE BLACK FIELD. ref-20 is "an all-black rectangle
                //     with a white oval burst punched through"; critic 1
                //     measured our corners at 0 % and called it "a
                //     black-and-white pinwheel".
                assert!(
                    m.corner_ink >= 99.5,
                    "{name}: the emptiest corner box is only {:.0} % ink — \
                     a ベタフラ's corners are solid black",
                    m.corner_ink
                );
                // REFS target 4, measured on the WHITE region, which is
                // what this row renders (a black field around a burst).
                assert!(
                    (18.0..=33.0).contains(&hole_rel),
                    "{name}: white region σ is {hole_rel:.1} % of its mean, \
                     wanted 20–31 % (ref-21 20 %, ref-20 31 %)"
                );
                let hi = m.hole_hi_ratio.unwrap_or(9.9);
                assert!(
                    (1.4..=2.1).contains(&hi),
                    "{name}: the longest white spike is {hi:.2}× the median, \
                     wanted 1.5–2.0 (above that they all run off the frame)"
                );
            }
        }
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
