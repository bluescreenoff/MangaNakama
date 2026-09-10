//! Speed/focus line GENERATION (TRIAGE 140 v1, SF-family): parametric,
//! seeded, deterministic black ink for manga effect lines.
//!
//! v1 is dialog-driven — parameters in, one new layer of hard-edged ink
//! out. CSP's two-driver-curve on-canvas editing (SF-004/005: the blue
//! reference line and the red shape line, editable alone) needs
//! Object-tool curve editing on generator layers and is deferred with
//! reason; the params here are exactly what those two curves will drive.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::tile::{FIX15_ONE, TILE_SIZE, Tile, TileIdx};

pub mod metrics;
pub mod presets;
pub use presets::{LineKind, LineOpts, LinePreset, builtin_presets};

/// 集中線 — focus lines: `count` rays converging toward `center`, drawn
/// from `r_in` to `r_out` (jittered per line), each a segment from a
/// jittered angle. Width jitters by `width_jitter` (0..1 of `width`).
///
/// `Default` exists so the parity round could add fields without
/// rewriting every literal; zeroing every knob is exactly the legacy
/// meaning, the same contract [`SpeedLinesParams`] already carried.
#[derive(Clone, Debug, Default)]
pub struct FocusLinesParams {
    pub center: [f32; 2],
    pub r_in: f32,
    pub r_out: f32,
    pub count: u32,
    pub width: f32,
    /// 0..1 — per-line angle jitter as a fraction of the angular gap.
    pub angle_jitter: f32,
    /// 0..1 — per-line width jitter fraction.
    pub width_jitter: f32,
    /// 0..1 — per-line length jitter fraction of (r_out − r_in).
    pub length_jitter: f32,
    /// 0..1 — the OUTER end's own length jitter. 0 = use
    /// `length_jitter` for both ends, which is what every file saved
    /// before this field drew (and the `rand()` is drawn either way —
    /// only the multiplier changes, so the sequence never moves).
    ///
    /// Why it had to split: the placed reach is the panel's far corner
    /// plus a margin, so an outer end that pulls in by up to
    /// `length_jitter/2` of the span lands INSIDE the frame — and
    /// `segment` caps a stroke with a half-disc, so a heavy ray ended in a
    /// semicircular blob (gauntlet critic, round 2: the single defect
    /// that failed the three best presets). A printed 集中線 runs every
    /// stroke off the frame at the rim; the INNER ends are the ragged
    /// ones. Keep this small (≤ 0.2) and let `length_jitter` stay big.
    pub jit_len_out: f32,
    /// 0..1 — rays thin toward the CENTRE (a printed 集中線 needles at the
    /// convergence and carries its weight at the rim). 0 = the legacy
    /// constant-width ray, bit-stable.
    pub taper: f32,
    /// The parity round's shared knobs — see [`Mix`]. 0 everywhere is the
    /// pre-parity ray set, bit for bit.
    pub mix: Mix,
    /// The angular gap in degrees the bundle walk steps by. Only read
    /// when the walk applies (`sweep_deg` > 0 or `group` > 1) — the plain
    /// gap-drives-the-count path lives in [`GenLinesSpec::ray_count`] and
    /// still lays rays out as `i · 2π / count`, which is what every saved
    /// file drew.
    pub gap_deg: f32,
    /// >0 — rays only inside `sweep_center_deg ± sweep_deg/2` instead of
    /// the full circle. ref-11's left panel: a burst whose centre sits
    /// off the page below fills a fan, not a ring, and clipping a full
    /// circle cannot make one (the rays that would leave the panel are
    /// the ones you want gone, but so are their opposites).
    pub sweep_deg: f32,
    /// Where that arc is centred, degrees. Fed from `hand_deg` — the
    /// direction the placing drag was made in.
    pub sweep_center_deg: f32,
    /// まとまり in ANGLE space: `group` rays a `gap_deg` apart, then a
    /// hole of `group_gap × gap_deg`. The owner's missing grouping
    /// setting, and the reason ref-08's burst reads as drawn rather than
    /// stepped. 0/1 = no bundling. Only read when `gap_deg` > 0.
    pub group: u32,
    /// The hole between bundles, in multiples of `gap_deg` (see `group`).
    pub group_gap: f32,
    /// 0..1 — how much a bundle's SIZE and its following hole wobble.
    /// See [`walk_step`]. 0 = every bundle exactly `group` rays with an
    /// exactly `group_gap` hole, which is what every file saved before
    /// this drew, and the extra `rand()`s live behind the `> 0` guard.
    pub group_jit: f32,
    /// 0..1 — pull each ray's INNER end BELOW `r_in` by up to
    /// `core_jit × r_in`, drawn per ray, so the white core is a ragged
    /// BAND rather than a circle.
    ///
    /// `length_jitter` alone cannot do it: it only ever pulls an end
    /// OUTWARD from `r_in`, so with any length skew a crowd of rays land
    /// on `r_in` exactly and the core reads as a compass circle (gauntlet
    /// critic, round 1: "the white core is a ragged blob, never a
    /// circle"). 0 = the old inner ring, and the extra `rand()` is inside
    /// that guard.
    pub core_jit: f32,
    pub seed: u64,
}

/// The knobs the parity round (2026-09-06) gave BOTH line renderers, so
/// they are declared once instead of twice.
///
/// Every field's 0 is the pre-parity behaviour and every `rand()` call
/// they add sits behind its own `> 0` guard, so a spec saved without
/// them draws the same pixels in the same order (`legacy_renders_are_bit_stable`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Mix {
    /// 0..1 — the fraction of lines drawn as ACCENTS. The single biggest
    /// gap against a printed page: ref-07's streak block and ref-08's
    /// burst both carry many hairlines and a FEW heavy strokes, and the
    /// old width jitter could only ever THIN a line, never thicken one.
    pub accent_frac: f32,
    /// The TOP of an accent's half-width multiplier (applied after the
    /// width jitter). 0 reads as 1.
    ///
    /// An accent draws its multiplier UNIFORMLY from `1.5 .. accent_mul`
    /// rather than always landing on `accent_mul` — a printed set is a
    /// continuum from hairline to heavy (gauntlet critic, round 1: "a
    /// wall of 1 px plus one fat line"), and one fixed multiplier can
    /// only ever make two weights. The extra `rand()` sits inside the
    /// `accent_frac > 0` guard, so a spec without accents draws the same
    /// sequence it always did.
    pub accent_mul: f32,
    /// 0..1 — the fraction of the length at the BASE end that ramps up
    /// from a point (入り). With `taper` on the other end this is a
    /// spindle: thin, thick, thin — ref-07's streaks, which our round cap
    /// could not make.
    pub entry: f32,
    /// The exponent on the taper ramp, `(1 − taper·t)^k`. 0 reads as
    /// k = 1, today's straight wedge. >1 thins fast and then runs a long
    /// thin needle (ref-08's wedges); <1 keeps a belly.
    pub needle: f32,
    /// 0..1 — biases every length draw toward the LONG end
    /// (`u.powf(1 + 3·len_skew)` on the shortening draw). A printed set
    /// has most lines long and a few stubs; a uniform draw has neither.
    pub len_skew: f32,
}

impl Mix {
    /// Pull a 0..1 sample toward 0 — "toward the full length". Every
    /// length draw here is a SHORTENING (how far an end pulls in), so
    /// biasing long is biasing this sample small.
    fn skew(&self, u: f32) -> f32 {
        if self.len_skew > 0.0 {
            u.powf(1.0 + 3.0 * self.len_skew.clamp(0.0, 1.0))
        } else {
            u
        }
    }

    /// This line's half-width after the accent roll. Guarded: with
    /// `accent_frac` 0 no random number is drawn, so the sequence is the
    /// one every saved file was rendered with.
    ///
    /// STRATIFIED, not a coin. `acc` is a deficit accumulator walked in
    /// run order — which is normal-offset order in both renderers, i.e.
    /// straight across the panel — and it fires when the accumulated
    /// share reaches a whole line. An independent coin per run has the
    /// right EXPECTED fraction and the wrong distribution at the counts
    /// a panel actually holds: `stream-line` puts ~7 accents on a page,
    /// and seven coin flips land in one half often enough that they did
    /// — thickest stroke 7 px in the top half against 26 px in the
    /// bottom (gauntlet critic, round 3). The accumulator spends the
    /// same fraction with the same one `rand()` per line, but the gap
    /// between accents is `1/accent_frac` lines give or take a third,
    /// never a run of thirty with none.
    ///
    /// `prev` carries "the line before this one was an accent", and a
    /// second accent immediately after one is REFUSED. Two neighbours in
    /// a bundle are a gap apart — 1° on `dark-burst`, which at the crop
    /// radius is ~9 px — and an accent is up to `accent_mul × width`
    /// wide, so two adjacent heavies MERGE into one slab and the white
    /// sliver left between them breaks into dashes (gauntlet critic,
    /// round 2: "reads as a print fault", `dark-burst-crop` y≈225). The
    /// refusal is on the DRAW, not the accumulation: the deficit stays
    /// owed and the next line takes it, so the fraction is not lost.
    fn accent(&self, hw: f32, seed: &mut u64, prev: &mut bool, acc: &mut f32) -> f32 {
        if self.accent_frac <= 0.0 {
            *prev = false;
            return hw;
        }
        // `3u²` has mean 1, so the expected fraction is exactly
        // `accent_frac` — but it is a LUMPY 1. A bare `+= frac` puts the
        // accents on an exact comb, and so does any tight jitter round
        // it: `0.5 + rand` was tried and its four-term sum concentrates,
        // which printed `dark-burst` as one heavy wedge every fourth ray
        // and `stream-line` as eight evenly ruled rails. ref-08 does the
        // opposite — two or three heavies almost touching, then a dozen
        // hairlines. The squared draw is mostly small with a long tail,
        // so a big step fires and leaves the remainder standing, which
        // fires again two lines later (the no-adjacent rule spaces the
        // pair) and gives the CLUSTER, while a run of small steps gives
        // the hole. What it still cannot do is spend a whole half-panel
        // without firing, which is the round-3 defect.
        let u = rand(seed);
        *acc += self.accent_frac * 3.0 * u * u;
        let take = *acc >= 1.0 && !*prev;
        *prev = take;
        if take {
            *acc -= 1.0;
            // The SPREAD, not the ceiling — see `accent_mul`. Both rand
            // calls are inside the guard, so the accent-free sequence is
            // untouched.
            let top = self.accent_mul.max(1.0);
            let lo = 1.5f32.min(top);
            hw * (lo + rand(seed) * (top - lo))
        } else {
            hw
        }
    }

    /// The stroke profile this mix asks [`segment`] for.
    fn profile(&self, taper: f32) -> Profile {
        Profile {
            taper: taper.clamp(0.0, 1.0),
            entry: self.entry.clamp(0.0, 1.0),
            needle: self.needle.max(0.0),
        }
    }
}

/// 流線 — speed lines: `count` parallel segments along `angle` degrees,
/// lengths in [len_min, len_max], scattered across the canvas perpendic.
///
/// `Default` exists for the `..Default::default()` shorthand and zeroes
/// every knob, which for the density fields is exactly the legacy meaning
/// (uniform-random scatter, no bundling, no split jitters).
#[derive(Clone, Debug, Default)]
pub struct SpeedLinesParams {
    pub angle_deg: f32,
    pub count: u32,
    pub len_min: f32,
    pub len_max: f32,
    pub width: f32,
    /// 0..1 — how far each run thins toward its TAIL (the end it travels
    /// to). 0 is the pre-2026-08-22 look, bit for bit; 1 ends in a needle
    /// point, which is what a printed 流線 block actually does (the
    /// pro-page audit's "flat noise field" complaint).
    pub taper: f32,
    /// Aim every run at this canvas point instead of running pure
    /// parallel — a far point gives the subtle fan a perspective panel
    /// wants; `None` is parallel. A NEAR point turns the block into
    /// focus lines, which is the other tool's job.
    pub converge: Option<[f32; 2]>,
    /// >0 — WALK the normal extent in steps of `gap_px` instead of
    /// scattering `count` runs at uniform-random offsets. The scatter is
    /// what makes a generated 流線 block read as noise: uniform-random
    /// positions CLUMP (three runs a pixel apart, then a bald strip),
    /// which is precisely what a hand-ruled block never does. 0 keeps the
    /// scatter, bit for bit, for every file saved before this existed.
    pub gap_px: f32,
    /// まとまり — bundle `group` runs at `gap_px`, then leave a hole of
    /// `group_gap` × `gap_px` before the next bundle. 0/1 = no bundling.
    /// Only read on the walk (`gap_px` > 0).
    pub group: u32,
    /// The hole between bundles, in multiples of `gap_px` (see `group`).
    pub group_gap: f32,
    /// 0..1 — how much a bundle's SIZE and its following hole wobble.
    /// See [`walk_step`]. 0 = every bundle exactly `group` runs, the
    /// constant that made `sparse-stream` and `drip-lines` read as "a
    /// picket fence of pairs" (gauntlet critic, round 2). Walk only, and
    /// its `rand()`s are behind its own `> 0` guard.
    pub group_jit: f32,
    /// 0..1 — positional wobble as a fraction of `gap_px`, so the walk is
    /// even without being mechanical. Walk only; 0 = dead even.
    pub jit_gap: f32,
    /// 0..1 — per-run length wobble, a fraction pulled off the drawn
    /// length. Walk only; 0 = the `len_min`..`len_max` spread alone.
    pub jit_len: f32,
    /// 0..1 — per-run width wobble, a fraction pulled off `width`. Walk
    /// only; 0 = every run at the nominal width.
    pub jit_width: f32,
    /// DEGREES — per-run direction wobble, `± jit_angle/2`. 0 = dead
    /// parallel, which is what every saved file drew and also what the
    /// critic measured on the page: "parallel to within ~3 px over
    /// 23 mm", i.e. ruled, not drawn. A hand-inked 流線 block leans by
    /// under a degree per stroke and that is the whole difference.
    /// The extra `rand()` is behind the `> 0` guard.
    pub jit_angle: f32,
    /// The parity round's shared knobs — see [`Mix`].
    pub mix: Mix,
    /// 0 = scatter each run along the direction (today, bit-stable).
    /// 1 = every run STARTS on the reference line — the line through
    /// `anchor` perpendicular to the direction — and runs `len` from
    /// there. ref-09's ゴ… drip lines: verticals that all hang off the
    /// panel's top edge at different lengths, which a scatter cannot do
    /// (it puts half of them in mid-air).
    pub start_mode: u8,
    /// 0..1 — `start_mode` 1 only: how far BEFORE that line a run may
    /// start, as a fraction of its own length. Backward, so the wobble
    /// is spent off the panel and shows up as ragged far ends rather
    /// than as a row of floating starts — see the call site.
    pub jit_start: f32,
    /// Where the reference line sits (`start_mode` 1 only). `None` = the
    /// canvas origin's projection, i.e. the line through (0, 0).
    pub anchor: Option<[f32; 2]>,
    pub seed: u64,
}

/// The walk's hard ceiling on runs. `gap_px` comes from a UI field and a
/// sub-pixel gap over a 600 dpi B4's ~10 000 px normal extent is an
/// unbounded rasterization, not a slow one (the same class of hang the
/// [`segment`] bbox clip fixed).
const MAX_RUNS: u32 = 20_000;

/// ウニフラッシュ — sea-urchin flash: `count` FILLED triangular spikes
/// around `center`, needle-pointed at `r_in` and `width` px wide at
/// `r_out`. That is the classic flash mat: the shape a segment-based
/// generator cannot make, and the pro-page audit's #1 IMPOSSIBLE.
///
/// `solid` flips the POLARITY to the ベタフラッシュ variant — the ring
/// area between `r_in` and `r_out` inks solid and the same teeth are cut
/// OUT of it, so the ink pools at the hole and breaks into outward
/// spikes at the rim. The hole stays empty either way: it is where the
/// art goes.
///
/// `Default` exists for the `..Default::default()` shorthand, and — the
/// rule the whole module runs on — zeroing every knob below is EXACTLY
/// the pre-Lane-F flash, `rand()` sequence included: every new field
/// guards its own draws.
#[derive(Clone, Debug, Default)]
pub struct UrchinParams {
    pub center: [f32; 2],
    pub r_in: f32,
    pub r_out: f32,
    pub count: u32,
    /// Spike base width in px at `r_out`, CLAMPED to 90% of the gap
    /// between neighbours: a wider value merges the teeth into a plain
    /// ring (and, inverted, erases the solid variant entirely), so the
    /// tool would silently stop drawing the shape it exists for.
    pub width: f32,
    /// The base width as a fraction of the angular PITCH at `r_out`
    /// instead of in px. 0 = use `width`, which is what every saved flash
    /// has.
    ///
    /// A millimetre cannot state what a ベタフラッシュ needs. Its cut has
    /// to be a WEDGE — most of the gap at the rim, a point at the hole —
    /// because the black that survives between two cuts IS the spike;
    /// with a slit instead you get a black panel with hairline scratches
    /// in it, which is exactly what `solid-flash.png` was before Lane F
    /// (84 % ink, and the last critic scored it 1/5 on every axis). The
    /// pitch is a function of the count and the reach, and the preset
    /// knows neither: only the renderer does.
    pub width_frac: f32,
    /// 0..1 — per-spike angle jitter as a fraction of the angular pitch.
    /// Clamped to half a pitch by the renderer — not for the renderer's
    /// sake any more (the solid scan finds its candidates by binary
    /// search over the teeth's own angles, so a tooth may sit anywhere),
    /// but because past half a pitch the teeth cross each other and the
    /// set stops reading as a burst.
    pub angle_jitter: f32,
    /// 0..1 — per-spike length jitter. Which END it moves depends on
    /// `jit_len_out`: alone, it pulls the TIP in from `r_out`, which is
    /// what every saved flash drew.
    pub length_jitter: f32,
    /// 0..1 — the OUTER end's own length jitter, and the switch that
    /// splits the two ends the way [`FocusLinesParams::jit_len_out`]
    /// splits a 集中線's.
    ///
    /// 0 = `length_jitter` moves the outer end and the apex does not
    /// move at all: the pre-Lane-F flash, bit for bit. Above 0 the outer
    /// end takes THIS jitter and `length_jitter` moves the APEX instead.
    ///
    /// Keep it small (≤ 0.1). An outer end that stops inside the frame
    /// shows the tooth's straight base cut sitting in open paper, and the
    /// round-cap probe counts it — that is where `sea-urchin-flash`'s
    /// three caps came from (gauntlet round 3), and it is the same defect
    /// the focus kind fixed by keeping its rim ends off the page. The
    /// variation a printed ウニフラ actually shows is at the HOLE: ref-12
    /// measures σ 20 % of the mean hole radius against 11 % at the rim.
    pub jit_len_out: f32,
    /// 0..1 — per-spike APEX jitter, the flash's half of
    /// [`FocusLinesParams::core_jit`]: each tooth's point drops below
    /// `r_in` by up to `core_jit × r_in`, so the teeth do not all start
    /// on one circle. 0 = the old shared apex.
    ///
    /// Since Lane F it moves BOTH variants. The solid one's ring scan
    /// used to start at `r_in` whatever the teeth did, so its hole was a
    /// compass circle and no knob could reach it (gauntlet critic, round
    /// 4: "solid-flash's core is a perfect circle", 1/5). The scan now
    /// starts at the per-angle apex, interpolated between the two
    /// neighbouring teeth — so with `core_jit` 0 every apex is `r_in`,
    /// the interpolation returns exactly `r_in`, and the old raster is
    /// unmoved.
    pub core_jit: f32,
    /// The parity round's shared knobs — see [`Mix`]. The flash reads
    /// `accent_frac`/`accent_mul` (a few teeth much wider at the base)
    /// and `len_skew` (most teeth long, a few short); `entry` and
    /// `needle` are stroke-profile knobs and a tooth is a filled
    /// triangle, so it has no ramp to bend.
    pub mix: Mix,
    /// まとまり in ANGLE space, the same walk the focus kind bundles
    /// with: `group` teeth a pitch apart, then a hole of `group_gap ×
    /// pitch`. 0/1 = the even circle every saved flash drew.
    ///
    /// ref-12's ウニフラ is packs of 8–15 near-parallel needles with a
    /// visible white gap between packs — "the bundle envelope, not the
    /// individual needle, is what gives the silhouette its stepped look".
    /// An even comb cannot say that at any count.
    pub group: u32,
    /// The hole between bundles, in multiples of the pitch (see `group`).
    pub group_gap: f32,
    /// 0..1 — how much a bundle's size and the hole after it wobble (see
    /// [`walk_step`]). 0 = every bundle exactly `group` teeth.
    pub group_jit: f32,
    /// >0 — teeth only inside `sweep_center_deg ± sweep_deg/2`, and (the
    /// solid variant) the ring only there too. A flash aimed from outside
    /// the panel otherwise spends its teeth all the way round a circle
    /// whose far side nothing can see — and worse for the solid kind,
    /// inks the whole panel on the way past. 0 = the full circle.
    pub sweep_deg: f32,
    /// Where that arc is centred, degrees. Fed from `hand_deg` — the
    /// direction the placing drag was made in.
    pub sweep_center_deg: f32,
    /// Which way up a tooth is. `false` = the point is at the hole and
    /// the wide end at the rim, which is what every saved flash drew.
    /// `true` = the other way: fat at the hole, whipping out to a needle.
    ///
    /// Every Japanese source describes the construction the same way — a
    /// ベタフラ is a 集中線 drawn with a **fat start and a thin whip-out**,
    /// so the fat starts fuse into solid black and the thin ends stay as
    /// spikes; a ウニフラ is the same stroke whose fat starts do NOT quite
    /// fuse, leaving a chewed hole (`REFS.md`, family table). ref-23's
    /// author labels the boundary 太い at the base and 細い at the tip.
    ///
    /// Which variant wants which is the whole trick, and they are
    /// opposites, because one draws the ink and the other draws the hole:
    ///
    /// - FILLED (ウニフラ) — the tooth IS the ink, so `true`: fat at the
    ///   hole, needle at the rim. `false` printed the old
    ///   "polar zoom filter" — 64 wedges pointing inward, ending in a
    ///   flat cut at the rim, which is also where its round caps came
    ///   from.
    /// - SOLID (ベタフラ) — the tooth is the CUT, and `true` again, which
    ///   is NOT the mirror image you would guess. ref-22's close-up of an
    ///   inked ベタフラ shows the WHITE slivers "widest right at the hole,
    ///   narrowing monotonically to a hair", so the cut is the fat-start
    ///   stroke too; what that leaves is black that is needle-thin at the
    ///   hole and fattens outward until the cuts run out and it fuses
    ///   into the field. ref-23 annotates that same boundary 細い on the
    ///   hole side and 太い on the field side.
    ///
    /// So both variants are drawn with one stroke shape and differ only
    /// in whether you ink it or cut it out.
    pub tip_out: bool,
    /// SOLID only: how far the black runs PAST the teeth, as a multiple
    /// of `r_out`. 0 = the black stops at `r_out` with the teeth, which
    /// is what every saved solid flash drew.
    ///
    /// ref-20 is the reason. A ベタフラ in its 全面ベタ form "is an
    /// all-black rectangle with a white oval burst punched through the
    /// middle. There is no black spike ring visible at all — the black
    /// just runs to the panel edges … everything you read as the flash is
    /// the WHITE shape". ref-21 is the same and keeps "an unbroken black
    /// margin of roughly 10–15 % of the panel width all round plus fully
    /// solid corners" — which is exactly what critic 1 failed the row for
    /// not having (corner ink 0 %, "white stripes reach all four panel
    /// edges").
    ///
    /// It cannot be done by making `r_out` the panel corner instead,
    /// which is what Lane F Builder A tried: the teeth are jittered as a
    /// fraction of `r_out − r_in`, so a corner-sized reach makes them
    /// corner-sized too and every white sliver runs off the frame. The
    /// two radii are different questions — how long is a sliver, and how
    /// far does the ink go — and they need separate answers.
    ///
    /// It is a MULTIPLE rather than "to the edge of the layer" so a flash
    /// dropped on a full page blacks a big area, not the page. 4× a
    /// balloon-sized reach covers any panel the balloon fits in.
    pub field: f32,
    pub solid: bool,
    pub seed: u64,
}

/// One flash tooth as a filled isoceles triangle: apex on the ray at
/// `r_apex`, `hw` half-width at `r_base`. The sides are STRAIGHT — the
/// test is the triangle's, not an angular wedge's, because a constant-
/// angle wedge bows outward and reads as a petal instead of a spike.
struct Tooth {
    /// The tooth's axis, normalised to `[0, τ)`. The solid variant finds
    /// a pixel's candidate teeth by BINARY SEARCH over a table of these,
    /// so a bundled tooth may sit anywhere — the old scan indexed by
    /// sector and could only ever look one sector either way, which is
    /// what capped `angle_jitter` at half a gap and would have broken
    /// outright under angular bundling.
    ang: f32,
    c: f32,
    s: f32,
    r_apex: f32,
    r_base: f32,
    hw: f32,
    /// The exponent on the width ramp, [`Mix::needle`]. 1 = the straight
    /// wedge every saved flash drew. Under 1 the tooth keeps its belly
    /// most of the way and then whips to a point, which is what a nib
    /// does and what keeps a ウニフラ's band BLACK: a straight wedge
    /// averages half its base width over its length, a 0.5 ramp averages
    /// two thirds, and the band's ink goes up by the same fraction
    /// (critic 1: "the band never reads black", 6–8 % ink where ref-12 is
    /// a mass).
    needle: f32,
    /// 入り, [`Mix::entry`]: the fraction of the length at the FAT end
    /// that ramps back down from full width to a point. 0 = the straight
    /// cut every saved flash drew.
    ///
    /// A tooth's fat end is the one thing in this module that could not
    /// come to a point, and critic 1 failed both rows on it — "the heavy
    /// accent bar … its left end is cut square in open white", "two black
    /// rectangles, blunt on both ends". A real nib enters and exits;
    /// 0.10 is a short entry, so the stroke still reads 太い at the base
    /// the way ref-23 labels it, but it arrives there from a point.
    ///
    /// Since Builder C it is stated PER TOOTH, not once for the set: the
    /// preset's fraction with a floor of [`ENTRY_NIB`] half-widths, so a
    /// fat accent on a short stroke still gets a ramp long enough to read
    /// as a point. See `render_urchin`.
    entry: f32,
    /// A ベタフラ's CUT only: how wide the cut becomes BELOW its fat end,
    /// in px. 0 (every other row, and every saved flash) = it stops at
    /// the fat end, which is the straight cut the whole module drew
    /// before.
    ///
    /// Why a cut has to keep going below its own base: the black between
    /// two cuts is the mark, and where the cuts stop the black abruptly
    /// widens to the full pitch and ends in a flat. That flat is critic
    /// 2's "blunt spike ends at the core" and the staircase along the
    /// core edge, and Builder B's answer to it — named in that report and
    /// left undone — is this: let each cut run on past the boundary,
    /// FLARING, so it merges with its neighbours and the black between
    /// them tapers to a real point and disappears into the white core.
    ///
    /// It only ever removes black inside the core's own boundary, which
    /// the solid scan skips anyway; what it changes is the last few
    /// pixels above it.
    flare_hw: f32,
}

impl Tooth {
    /// The end nearer the centre, whichever end that is — the solid
    /// variant's ragged core is built out of these.
    fn r_inner(&self) -> f32 {
        self.r_apex.min(self.r_base)
    }

    /// Is `(dx, dy)` — a pixel relative to the flash centre — inside?
    fn hit(&self, dx: f32, dy: f32) -> bool {
        let along = dx * self.c + dy * self.s;
        // `r_apex` is the POINT and `r_base` the wide end, and since
        // Lane F either may be the outer one — a ウニフラ's hairs are fat
        // at the hole and whip out to a needle, a ベタフラ's CUTS are the
        // other way up so that the black between them is. Ordering them
        // here rather than assuming `apex < base` keeps one `hit` for
        // both, and with `apex < base` every line below is the arithmetic
        // it replaced, float for float.
        let (lo, hi) = if self.r_apex <= self.r_base {
            (self.r_apex, self.r_base)
        } else {
            (self.r_base, self.r_apex)
        };
        if along > hi {
            return false;
        }
        if along < lo {
            // 入り below the base — the flare (see `flare_hw`). Off by
            // default, and `along < 0` is the far side of the centre.
            if self.flare_hw <= self.hw || along < 0.0 {
                return false;
            }
            let gain = self.flare_hw - self.hw;
            let x = ((lo - along) / (FLARE_SLOPE * gain).max(1.0)).min(1.0);
            let perp = -dx * self.s + dy * self.c;
            return perp.abs() <= self.hw + gain * x;
        }
        let span = hi - lo;
        if span <= f32::EPSILON {
            return false;
        }
        let perp = -dx * self.s + dy * self.c;
        // The tooth's half-width at this radius, floored at half a pixel
        // — the SAME floor `segment` puts on its ramp, and for the same
        // reason: this test is hard-edged, so once the wedge narrows
        // under ~0.35 px it catches only the odd pixel centre and the
        // apex prints as a row of dots. `segment` got the floor in round
        // 0 and `fill_tooth` did not, which is why the dashes survived in
        // `sea-urchin-flash-crop` and in the solid flash's slits.
        //
        // It only ADDS pixels, all of them within half a pixel of the
        // spike's own axis; no flash kind carries a fingerprint pin, and
        // the two shape tests measure rim-vs-hole ink, not the apex.
        // `t` runs 0 at the point to 1 at the fat end.
        let t = (along - self.r_apex).abs() / span;
        let mut ramp = if self.needle == 1.0 {
            t
        } else {
            t.powf(self.needle.clamp(0.05, 8.0))
        };
        if self.entry > 0.0 {
            // 入り: the last `entry` of the way to the fat end ramps back
            // to a point, so the base is a nib's entry and not a cut.
            let e = self.entry.clamp(0.0, 0.9);
            ramp *= ((1.0 - t) / e).min(1.0);
        }
        perp.abs() <= (self.hw * ramp).max(0.5)
    }
}

/// xorshift64* — small, deterministic, no deps.
fn rand(seed: &mut u64) -> f32 {
    // splitmix64 — full-range, no low-bit correlation (the first xorshift
    // attempt biased the top 24 bits to [0.5, 1) for small seeds).
    *seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *seed;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 40) as f32 / (1u64 << 24) as f32
}

/// Ink one pixel opaque BLACK, premultiplied fix15 — `[0, 0, 0, ONE]`.
///
/// This wrote `[ONE; 4]` from the first commit, which is opaque WHITE (the
/// same four words `Layer::fill_white` writes), so every generator in this
/// module drew white-on-white: a layer in the palette and nothing on the
/// page. It survived because every test here reads channel 3 only —
/// coverage — and the fingerprint pin counts inked PIXELS, not their
/// colour, so the whole suite agreed with a bug it never looked at. Owner
/// repro 2026-08-22, Figure ▸ Saturated line. Black is the documented
/// contract of this module (three doc comments say so) and the print
/// reality of 集中線/流線; the geometry is untouched, so the bit-stability
/// pin still matches.
fn put(map: &mut HashMap<TileIdx, Tile>, x: i32, y: i32) {
    if x < 0 || y < 0 {
        return;
    }
    let idx = TileIdx::of_pixel(x, y);
    let (ox, oy) = idx.origin();
    let tile = map.entry(idx).or_insert_with(Tile::new_transparent);
    let lx = (x - ox) as usize;
    let ly = (y - oy) as usize;
    if lx < TILE_SIZE && ly < TILE_SIZE {
        let o = Tile::offset(lx, ly);
        let d = tile.data_mut();
        d[o] = 0;
        d[o + 1] = 0;
        d[o + 2] = 0;
        d[o + 3] = FIX15_ONE as u16;
    }
}

/// The half-width ramp along one stroke, `t` = 0 at the base `a` and 1
/// at the tip `b`. Carried as a struct rather than three more positional
/// floats: both renderers thread it through and a bare
/// `segment(.., 0.35, 1.2, ..)` call site is unreadable and easy to
/// transpose.
#[derive(Clone, Copy, Debug, Default)]
struct Profile {
    taper: f32,
    entry: f32,
    needle: f32,
}

impl Profile {
    /// Just a taper — the pre-parity shape, and what the flash-round
    /// tests measure.
    #[cfg(test)]
    fn taper(taper: f32) -> Self {
        Self {
            taper,
            ..Self::default()
        }
    }

    fn width_at(&self, t: f32) -> f32 {
        width_at(t, self.taper, self.entry, self.needle)
    }
}

/// The profile, spelled out: exit ramp, needle exponent, entry ramp.
///
/// `taper 0, entry 0, needle 0` returns exactly `1.0` for every `t` —
/// not "1.0 to within a float" but the literal, because that is the
/// value every effect-line layer saved before this existed was
/// rasterized with. Both extras are skipped rather than applied at their
/// identity for the same reason: `powf(1.0)` is not contractually the
/// identity on every input.
fn width_at(t: f32, taper: f32, entry: f32, needle: f32) -> f32 {
    let mut w = (1.0 - taper * t).max(0.0);
    if needle > 0.0 {
        w = w.powf(needle);
    }
    // 抜き: run the residue out to a point. A `taper` under 1 leaves
    // `(1 − taper)^needle` of the NIB standing at the tip — 0.16 of it
    // at the shipped 0.9/0.8 — and that is a fraction of the nib, not a
    // fixed width: on a hairline it is half a pixel and the floor in
    // `segment` already reads it as a needle, but on a 30 px accent it
    // is a rounded 5 px stub. That is the one round cap left on the
    // sheet after round 2 (critic, round 3: `dark-burst-crop` (182,149),
    // half-width 4 px), and it is a property of every heavy tapered
    // stroke, not of that ray.
    //
    // The last tenth of the length runs the residue linearly to zero, so
    // the exit is a point at EVERY weight. It costs ~0.8 % of the ink
    // (half of a tenth of the tail's already-thin width) and it is
    // invisible on a hairline, where the half-pixel floor holds the line
    // to the tip either way. Guarded on `taper` > 0: a constant-width
    // stroke has no exit to run out, and that is the pinned legacy path.
    const EXIT: f32 = 0.1;
    if taper > 0.0 && t > 1.0 - EXIT {
        w *= (1.0 - t) / EXIT;
    }
    // 入り: a fast-in ramp from a point, so the base end is a needle too
    // and the stroke reads as the spindle ref-07's streaks are. 0.7 is
    // the "fast" — a linear ramp leaves a visible triangle at the base,
    // which is a wedge pointing the wrong way.
    if entry > 0.0 && t < entry {
        w *= (t / entry).powf(0.7);
    }
    w
}

/// Rasterize one thick segment (a x b, half-width hw) by scanning its
/// bbox and testing point-to-segment distance. Hard edges — speed lines
/// are print black; AA lives in the resample on export.
///
/// The [`Profile`] ramps the half-width along a→b, so `b` is the needle
/// end; an all-zero profile leaves the constant-width behaviour
/// untouched (the ramp evaluates to `hw * 1.0`, the same float, so every
/// effect-line layer saved before tapering existed regenerates bit for
/// bit).
///
/// The bbox is CLIPPED to the canvas here, not after: the dialog's own
/// maximums (count 512, outer radius 2×width) put a segment's unclipped
/// bbox at ~10^7 pixels on a 600 dpi page — unclipped, the scan was
/// quadratic in the radius and allocated unbounded off-canvas tiles that
/// `retain` only discarded after building (a multi-minute UI hang and a
/// commit spike, from three slider drags).
///
/// Clipping to the page is not enough when the stroke CROSSES the page,
/// which is what a 流線 does. The lag hunt (2026-09-10) measured one
/// default `Stream line` preset at **20 seconds** on a B4 600 dpi page in
/// release, and `Perspective stream` at over nine minutes — the owner's
/// "speedlines/radial crash manganakama from slowness". The cost is the
/// axis-aligned box: a stroke 8,000 px long and 3 px wide has a
/// 6,000 × 5,700 px box, so a scan that should test 30,000 pixels tests 34
/// million, and a preset draws hundreds of strokes. It is O(length²) in a
/// job that is O(length × width).
///
/// So the scan walks the stroke in CHUNKS along its own axis: each chunk's
/// box is the box of its two endpoints grown by the reach, and the total
/// area is proportional to length × width again. This is not an
/// approximation and not a quality knob — every pixel within reach of the
/// stroke has its nearest point on some chunk and is therefore inside that
/// chunk's box, [`put`] is idempotent so the overlap between neighbouring
/// boxes costs a repeated test and nothing else, and the boxes only ever
/// SHRINK relative to the old one, so no pixel that was not inked before is
/// inked now. The written pixel set is identical, which is what
/// `legacy_renders_are_bit_stable` pins.
fn segment(
    map: &mut HashMap<TileIdx, Tile>,
    a: [f32; 2],
    b: [f32; 2],
    hw: f32,
    prof: Profile,
    size: (u32, u32),
) {
    let d = [b[0] - a[0], b[1] - a[1]];
    let dd = d[0] * d[0] + d[1] * d[1];
    if dd <= f32::EPSILON {
        return;
    }
    // How far off its own axis this stroke can still ink a pixel: the ramp
    // never returns more than 1.0 and the test below floors the half-width
    // at half a pixel, so `hw.max(0.5)` is the true half-width for the
    // whole stroke, and the `+ 1.0` is the margin the old box already had.
    // It is never SMALLER than the old `hw + 1.0`, so the chunk boxes can
    // only lose pixels that were outside the stroke anyway.
    let reach = hw.max(0.5) + 1.0;
    for (p, q) in scan_chunks(a, b, reach) {
        let x0 = (p[0].min(q[0]) - reach).max(0.0);
        let x1 = (p[0].max(q[0]) + reach).min(size.0 as f32);
        let y0 = (p[1].min(q[1]) - reach).max(0.0);
        let y1 = (p[1].max(q[1]) + reach).min(size.1 as f32);
        if x0 >= x1 || y0 >= y1 {
            continue;
        }
        for y in y0.floor() as i32..=y1.ceil() as i32 {
            for x in x0.floor() as i32..=x1.ceil() as i32 {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let t = (((px - a[0]) * d[0] + (py - a[1]) * d[1]) / dd).clamp(0.0, 1.0);
                let qx = a[0] + t * d[0];
                let qy = a[1] + t * d[1];
                let ex = px - qx;
                let ey = py - qy;
            // The ramped half-width, floored at half a pixel: a pen
            // needle stays a 1 px line until it ends, and this test is
            // hard-edged, so once `hw · width_at(t)` drops under ~0.35 px
            // it only catches the odd pixel centre and the last third of
            // every tapered stroke prints DASHED. The floor is where the
            // ramp stops — one pixel wide, held to the tip.
            //
            // The trap: both callers already floor `hw` at 0.5, and a
            // zero profile returns exactly 1.0, so `hwt == hw` on every
            // pre-parity layer and the pinned rasters do not move
            // (`legacy_renders_are_bit_stable`). A layer saved WITH a
            // taper does redraw — solid tails instead of dotted ones —
            // which is the fix, not a regression.
                let hwt = (hw * prof.width_at(t)).max(0.5);
                if ex * ex + ey * ey <= hwt * hwt {
                    put(map, x, y);
                }
            }
        }
    }
}

/// A hard ceiling on how many boxes one stroke is scanned in — a runaway
/// parameter must cost a coarse scan, never an unbounded loop.
const MAX_SCAN_CHUNKS: usize = 8192;

/// Split `a`→`b` into consecutive sub-segments short enough that each one's
/// axis-aligned box is proportional to `reach`, not to the whole stroke.
/// See [`segment`] for why this is the whole effect-line fix.
///
/// The floor of 8 px keeps the per-chunk bookkeeping from dominating a
/// hairline: at `reach` ≈ 1.5 a chunk box is about 11 × 11, so the scan
/// costs roughly 15 tests per pixel of length instead of the length itself.
fn scan_chunks(a: [f32; 2], b: [f32; 2], reach: f32) -> impl Iterator<Item = ([f32; 2], [f32; 2])> {
    let d = [b[0] - a[0], b[1] - a[1]];
    let len = d[0].hypot(d[1]);
    let steps = ((len / (reach * 2.0).max(8.0)).ceil() as usize).clamp(1, MAX_SCAN_CHUNKS);
    (0..steps).map(move |s| {
        let (u0, u1) = (s as f32 / steps as f32, (s + 1) as f32 / steps as f32);
        (
            [a[0] + d[0] * u0, a[1] + d[1] * u0],
            [a[0] + d[0] * u1, a[1] + d[1] * u1],
        )
    })
}

/// Fill one tooth by scanning ITS bbox — the same clip-first rule as
/// [`segment`], and for the same reason: an off-canvas flash centre with
/// a page-sized outer radius is an unbounded scan and an unbounded tile
/// allocation, not a slow one.
fn fill_tooth(map: &mut HashMap<TileIdx, Tile>, c: [f32; 2], t: &Tooth, size: (u32, u32)) {
    let apex = [c[0] + t.c * t.r_apex, c[1] + t.s * t.r_apex];
    let base = [c[0] + t.c * t.r_base, c[1] + t.s * t.r_base];
    let off = [-t.s * t.hw, t.c * t.hw];
    let xs = [apex[0], base[0] + off[0], base[0] - off[0]];
    let ys = [apex[1], base[1] + off[1], base[1] - off[1]];
    let x0 = (xs.iter().copied().fold(f32::INFINITY, f32::min) - 1.0).max(0.0);
    let x1 = (xs.iter().copied().fold(f32::NEG_INFINITY, f32::max) + 1.0).min(size.0 as f32);
    let y0 = (ys.iter().copied().fold(f32::INFINITY, f32::min) - 1.0).max(0.0);
    let y1 = (ys.iter().copied().fold(f32::NEG_INFINITY, f32::max) + 1.0).min(size.1 as f32);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    // Same story as [`segment`], same fix (lag hunt 2026-09-10): a flash's
    // spike is long, thin and diagonal, so ITS box is quadratic in the
    // outer radius too. Walk it in chunks along the apex→base axis.
    //
    // Every chunk box is intersected with the WHOLE-tooth box computed
    // above, and that is load-bearing rather than tidy: `Tooth::hit` also
    // answers true a little way BELOW the near end when a ベタフラ's cut
    // flares, and today that extra is clipped by this very box. Clipping to
    // it keeps the clip exactly where it already was, so the pixel set does
    // not move. The chunk range covers the flare's reach as well, so
    // nothing inside the box is skipped either.
    let reach = t.hw.max(t.flare_hw).max(0.5) + 1.0;
    // A flaring cut keeps answering true all the way down to the centre
    // (`Tooth::hit` only stops at `along < 0.0`), so when one is armed the
    // walk starts at the centre. Read off `hit` rather than assumed: its
    // ramp is clamped at 1.0, it does not close again below the ramp.
    let r_lo = if t.flare_hw > t.hw {
        0.0
    } else {
        t.r_apex.min(t.r_base)
    };
    let r_hi = t.r_apex.max(t.r_base);
    let ray = |r: f32| [c[0] + t.c * r, c[1] + t.s * r];
    for (p, q) in scan_chunks(ray(r_lo), ray(r_hi), reach) {
        let cx0 = (p[0].min(q[0]) - reach).max(x0);
        let cx1 = (p[0].max(q[0]) + reach).min(x1);
        let cy0 = (p[1].min(q[1]) - reach).max(y0);
        let cy1 = (p[1].max(q[1]) + reach).min(y1);
        if cx0 >= cx1 || cy0 >= cy1 {
            continue;
        }
        for y in cy0.floor() as i32..=cy1.ceil() as i32 {
            for x in cx0.floor() as i32..=cx1.ceil() as i32 {
                if t.hit(x as f32 + 0.5 - c[0], y as f32 + 0.5 - c[1]) {
                    put(map, x, y);
                }
            }
        }
    }
}

/// The walk's ceiling on rays — the same number [`GenLinesSpec::ray_count`]
/// clamps a silly `gap_deg` to, for the same reason.
const MAX_RAYS: usize = 4096;

/// One step of a bundled walk: how many lines the NEXT bundle holds, and
/// how wide the hole after it is in multiples of the base gap. Shared by
/// both walks so a bundle means the same thing in degrees and in pixels.
///
/// `group_jit` 0 returns exactly `(group, group_gap)` and draws NO random
/// number — the constant-size bundle both walks have always had, kept bit
/// for bit. That constant is what the critic read as "a picket fence of
/// pairs at a near-constant pitch" in `sparse-stream` and `drip-lines`
/// (round 2): a repeated unit is a texture, and the eye finds the period
/// in about a second. Above 0 the size is drawn from `1..=group` (biased
/// toward `group`, because a set of mostly-full bundles with the odd
/// single is what a hand does) and the hole wobbles by `±group_jit/2` of
/// itself, so no two bundles are the same shape.
fn walk_step(group: u32, group_gap: f32, group_jit: f32, seed: &mut u64) -> (u32, f32) {
    let g = group.max(1);
    // A hole only exists once there is something to bundle; `group <= 1`
    // is a plain walk at the gap, hole multiplier 1×.
    let hole = if group > 1 { group_gap.max(1.0) } else { 1.0 };
    if group_jit <= 0.0 || g <= 1 {
        return (g, hole);
    }
    let j = group_jit.clamp(0.0, 1.0);
    let cut = (rand(seed) * j * (g - 1) as f32).round() as u32;
    (
        g.saturating_sub(cut).max(1),
        hole * (1.0 + (rand(seed) - 0.5) * j),
    )
}

/// Where a bundled walk puts its runs across `lo..=hi`, stepping `gap`
/// and leaving a hole after each bundle. Its own function so the test can
/// read the bundle sizes back off it — the thing the critic reads off the
/// page is exactly this list.
fn walk_offsets(
    lo: f32,
    hi: f32,
    gap: f32,
    group: u32,
    group_gap: f32,
    group_jit: f32,
    seed: &mut u64,
) -> Vec<f32> {
    let mut out = Vec::new();
    let (mut bundle, mut hole) = walk_step(group, group_gap, group_jit, seed);
    let (mut t, mut i) = (lo, 0u32);
    while t <= hi && (out.len() as u32) < MAX_RUNS {
        out.push(t);
        i += 1;
        if i >= bundle {
            t += gap * hole;
            i = 0;
            (bundle, hole) = walk_step(group, group_gap, group_jit, seed);
        } else {
            t += gap;
        }
    }
    out
}

/// Where a radial set's rays sit, or `None` for the legacy layout
/// (`i · 2π / count`, one full even circle) — which is what every file
/// saved before the sweep and the angular bundle walk existed drew, so
/// `None` has to stay reachable by exactly the old field values.
///
/// The walk mirrors [`render_speed`]'s: `group` rays a `gap_deg` apart,
/// then a hole of `group_gap × gap_deg`. Bundling needs a gap to be a
/// multiple OF, so — like the speed walk keying on `gap_px` — it is only
/// read when `gap_deg` > 0.
///
/// A SWEEP walks too, bundles or not (`group <= 1` = a walk with no
/// holes). It has to: the even-spread branch below spreads `count` rays
/// over the arc, and for a gap-driven set `count` is
/// [`GenLinesSpec::ray_count`]'s 360/gap — so a 3° set swept to 170°
/// used to squeeze all 120 rays into that arc at double density. The
/// even spread is now only for `gap_deg == 0`, where `count` IS the
/// stored ray count and means what it says.
///
/// `seed` only matters when `group_jit` > 0, and it seeds a stream of its
/// OWN: [`GenLinesSpec::ray_count`] calls this outside any render to size
/// the set, so the walk cannot draw from the per-ray sequence
/// [`render_focus`] uses or the two would disagree about how many rays
/// there are. The constant is just a decorrelating splash so bundle sizes
/// do not shadow the first ray's angle jitter.
fn radial_angles(
    count: u32,
    gap_deg: f32,
    sweep_deg: f32,
    center_deg: f32,
    group: u32,
    group_gap: f32,
    group_jit: f32,
    seed: u64,
) -> Option<Vec<f32>> {
    let walk = gap_deg > 0.0 && (group > 1 || sweep_deg > 0.0);
    if !walk && sweep_deg <= 0.0 {
        return None;
    }
    let sweep = if sweep_deg > 0.0 {
        sweep_deg.min(360.0)
    } else {
        360.0
    }
    .to_radians();
    let start = center_deg.to_radians() - sweep * 0.5;
    let mut out = Vec::new();
    if walk {
        let gap = gap_deg.to_radians();
        let mut s = (seed ^ 0x517C_C1B7_2722_0A95) | 1;
        // No bundle asked for = a plain walk at the gap: `walk_step`
        // returns `(1, 1.0)` and a hole of 1× is no hole.
        let (mut bundle, mut hole) = walk_step(group, group_gap, group_jit, &mut s);
        let (mut a, mut i) = (start, 0u32);
        while a < start + sweep && out.len() < MAX_RAYS {
            out.push(a);
            i += 1;
            if i >= bundle {
                a += gap * hole;
                i = 0;
                (bundle, hole) = walk_step(group, group_gap, group_jit, &mut s);
            } else {
                a += gap;
            }
        }
    } else {
        // Sweep without bundling: `count` rays spread evenly over the arc.
        let n = count.max(1).min(MAX_RAYS as u32);
        let step = sweep / n as f32;
        out.extend((0..n).map(|i| start + i as f32 * step));
    }
    (!out.is_empty()).then_some(out)
}

/// Render focus lines into sparse tiles (opaque black premul fix15).
pub fn render_focus(p: &FocusLinesParams, size: (u32, u32)) -> HashMap<TileIdx, Arc<Tile>> {
    let mut map: HashMap<TileIdx, Tile> = HashMap::new();
    let mut seed = p.seed | 1;
    let span = (p.r_out - p.r_in).max(1.0);
    let bases = radial_angles(
        p.count,
        p.gap_deg,
        p.sweep_deg,
        p.sweep_center_deg,
        p.group,
        p.group_gap,
        p.group_jit,
        p.seed,
    );
    let n = bases.as_ref().map_or(p.count.max(1), |v| v.len() as u32);
    // The unit the angle jitter is a fraction OF: the walk's own step
    // when it walks, else the even circle's gap — which is the value the
    // legacy path used and must keep using.
    let unit = match &bases {
        Some(v) if v.len() >= 2 => v[1] - v[0],
        _ => std::f32::consts::TAU / n as f32,
    };
    let prof = p.mix.profile(p.taper);
    // The OUTER end's own jitter — 0 means "the same as the inner end",
    // which is what every file saved before the split drew. See
    // `FocusLinesParams::jit_len_out`.
    let out_jit = if p.jit_len_out > 0.0 {
        p.jit_len_out
    } else {
        p.length_jitter
    };
    let (mut prev_accent, mut accent_acc) = (false, 0.0f32);
    for i in 0..n {
        let base = match &bases {
            Some(v) => v[i as usize],
            None => i as f32 * std::f32::consts::TAU / n as f32,
        };
        let ang = base + (rand(&mut seed) - 0.5) * p.angle_jitter * unit;
        // The two ends pull IN from the ring by a skewed draw: a printed
        // 集中線 has its inner ends scattered over a wide band with most
        // rays long (ref-10, ref-11), where a uniform draw makes the
        // fuzzy-but-even ring the critic keeps seeing.
        let mut r1 = p.r_in + p.mix.skew(rand(&mut seed)) * p.length_jitter * span * 0.5;
        // ...and then a BAND around `r_in`, downward: see
        // `FocusLinesParams::core_jit`. Clamped at 0 so a big jitter on a
        // small hole cannot flip the ray inside out.
        if p.core_jit > 0.0 {
            r1 = (r1 - p.r_in * p.core_jit.clamp(0.0, 1.0) * rand(&mut seed)).max(0.0);
        }
        let r2 = p.r_out - p.mix.skew(rand(&mut seed)) * out_jit * span * 0.5;
        let w = p.width * (1.0 - rand(&mut seed) * p.width_jitter);
        let hw = p
            .mix
            .accent((w * 0.5).max(0.5), &mut seed, &mut prev_accent, &mut accent_acc);
        let (s, c) = ang.sin_cos();
        // OUTER first: segment() tapers toward `b`, and a focus ray thins
        // toward the convergence (the inner end). With taper 0 the order
        // is invisible — the distance test is symmetric — so legacy
        // renders stay bit-stable (pinned test).
        segment(
            &mut map,
            [p.center[0] + c * r2, p.center[1] + s * r2],
            [p.center[0] + c * r1, p.center[1] + s * r1],
            hw,
            prof,
            size,
        );
    }
    // Clip: drop fully-off-canvas tiles; per-pixel clipping happened in put.
    let (w, h) = (size.0 as i32, size.1 as i32);
    map.retain(|idx, _| {
        let (ox, oy) = idx.origin();
        ox < w && oy < h && ox + TILE_SIZE as i32 > 0 && oy + TILE_SIZE as i32 > 0
    });
    map.into_iter().map(|(k, v)| (k, Arc::new(v))).collect()
}

/// Render speed lines: parallel runs scattered across the canvas along
/// `angle_deg`, each starting within the canvas's perpendicular extent.
pub fn render_speed(p: &SpeedLinesParams, size: (u32, u32)) -> HashMap<TileIdx, Arc<Tile>> {
    let mut map: HashMap<TileIdx, Tile> = HashMap::new();
    let mut seed = p.seed | 1;
    let (w, h) = (size.0 as f32, size.1 as f32);
    let rad = p.angle_deg.to_radians();
    let dir = [rad.cos(), rad.sin()];
    let nrm = [-rad.sin(), rad.cos()];
    // The canvas extent along the normal — lines scatter across it.
    let corners = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]];
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    // The same projection ALONG the direction — how much room a run has
    // to sit in. The walk needs it (see `start_along` below); the legacy
    // scatter never asked.
    let mut a_lo = f32::INFINITY;
    let mut a_hi = f32::NEG_INFINITY;
    for c in corners {
        let t = c[0] * nrm[0] + c[1] * nrm[1];
        lo = lo.min(t);
        hi = hi.max(t);
        let u = c[0] * dir[0] + c[1] * dir[1];
        a_lo = a_lo.min(u);
        a_hi = a_hi.max(u);
    }
    // Where the runs sit along the normal. The WALK (gap_px > 0) steps a
    // fixed gap with optional bundling; the legacy path scatters `count`
    // of them at uniform-random offsets and is kept bit for bit, so no
    // saved layer redraws (`legacy_renders_are_bit_stable`). Every new
    // `rand` call lives behind a `> 0.0` guard for the same reason: the
    // draw sequence has to be untouched when the new knobs are absent.
    let mut offsets: Vec<f32> = Vec::new();
    if p.gap_px > 0.0 {
        // A CONVERGED run does not travel along the shared direction, so
        // the canvas's extent along the shared NORMAL under-covers it.
        // Every run in a fan passes through the vanishing point, so the
        // panel corner on the FAR side of the fan lies on the line of
        // exactly one run — the one based at that corner — and the walk
        // stops there. That corner measured 4.9 % ink against a 9.9 %
        // panel mean, the emptiest patch on the whole sheet (gauntlet
        // critic, rounds 2 and 3). It is the same mistake round 2 found
        // in the along-extent fit, on the other axis: an extent solved in
        // a basis the run never takes.
        //
        // A quarter of the extent either side gives the fan bases just
        // outside the canvas that sweep INTO those corners, which is what
        // a converging streak block does on paper. The runs that miss the
        // canvas cost one clipped bbox test each. Parallel sets — every
        // shipped preset but `perspective-stream` — are untouched.
        let pad = if p.converge.is_some() {
            (hi - lo) * 0.25
        } else {
            0.0
        };
        offsets = walk_offsets(
            lo - pad,
            hi + pad,
            p.gap_px.max(0.25),
            p.group,
            p.group_gap,
            p.group_jit,
            &mut seed,
        );
    }
    let n = if offsets.is_empty() {
        p.count.max(1)
    } else {
        offsets.len() as u32
    };
    let prof = p.mix.profile(p.taper);
    let (mut prev_accent, mut accent_acc) = (false, 0.0f32);
    // The extent along ANY direction, not just the shared one — see the
    // per-run fit below. Corners are the only points that can bound it.
    let extent_along = |d: [f32; 2]| {
        corners.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(l, h), c| {
            let u = c[0] * d[0] + c[1] * d[1];
            (l.min(u), h.max(u))
        })
    };
    for i in 0..n {
        let mut t = match offsets.get(i as usize) {
            Some(t) => *t,
            None => lo + rand(&mut seed) * (hi - lo),
        };
        if p.jit_gap > 0.0 {
            t += (rand(&mut seed) - 0.5) * p.jit_gap.clamp(0.0, 1.0) * p.gap_px.max(0.25);
        }
        // The spread draw, biased toward len_max when `len_skew` is on —
        // a printed streak block (ref-07) is mostly long strokes with a
        // few stubs, which a flat draw states as "every length equally".
        let u = rand(&mut seed);
        let u = if p.mix.len_skew > 0.0 {
            1.0 - p.mix.skew(1.0 - u)
        } else {
            u
        };
        let mut len = p.len_min + u * (p.len_max - p.len_min).max(0.0);
        if p.jit_len > 0.0 {
            len *= 1.0 - p.mix.skew(rand(&mut seed)) * p.jit_len.clamp(0.0, 0.9);
        }
        // Where the run's base sits along the direction. `t` is already
        // the ABSOLUTE normal coordinate (corner projection) — no
        // canvas-centre offset.
        // Where this run actually POINTS. With `converge` that is not the
        // shared `dir`, and the along-extent fit below has to be solved in
        // the run's own basis or a leaning run gets its start solved for a
        // direction it never takes: it stops short, and on
        // `perspective-stream` a whole quarter of the panel came out at
        // 1.8 % ink against a 13.9 % mean (gauntlet critic, round 2).
        //
        // Two passes, because the direction depends on the base and the
        // base depends on the direction: a PROVISIONAL base at the middle
        // of the along-extent gives a direction close enough to fit
        // against, then the final base gives the direction that is drawn.
        // With `converge` None both passes are `dir` and every line below
        // collapses to the arithmetic it replaced, bit for bit.
        let aim = |b: [f32; 2]| match p.converge {
            Some(v) => {
                let (vx, vy) = (v[0] - b[0], v[1] - b[1]);
                let l = vx.hypot(vy);
                if l > 1e-3 { [vx / l, vy / l] } else { dir }
            }
            None => dir,
        };
        let start_along = if p.start_mode == 1 {
            // Hang off the reference line: every run's base lands on the
            // line through `anchor` perpendicular to the direction, so
            // the block has one straight edge and ragged tails (ref-09).
            let base_at = p.anchor.map_or(0.0, |a| a[0] * dir[0] + a[1] * dir[1]);
            // BACKWARD, never forward. A start pushed PAST the reference
            // line leaves the run hanging in mid-air a millimetre or two
            // inside the panel with its own base cap showing, which is
            // what the drips regressed to: 3 of ~40 reached row 0 and the
            // rest floated with a fine point aimed at the frame (critic,
            // round 3). ref-09 cuts every line hard ON the frame line.
            // Subtracted, the wobble is spent OUTSIDE the panel, so it
            // varies the far ends — the thing you can see — and the
            // starts all read as one straight cut.
            let off = if p.jit_start > 0.0 {
                rand(&mut seed) * p.jit_start.clamp(0.0, 1.0) * len
            } else {
                0.0
            };
            base_at - off
        } else if p.gap_px > 0.0 {
            // FIT the run inside the canvas's along-extent instead of
            // scattering it over `w.max(h) + len`.
            //
            // This is where the density went. The old draw spreads a
            // run's start over the canvas PLUS a whole run length and
            // then shifts it back by another half length, so at any
            // cross-section only about `len / (extent + len)` of the runs
            // are present — for a panel-crossing 流線 that is half of
            // them, and the "dense" preset measured 14 strokes per 25 mm
            // where its own 0.6 mm gap asks for 36 (gauntlet critic,
            // round 1: "dense is not dense"). Nothing was dropping
            // `gap_px`; the walk laid the runs out correctly and the
            // along-offset threw half of them off the page.
            //
            // `slack` is negative when the run outruns the extent, and
            // then both ends of the draw still cover it end to end — so a
            // long run always crosses and a short one lands anywhere
            // inside, which is the reference's "many never cross the
            // panel" without the bald half.
            //
            // Guarded on the WALK (`gap_px` > 0): the scatter is what
            // every pre-density file drew and it is pinned bit for bit.
            let mid = (a_lo + a_hi) * 0.5;
            let r0 = aim([nrm[0] * t + dir[0] * mid, nrm[1] * t + dir[1] * mid]);
            let (u_lo, u_hi) = extent_along(r0);
            let u = u_lo + rand(&mut seed) * (u_hi - u_lo - len);
            // `u` is measured along the RUN; the base point is built in
            // the shared (nrm, dir) basis, so convert. `dir·r0` is exactly
            // 1 with no convergence — the whole expression is then
            // `a_lo + rand·(a_hi − a_lo − len)`, the line it replaced —
            // and a run aimed nearly sideways to `dir` cannot be placed
            // along `dir` at all, so under 0.2 it falls back.
            let dr = dir[0] * r0[0] + dir[1] * r0[1];
            let nr = nrm[0] * r0[0] + nrm[1] * r0[1];
            if dr.abs() > 0.2 { (u - t * nr) / dr } else { u }
        } else {
            rand(&mut seed) * (w.max(h) + len) - len - len * 0.5
        };
        let base = [
            nrm[0] * t + dir[0] * start_along,
            nrm[1] * t + dir[1] * start_along,
        ];
        // Convergence aims the run at a far point instead of along the
        // shared direction; the SCATTER stays the parallel layout's, so
        // the fan is a lean on the block rather than a second tool.
        let mut run = aim(base);
        // …and then the hand's own wobble. Without it a block is EXACTLY
        // parallel, which is measurable (the critic did: "3 px over
        // 23 mm") and is the single most machine-made thing about a
        // generated 流線. Guarded, so a dead-parallel spec stays dead
        // parallel and bit-stable.
        if p.jit_angle > 0.0 {
            let (s, c) = ((rand(&mut seed) - 0.5) * p.jit_angle.to_radians()).sin_cos();
            run = [run[0] * c - run[1] * s, run[0] * s + run[1] * c];
        }
        let tip = [base[0] + run[0] * len, base[1] + run[1] * len];
        let mut hw = p.width * 0.5;
        if p.jit_width > 0.0 {
            hw *= 1.0 - rand(&mut seed) * p.jit_width.clamp(0.0, 0.9);
        }
        let hw = p
            .mix
            .accent(hw.max(0.5), &mut seed, &mut prev_accent, &mut accent_acc);
        segment(&mut map, base, tip, hw, prof, size);
    }
    let (wi, hi_) = (size.0 as i32, size.1 as i32);
    map.retain(|idx, _| {
        let (ox, oy) = idx.origin();
        ox < wi && oy < hi_ && ox + TILE_SIZE as i32 > 0 && oy + TILE_SIZE as i32 > 0
    });
    map.into_iter().map(|(k, v)| (k, Arc::new(v))).collect()
}

/// The angular half-extent of a tooth: its half-width at radius `r` is
/// `hw · (r − apex)/(base − apex)`, so `w(r)/r` peaks at the base and the
/// tooth can never reach further round than `asin(hw / r_base)`. Every
/// tooth that covers a pixel therefore lies within this angle of it —
/// which is what lets the solid scan bound its search by MEASUREMENT
/// instead of by the ±1 sector the bundle walk would have broken.
fn tooth_reach(t: &Tooth) -> f32 {
    // A flared cut is at its widest a flare below its own base, so that
    // is the radius the angle has to be taken at. Without a flare this is
    // the arithmetic it always was, float for float.
    let (w, r) = if t.flare_hw > t.hw {
        (
            t.flare_hw,
            (t.r_base - FLARE_SLOPE * (t.flare_hw - t.hw)).max(1.0),
        )
    } else {
        (t.hw, t.r_base.max(1.0))
    };
    (w / r).clamp(0.0, 1.0).asin()
}

/// The most teeth any arc `reach` wide can hold, given their sorted
/// angles — so testing `idx ± win` either side of a query's insertion
/// point provably covers every tooth within `reach` of it, wrap included.
fn candidate_window(angs: &[f32], reach: f32) -> usize {
    let n = angs.len();
    if n == 0 || !(reach > 0.0) {
        return 0;
    }
    let ext: Vec<f32> = angs
        .iter()
        .copied()
        .chain(angs.iter().map(|a| a + std::f32::consts::TAU))
        .collect();
    (0..n)
        .map(|i| ext.partition_point(|v| *v <= angs[i] + reach) - i)
        .max()
        .unwrap_or(1)
        .min(n)
}

/// Bins in the ragged-core table. 2048 over a full circle is finer than
/// any tooth pitch the count clamp allows, so the table never smooths a
/// tooth away.
const CORE_LUT: usize = 2048;

/// Where the solid variant's ink STARTS, per angle: interpolated between
/// the two angularly neighbouring teeth's apexes, so the black core's
/// edge follows the teeth instead of sitting on a compass circle
/// (gauntlet critic, round 4: "solid-flash's core is a perfect circle",
/// scored 1/5 — its ring scan began at `r_in` whatever the teeth did, so
/// no knob could reach it).
///
/// Written as `a + (b − a) · t`: with every apex equal — `core_jit` 0 and
/// no apex jitter, i.e. every saved flash — each bin is EXACTLY `r_in`
/// and the scan below is the old `r² < r_in²` test, bit for bit.
fn core_lut(teeth: &[Tooth], angs: &[f32], win: usize) -> Vec<f32> {
    let n = teeth.len();
    let raw: Vec<f32> = (0..CORE_LUT)
        .map(|k| {
            let a = k as f32 * std::f32::consts::TAU / CORE_LUT as f32;
            let hi = angs.partition_point(|v| *v < a) % n;
            let lo = (hi + n - 1) % n;
            let d = (angs[hi] - angs[lo]).rem_euclid(std::f32::consts::TAU);
            let t = if d > 1e-6 {
                ((a - angs[lo]).rem_euclid(std::f32::consts::TAU) / d).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let (a0, a1) = (teeth[lo].r_inner(), teeth[hi].r_inner());
            a0 + (a1 - a0) * t
        })
        .collect();
    if win == 0 {
        return raw;
    }
    // …then a MIN over ±`win` bins.
    //
    // The interpolation alone leaves CRESCENTS, and they were 355 of the
    // 357 "floating fragments" this round started with — critic 1's
    // "isolated 1–3 px black dots inside the white hole … on paper these
    // read as dirt on the scan", every one of them measured within a
    // millimetre of the core edge.
    //
    // Where they come from: the black between two neighbouring cuts is at
    // its NARROWEST just above their fat ends, because below a cut's base
    // that cut stops and the black abruptly widens. The interpolated
    // boundary lands in exactly that pinch, so what survives is a
    // sub-pixel wedge, and a hard-edged test prints a sub-pixel wedge as a
    // row of dots. Pushing the boundary DOWN past the pinch — a minimum
    // over about three quarters of a pitch, which is the arc a base cut
    // spans — means the black that survives is the wide part below it, so
    // there is nothing left to dot. (Lifting the boundary instead was
    // tried and doubles the count: it parks the boundary in the pinch on
    // purpose.)
    //
    // It is not the whole answer. The pinch has to be a couple of pixels
    // wide in the first place, which is `1 − width_frac` times the pitch
    // at the hole — see the Solid flash preset's count.
    //
    // With every apex equal it is a min over a constant — still exactly
    // `r_in`, so no saved flash moves.
    let w = win.min(CORE_LUT / 2);
    (0..CORE_LUT)
        .map(|k| {
            (0..=2 * w)
                .map(|d| raw[(k + CORE_LUT + d - w) % CORE_LUT])
                .fold(f32::INFINITY, f32::min)
        })
        .collect()
}

/// How far a sliver's POINT may differ from its bundle's shared peak, as
/// a fraction of the ring's span. A bundle is a STACK of slivers with one
/// peak, not a clump: at 0.03 the pack still reads as a single spike.
const SLIVER_JIT: f32 = 0.03;

/// The hole, in three parts — all of them measured against `core_jit`,
/// so that knob still says how ragged the hole is, and NONE of them ever
/// pushes a stroke OUTWARD past the hole curve.
///
/// That last word is the whole of critic 2's fix 1. Builder B's hole was
/// a lobe plus a per-bundle draw that ran BOTH ways, so a pack could
/// start a couple of millimetres outside its neighbours with clean white
/// all round its inner end — a leaf floating in the annulus, which is
/// ref-15's plain-flash signature and a rubric ban. ref-17 and ref-18 are
/// unambiguous: every stroke starts ON the hole edge and only ever runs
/// outward.
///
/// So the hole is now ONE smooth curve — `r_in · (1 + core_jit ·
/// HOLE_LOBE · lobe(θ))`, continuous in θ, so neighbouring bundles sit on
/// nearly the same radius — and both jitters bite INWARD from it: a small
/// per-bundle one and a bigger per-stroke one. Ink therefore begins at or
/// inside the curve at every angle, the ring cannot break, and the
/// raggedness that is left is at STROKE scale (critic 2: ours was "at
/// bundle scale … both measure 20 %; only one looks inked").
const HOLE_LOBE: f32 = 0.75;
/// …the per-BUNDLE pull-in, inward only.
const HOLE_BUMP: f32 = 0.18;
/// …and the per-STROKE one, which is the serration a reader sees.
const HOLE_CHEW: f32 = 0.24;

/// The nib's 入り floor: an entry ramp is never shorter than this many of
/// the stroke's own half-widths, whatever fraction of the LENGTH that
/// works out as.
///
/// [`Tooth::entry`] is a fraction of the length, and Builder B left the
/// consequence written down: at half the burst size the ramp is half as
/// long in pixels, so `sea-urchin-flash-tight` reported 9 blunt ends
/// where the shipped row reports 0. A ramp reads as a point when it is
/// long against the WIDTH it ramps from — that is a property of the nib,
/// not of the burst — so the floor is stated there.
const ENTRY_NIB: f32 = 7.0;
/// …and it never eats more than this much of the stroke, or a fat accent
/// on a short stroke turns into ref-15's double-tapered spindle.
const ENTRY_MAX: f32 = 0.45;

/// How steeply a ベタフラ's cut flares below its own fat end: the depth
/// it takes to reach [`Tooth::flare_hw`], as a multiple of the width it
/// has to gain. 1.2 makes the black valley between two bundles close at
/// roughly a 70° point — pointed enough to read as a spike's end, steep
/// enough that the last sub-pixel stretch of black is under a pixel of
/// radius and cannot print as a row of dots.
const FLARE_SLOPE: f32 = 0.7;

/// The narrowest black a ベタフラ may leave between two neighbouring
/// cuts, in px. A pixel and a bit: under one pixel a hard-edged raster
/// prints a thread as a dashed line, which is dirt on the page and
/// `frags` on the table.
const BLACK_MIN: f32 = 2.4;

/// How much shorter a bundle's OUTERMOST sliver is than its middle one,
/// as a fraction of the ring's span.
///
/// This is the zigzag, and it is the whole of critic 1's first fix. A
/// pack whose members are all the same length has a flat-topped envelope
/// and reads as a comb; ref-23's author draws a red sawtooth over a real
/// inked boundary captioned 「このジグザグをつくる」 ("make this zigzag"),
/// and what makes it is a pack of ~5 strokes leaving the black mass
/// shoulder to shoulder with **the middle one longest**. So the length
/// tapers away from the pack's centre member and the pack's tip is a
/// triangle.
const PEAK_DROP: f32 = 0.16;

/// A smooth, seeded, three-harmonic wobble around the circle, −1..1.
///
/// The hole of a printed ウニフラ is not white noise per stroke: ref-17's
/// fat starts "stack against it with visible gaps and overlaps", and
/// ref-18 has a whole 70° arc where they ran together into solid black.
/// Both are LOW-frequency — the hole has lobes, some arcs chewed deep and
/// some barely — and an independent draw per bundle cannot make one,
/// because it averages out over any arc wide enough to see. Three
/// harmonics (2, 5 and 11 round the circle) give arcs a few tens of
/// degrees wide, which is the scale ref-18 fuses at.
fn lobe(ang: f32, ph: [f32; 3]) -> f32 {
    0.45 * (ang * 2.0 + ph[0]).sin()
        + 0.32 * (ang * 5.0 + ph[1]).sin()
        + 0.23 * (ang * 11.0 + ph[2]).sin()
}

/// Render a sea-urchin flash (or, with `solid`, its inverse) into sparse
/// tiles. Deterministic under `seed` like the other two.
///
/// Since Lane F (2026-09-07) the structure is BUNDLES, not rays. Three
/// independent Japanese sources say the same thing and it is the
/// highest-confidence fact in the reference pack: Clip Studio's own flash
/// tool carries a まとまり ("grouping") setting and labels its example
/// spikes 2本/3本/4本 (ref-24); a close-up of a professionally inked
/// analog ベタフラ resolves every visual spike into 4–10 parallel slivers
/// that peak together (ref-22); a manga school states the budget as 4–6
/// lines per big peak and 2–3 per small one (ref-23). A flash is `M`
/// bundles of a randomised few teeth separated by a gap several times the
/// within-bundle one — NOT `N` evenly spaced rays, at any `N`.
///
/// The teeth of one bundle share their bundle's apex and tip radii, give
/// or take [`SLIVER_JIT`]. That sharing is what makes a bundle read as
/// ONE peak built of slivers rather than as a ragged clump, and it is the
/// sawtooth ref-23's author annotates 「このジグザグをつくる」.
pub fn render_urchin(p: &UrchinParams, size: (u32, u32)) -> HashMap<TileIdx, Arc<Tile>> {
    let mut map: HashMap<TileIdx, Tile> = HashMap::new();
    let mut seed = p.seed | 1;
    let n0 = p.count.max(1);
    let step = std::f32::consts::TAU / n0 as f32;
    let r_out = p.r_out.max(1.0);
    let r_in = p.r_in.clamp(0.0, r_out - 1.0);
    let span = r_out - r_in;
    // WHERE the teeth sit. `radial_angles` is the focus kind's own walk,
    // reused rather than forked: `group` teeth a pitch apart, then a
    // hole. It returns `None` for the plain even circle — what every
    // saved flash drew — and its `rand()`s run on their own seed stream.
    let bases = radial_angles(
        n0,
        360.0 / n0 as f32,
        p.sweep_deg,
        p.sweep_center_deg,
        p.group,
        p.group_gap,
        p.group_jit,
        p.seed,
    );
    let n = bases.as_ref().map_or(n0 as usize, |v| v.len()).max(1);
    // The nominal half-width. `width_frac` states it against the PITCH at
    // the rim (see the field): a ベタフラッシュ needs a cut WIDER than the
    // pitch, because the black that survives between two cuts is the
    // spike, and it only comes to a needle point where the two cuts meet
    // — at a radius inside the ring, different for every pair. That pinch
    // is the sawtooth; a narrow cut gives a black panel with scratches.
    // The radius the WIDE end sits at — the rim normally, the hole when
    // the tooth is the other way up. Everything stated "against the
    // pitch" has to be stated against the pitch THERE, or an urchin's
    // fat starts are sized by an arc six times longer than the one they
    // actually stand on.
    let r_wide = if p.tip_out { r_in.max(1.0) } else { r_out };
    let hw = if p.width_frac > 0.0 {
        (step * r_wide * 0.5 * p.width_frac.clamp(0.0, 3.0)).max(0.5)
    } else {
        // See UrchinParams::width — 90% of the gap, never more. The cap is
        // floored before the clamp because f32::clamp PANICS on min > max
        // and a tiny flash would otherwise abort through wndproc (audit B).
        (p.width * 0.5).clamp(0.5, (step * r_out * 0.45).max(0.5))
    };
    // An accent is by definition a tooth wider than the rule; `Mix::accent`
    // refuses two in a row, so its neighbours are nominal. Three pitches
    // is the widest that still leaves the pair legible, and the floor
    // stops a degenerate flash from inverting the bound.
    let hw_max = (step * r_wide * 3.0).max(hw);
    // The tooth's own stroke profile. Both were documented as "a tooth is
    // a filled triangle, so it has no ramp to bend" and ignored; both are
    // now the answer to a critic-1 fix, and both read 0 as the straight
    // wedge every saved flash drew (see `Tooth::needle` / `Tooth::entry`).
    let nib = if p.mix.needle > 0.0 {
        p.mix.needle
    } else {
        1.0
    };
    // How wide a ベタフラ's cut gets below its base, in PITCHES. The
    // widest black a flare has to close is a whole between-bundle valley
    // — `group_gap` pitches — so half of that, plus a little, is what
    // makes every black spike end in a point instead of a flat.
    let flare_pitch = 0.60 * p.group_gap.max(1.0) * (1.0 + p.group_jit.clamp(0.0, 1.0));
    let aj = p.angle_jitter.clamp(0.0, 0.5);
    let lj = p.length_jitter.clamp(0.0, 1.0);
    // Which end the length jitter moves — see `UrchinParams::jit_len_out`.
    // With it 0 this is the legacy pair (outer only) and the apex draw
    // below is skipped, so the `rand()` sequence does not shift.
    let (in_jit, out_jit) = if p.jit_len_out > 0.0 {
        (lj, p.jit_len_out.clamp(0.0, 1.0))
    } else {
        (0.0, lj)
    };
    let (mut prev_accent, mut accent_acc) = (false, 0.0f32);
    let mut teeth: Vec<Tooth> = Vec::with_capacity(n);
    let (mut b_base, mut b_apex) = (r_out, r_in);
    // WHICH BUNDLE each tooth belongs to, as `(place in the pack, pack
    // size)`. A new bundle starts wherever the walk left a hole. Read
    // back off the angles rather than reported by `radial_angles`, so the
    // walk stays shared with the focus kind — but read UP FRONT, because
    // the pack's silhouette needs its size before its first member is
    // placed (see `PEAK_DROP`).
    let packs: Vec<(usize, usize)> = match &bases {
        Some(v) => {
            let mut out = vec![(0usize, 1usize); v.len()];
            let mut start = 0usize;
            for i in 0..=v.len() {
                let cut = i == v.len() || (i > 0 && (v[i] - v[i - 1]) > step * 1.5);
                if cut {
                    for (j, k) in (start..i).enumerate() {
                        out[k] = (j, i - start);
                    }
                    start = i;
                }
            }
            out
        }
        None => Vec::new(),
    };
    // The hole's lobes (see `lobe`), on their own seed stream so they do
    // not shift the per-tooth sequence.
    let ph = {
        let mut s = (p.seed ^ 0x2545_F491_4F6C_DD1D) | 1;
        [
            rand(&mut s) * std::f32::consts::TAU,
            rand(&mut s) * std::f32::consts::TAU,
            rand(&mut s) * std::f32::consts::TAU,
        ]
    };
    for i in 0..n {
        let base = match &bases {
            Some(v) => v[i],
            None => i as f32 * step,
        };
        let (place, pack) = packs.get(i).copied().unwrap_or((0, 1));
        let fresh = place == 0;
        let ang = base + (rand(&mut seed) - 0.5) * aj * step;
        if fresh {
            // Half the span is as far as a jitter may pull an end on the
            // legacy path, so that two ends jittering toward each other
            // cannot cross. On the SPLIT path they cannot: only one end
            // takes this draw. A ベタフラ needs the longer reach — its
            // white spikes have to stop well inside the panel and in a
            // spread of lengths, and at half a span every one of them ran
            // off the frame (measured 2.9× the median where ref-21 is
            // 1.5×).
            let out_reach = if p.jit_len_out > 0.0 { 0.85 } else { 0.5 };
            b_base = r_out - p.mix.skew(rand(&mut seed)) * out_jit * span * out_reach;
            b_apex = r_in;
            // The draw happens either way so the `rand()` sequence does
            // not depend on the walk; only the LEGACY path spends it on
            // pushing the apex outward, because on a walked flash the
            // hole is a curve and nothing may start outside it (see
            // `HOLE_LOBE`).
            let push_in = if in_jit > 0.0 {
                p.mix.skew(rand(&mut seed)) * in_jit * span * 0.5
            } else {
                0.0
            };
            if bases.is_none() {
                b_apex += push_in;
            }
            if p.core_jit > 0.0 {
                // A WALKED flash's hole is ONE smooth lobed curve, and
                // this bundle sits on it and is only ever pulled INSIDE
                // it (see `HOLE_LOBE`/`HOLE_BUMP`). Every saved flash is
                // unwalked and keeps the one-sided pull-in from `r_in`,
                // off the same single `rand()`.
                let u = rand(&mut seed);
                let cj = p.core_jit.clamp(0.0, 1.0);
                if bases.is_some() {
                    b_apex = r_in * (1.0 + cj * HOLE_LOBE * lobe(ang, ph)) - r_in * cj * HOLE_BUMP * u;
                } else {
                    b_apex -= r_in * cj * u;
                }
            }
        }
        let (mut r_base, mut r_apex) = (b_base, b_apex);
        if bases.is_some() {
            // The pack's TRIANGLE: longest in the middle, tapering to its
            // edges, so the bundle's tip is a peak and the boundary a
            // sawtooth (see `PEAK_DROP`).
            if pack > 1 {
                let half = (pack - 1) as f32 * 0.5;
                let d = (place as f32 - half).abs() / half;
                r_base -= d * PEAK_DROP * span;
            }
            if !fresh {
                // The sliver's own small wobble about its bundle's peak.
                // Guarded on the walk, so an unbundled flash draws no
                // extra number.
                r_base -= rand(&mut seed) * SLIVER_JIT * span;
            }
            // …and its own bite out of the hole, INWARD ONLY, which is
            // the serration ref-12's 20 % is made of (see `HOLE_CHEW`).
            // The old draw here was ± half the bite, and the half that
            // pushed a stroke outward is what floated the leaves.
            if p.core_jit > 0.0 {
                r_apex -= r_in * p.core_jit.clamp(0.0, 1.0) * HOLE_CHEW * rand(&mut seed);
            }
        }
        let r_apex = r_apex.max(0.0).min(r_out - 1.0);
        let hw_i = p
            .mix
            .accent(hw, &mut seed, &mut prev_accent, &mut accent_acc)
            .min(hw_max);
        let (s, c) = ang.sin_cos();
        // `tip_out` swaps which end is the point — see the field. The two
        // radii themselves are drawn identically either way, so the
        // `rand()` sequence does not depend on the orientation.
        let (r_apex, r_base) = if p.tip_out {
            (r_base, r_apex)
        } else {
            (r_apex, r_base)
        };
        // The 入り, per tooth: the preset's fraction, floored at
        // `ENTRY_NIB` half-widths of THIS tooth so a fat accent on a
        // short stroke still arrives from a point, capped at `ENTRY_MAX`
        // so the floor can never turn a stroke into a spindle. 0 stays 0
        // — every saved flash draws the straight cut it always drew.
        let len_i = (r_base - r_apex).abs().max(1.0);
        let entry_i = if p.mix.entry > 0.0 {
            p.mix
                .entry
                .max(ENTRY_NIB * hw_i / len_i)
                .min(ENTRY_MAX)
        } else {
            0.0
        };
        teeth.push(Tooth {
            ang: ang.rem_euclid(std::f32::consts::TAU),
            c,
            s,
            r_apex,
            r_base,
            hw: hw_i,
            needle: nib,
            entry: entry_i,
            // A ベタフラ's cut runs on below its base, flaring to half the
            // bundle's own pitch so it merges with its neighbours and the
            // black between them ends in a point (see `Tooth::flare_hw`).
            flare_hw: if p.solid && p.width_frac > 0.0 {
                (step * r_wide * flare_pitch).max(hw_i)
            } else {
                0.0
            },
        });
    }

    if p.solid {
        // One scan over the ring, inking everything the teeth do NOT
        // cover. The candidates per pixel come from a BINARY SEARCH over
        // the teeth's own angles, with a window wide enough to hold every
        // tooth that could reach that far (`candidate_window`). The old
        // scan indexed by SECTOR and tested ±1, which both the bundle
        // walk and a wider-than-pitch cut break — a black tooth
        // straddling a white gap.
        teeth.sort_by(|a, b| a.ang.total_cmp(&b.ang));
        // THE BLACK GETS A FLOOR TOO. `Tooth::hit` floors the CUT at half
        // a pixel so a white sliver never prints as a dotted line; the
        // black between two cuts had no such floor, and once the count
        // goes up it is the thing that runs out of room first. Measured:
        // at 520 teeth the nominal thread is 2.5 px, but the angle wobble
        // and a 1.9× accent between them take pairs under a pixel, and a
        // 1-bit raster prints those as a dashed line — 52 specks, every
        // one of them a long dash rather than a spike tip (dumped and
        // read, not guessed).
        //
        // So each cut is capped at half its own tightest neighbour gap,
        // less `BLACK_MIN`. Both members of a pair take the same cap, so
        // their two half-widths can never sum past the gap, and the black
        // between any two cuts survives at a printable width. The pitch
        // is smallest at the hole, which is where this is measured.
        if p.width_frac > 0.0 && teeth.len() > 2 {
            let n = teeth.len();
            // Each tooth's tightest ANGULAR gap to a neighbour. Everything
            // below is that angle turned into arc length at some radius.
            let ang_gap: Vec<f32> = (0..n)
                .map(|i| {
                    let nx = (teeth[(i + 1) % n].ang - teeth[i].ang)
                        .rem_euclid(std::f32::consts::TAU);
                    let pv = (teeth[i].ang - teeth[(i + n - 1) % n].ang)
                        .rem_euclid(std::f32::consts::TAU);
                    nx.min(pv).max(1e-6)
                })
                .collect();
            // The arc one cut, one black thread and half of each
            // neighbour needs: two half-pixel cuts (the raster floor in
            // `Tooth::hit`) plus a printable thread.
            let need = 1.0 + BLACK_MIN;
            for i in 0..n {
                // A HOLE CANNOT BE CHEWED DEEPER THAN THE PITCH THERE CAN
                // CARRY. `core_jit`'s lobe takes a base down to two
                // thirds of `r_in` and the per-stroke chew takes it lower
                // still, and at that radius the same angle is two thirds
                // of the arc — so the black thread beside it goes under a
                // pixel and prints as a dashed line. (Dumped and read:
                // every speck on this row was a dash on the core
                // boundary in a deep lobe, never a spike tip.) Pulling
                // the base back out to where there IS room says the same
                // thing as "the lobe stops where the raster stops", and
                // the core's boundary follows the bases, so the picture
                // stays consistent with itself.
                let r_min = need / ang_gap[i];
                let (lo, hi) = (teeth[i].r_inner(), teeth[i].r_apex.max(teeth[i].r_base));
                if lo < r_min && hi > r_min + 1.0 {
                    if teeth[i].r_apex <= teeth[i].r_base {
                        teeth[i].r_apex = r_min;
                    } else {
                        teeth[i].r_base = r_min;
                    }
                }
            }
            // …and then the cut itself is capped at half its own tightest
            // gap, less the thread. Both members of a pair take the same
            // cap off the same gap, so their half-widths can never sum
            // past it.
            let arc = |i: usize, j: usize| {
                (teeth[j].ang - teeth[i].ang).rem_euclid(std::f32::consts::TAU)
                    * teeth[i].r_inner().min(teeth[j].r_inner()).max(1.0)
            };
            let caps: Vec<f32> = (0..n)
                .map(|i| {
                    let d = arc(i, (i + 1) % n).min(arc((i + n - 1) % n, i));
                    ((d - BLACK_MIN) * 0.5).max(0.5)
                })
                .collect();
            for (t, c) in teeth.iter_mut().zip(caps) {
                t.hw = t.hw.min(c);
            }
        }
        let angs: Vec<f32> = teeth.iter().map(|t| t.ang).collect();
        let reach = teeth.iter().map(tooth_reach).fold(0.0, f32::max);
        let win = candidate_window(&angs, reach);
        // No min-filter once the cuts FLARE (see `Tooth::flare_hw`): the
        // filter existed to push the boundary past the pinch above a
        // cut's base, and a flaring cut has already eaten the black
        // there, so the sharp interpolation is back and with it the
        // sliver starts that used to sit blunt out in the black. Without
        // a flare it is still three quarters of a pitch — the arc a base
        // cut spans (see `core_lut`).
        let inner = core_lut(
            &teeth,
            &angs,
            (4.0 * step / std::f32::consts::TAU * CORE_LUT as f32).ceil() as usize,
        );
        // A swept flash inks only its own arc: a burst aimed from outside
        // the panel would otherwise black the whole page on the way past,
        // which is the solid kind's version of the "ruled vector
        // starburst" the focus kind's sweep fixed in round 2.
        let arc = (p.sweep_deg > 0.0 && p.sweep_deg < 360.0).then(|| {
            (
                p.sweep_center_deg.to_radians(),
                p.sweep_deg.to_radians() * 0.5,
            )
        });
        let c = p.center;
        // How far the BLACK goes, which since Lane F is not how far the
        // teeth go — see `UrchinParams::field`.
        let field_r = if p.field > 0.0 {
            r_out * p.field.max(1.0)
        } else {
            r_out
        };
        let x0 = (c[0] - field_r - 1.0).max(0.0);
        let x1 = (c[0] + field_r + 1.0).min(size.0 as f32);
        let y0 = (c[1] - field_r - 1.0).max(0.0);
        let y1 = (c[1] + field_r + 1.0).min(size.1 as f32);
        // ONE FIELD, LITERALLY. A ベタフラ's black is a single mass with a
        // burst punched through it (REFS target 5, ref-20/21), and the
        // only way a hard-edged raster can end a black wedge in a point
        // is to print its last sub-pixel stretch as a dot or two. Both
        // Builder B (355 of them) and this round (down to 12 by geometry
        // alone: a flare that closes steeply, a boundary pushed two
        // pitches clear, a floor on the black thread) fought that with
        // shape, and shape can only trade it against the core's own
        // raggedness — measured, `core_jit` 0.42 → 0.20 halves the specks
        // and takes white σ from 18.8 % to 14.5 %, which is the wrong
        // trade.
        //
        // So the rule is stated instead of approximated: on a flash whose
        // black is a FIELD, keep only the black connected to it. Seeded
        // from past `r_out`, where there are no teeth and the field is
        // solid by construction. Gated on `field`, so a saved flash — no
        // field, black that stops with the teeth — is untouched.
        let despeckle = p.field > 1.0 && p.width_frac > 0.0;
        let (rw, rh) = (
            (x1.ceil() - x0.floor()) as usize + 2,
            (y1.ceil() - y0.floor()) as usize + 2,
        );
        let (ox, oy) = (x0.floor() as i32, y0.floor() as i32);
        let mut hit_map = if despeckle {
            vec![false; rw * rh]
        } else {
            Vec::new()
        };
        let mut seeds: Vec<u32> = Vec::new();
        if x0 < x1 && y0 < y1 {
            let ro2 = field_r * field_r;
            let solid_r2 = (r_out + 1.0) * (r_out + 1.0);
            let ni = teeth.len() as isize;
            let wi = win as isize;
            for y in y0.floor() as i32..=y1.ceil() as i32 {
                for x in x0.floor() as i32..=x1.ceil() as i32 {
                    let dx = x as f32 + 0.5 - c[0];
                    let dy = y as f32 + 0.5 - c[1];
                    let r2 = dx * dx + dy * dy;
                    if r2 > ro2 {
                        continue;
                    }
                    let a = dy.atan2(dx).rem_euclid(std::f32::consts::TAU);
                    if let Some((mid, half)) = arc {
                        let d = (a - mid + std::f32::consts::PI)
                            .rem_euclid(std::f32::consts::TAU)
                            - std::f32::consts::PI;
                        if d.abs() > half {
                            continue;
                        }
                    }
                    let bin = (a / std::f32::consts::TAU * CORE_LUT as f32) as usize % CORE_LUT;
                    let ri = inner[bin];
                    if r2 < ri * ri {
                        continue;
                    }
                    let idx = angs.partition_point(|v| *v < a) as isize;
                    let cut = (-wi..=wi)
                        .any(|o| teeth[(idx + o).rem_euclid(ni) as usize].hit(dx, dy));
                    if cut {
                        continue;
                    }
                    if !despeckle {
                        put(&mut map, x, y);
                        continue;
                    }
                    let k = (y - oy) as usize * rw + (x - ox) as usize;
                    hit_map[k] = true;
                    if r2 > solid_r2 {
                        seeds.push(k as u32);
                    }
                }
            }
            if despeckle {
                // Flood the field, then ink only what it reached.
                let mut keep = vec![false; rw * rh];
                let mut stack = seeds;
                for s in &stack {
                    keep[*s as usize] = true;
                }
                while let Some(k) = stack.pop() {
                    let (kx, ky) = ((k as usize) % rw, (k as usize) / rw);
                    for dy in -1i32..=1 {
                        for dx in -1i32..=1 {
                            let (nx, ny) = (kx as i32 + dx, ky as i32 + dy);
                            if nx < 0 || ny < 0 || nx >= rw as i32 || ny >= rh as i32 {
                                continue;
                            }
                            let j = ny as usize * rw + nx as usize;
                            if hit_map[j] && !keep[j] {
                                keep[j] = true;
                                stack.push(j as u32);
                            }
                        }
                    }
                }
                for (k, on) in keep.iter().enumerate() {
                    if *on {
                        put(&mut map, ox + (k % rw) as i32, oy + (k / rw) as i32);
                    }
                }
            }
        }
    } else {
        for t in &teeth {
            fill_tooth(&mut map, p.center, t, size);
        }
    }

    let (wi, hi_) = (size.0 as i32, size.1 as i32);
    map.retain(|idx, _| {
        let (ox, oy) = idx.origin();
        ox < wi && oy < hi_ && ox + TILE_SIZE as i32 > 0 && oy + TILE_SIZE as i32 > 0
    });
    map.into_iter().map(|(k, v)| (k, Arc::new(v))).collect()
}

#[cfg(test)]
mod scan_cost_tests {
    use super::*;

    /// A page big enough that an O(length²) scan cannot hide, small enough
    /// that the fixed version is instant even in a debug build.
    const PAGE: (u32, u32) = (3000, 4000);

    /// The lag hunt's regression guard (2026-09-10). Before [`scan_chunks`]
    /// a default `Stream line` preset took **20 seconds** on a B4 600 dpi
    /// page in RELEASE, because each stroke scanned its axis-aligned box
    /// instead of itself — and this page is a third of B4's area, so the
    /// old code cannot come near the budget even unoptimised.
    ///
    /// The budget is wall-clock, which is normally a flaky thing to assert
    /// on, so it is set with a lot of air: the heaviest preset takes ~5 s
    /// here in a DEBUG build under `--test-threads=2` contention, and the
    /// quadratic scan took **minutes** on the same page in the same build.
    /// A 30 s ceiling is therefore six times the honest cost and a fifth of
    /// the broken one. A tighter number failed on nothing but a busy
    /// machine (observed); a looser one would stop catching the bug. If
    /// this fails, the scan went quadratic again — do not raise it further,
    /// find the box that grew.
    ///
    /// `LineKind::Solid` (ベタフラ) is excluded on purpose: it draws no
    /// strokes at all — it scans a filled disc and punches the teeth out of
    /// it — so its cost is the AREA of the black it prints, which is honest
    /// work and always was. It is the one preset still slow enough to be
    /// felt on a B4 page; see the Lane A report's "not fixed" section.
    #[test]
    fn every_preset_renders_a_full_page_in_well_under_a_second_of_work() {
        let (w, h) = (PAGE.0 as f32, PAGE.1 as f32);
        for (i, p) in builtin_presets().iter().enumerate() {
            if p.kind == LineKind::Solid {
                continue;
            }
            let opts = (p.opts)(600);
            let a = [w * 0.5, h * 0.5];
            let b = [a[0] + w / 6.0, a[1]];
            let spec = opts.place(p.kind, a, b, [0.0, 0.0, w, h], 0x51ED_5EED ^ i as u64);
            let t = std::time::Instant::now();
            let map = spec.render(PAGE);
            let ms = t.elapsed().as_secs_f32() * 1000.0;
            assert!(
                !map.is_empty(),
                "{} drew nothing — the bound must not clip the effect away",
                p.name
            );
            assert!(
                ms < 30_000.0,
                "{} took {ms:.0} ms to render {}x{} — the stroke scan went quadratic again",
                p.name,
                PAGE.0,
                PAGE.1,
            );
        }
    }

    /// The other half of the same promise: a SMALL effect on a big page
    /// must touch a small part of it. One case per render kind, because
    /// each builds its own scan boxes.
    #[test]
    fn a_small_effect_touches_a_small_share_of_the_page() {
        // 3000x4000 at 64 px tiles = 47 x 63 = 2,961 tiles.
        let full = 47 * 63;
        let centre = [300.0, 300.0];

        let focus = render_focus(
            &FocusLinesParams {
                center: centre,
                r_in: 40.0,
                r_out: 200.0,
                count: 60,
                width: 3.0,
                ..Default::default()
            },
            PAGE,
        );
        let urchin = render_urchin(
            &UrchinParams {
                center: centre,
                r_in: 40.0,
                r_out: 200.0,
                count: 60,
                width: 3.0,
                width_frac: 0.5,
                ..Default::default()
            },
            PAGE,
        );
        // Speed lines scatter over the whole canvas by design, so the
        // bound that means anything for them is the LENGTH: 60 px runs
        // cover a band, never the page.
        let speed = render_speed(
            &SpeedLinesParams {
                angle_deg: 20.0,
                count: 40,
                len_min: 60.0,
                len_max: 60.0,
                width: 3.0,
                ..Default::default()
            },
            PAGE,
        );

        for (name, n, share) in [
            ("focus", focus.len(), 0.10),
            ("urchin", urchin.len(), 0.10),
            ("speed", speed.len(), 0.35),
        ] {
            assert!(
                n > 0 && (n as f32) < full as f32 * share,
                "{name}: {n} tiles of {full} — a small effect must not allocate the page",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ink_at(map: &HashMap<TileIdx, Arc<Tile>>, x: i32, y: i32) -> bool {
        let idx = TileIdx::of_pixel(x, y);
        map.get(&idx).is_some_and(|t| {
            t.pixel((x - idx.origin().0) as usize, (y - idx.origin().1) as usize)[3] > 0
        })
    }

    /// Focus lines: ink near the outer ring at many angles, none inside
    /// the inner radius, deterministic under a fixed seed.
    #[test]
    fn focus_lines_ring_and_hole() {
        let p = FocusLinesParams {
            center: [256.0, 256.0],
            r_in: 100.0,
            r_out: 240.0,
            count: 64,
            width: 6.0,
            angle_jitter: 0.5,
            width_jitter: 0.5,
            length_jitter: 0.2,
            taper: 0.0,
            seed: 7,
            ..Default::default()
        };
        let m = render_focus(&p, (512, 512));
        // Sectors with ink at r ≈ 200 — each sector samples a short ARC
        // (7 points ±3°) because a single point can fall between two
        // jittered lines.
        let mut sectors = 0;
        for k in 0..32 {
            let base = k as f32 * std::f32::consts::TAU / 32.0;
            let hit = (-3..=3).map(|d| d as f32).any(|d| {
                let a = base + d.to_radians();
                let (s, c) = a.sin_cos();
                ink_at(&m, (256.0 + c * 200.0) as i32, (256.0 + s * 200.0) as i32)
            });
            if hit {
                sectors += 1;
            }
        }
        assert!(sectors >= 26, "most sectors carry ink ({sectors}/32)");
        assert!(!ink_at(&m, 256, 256), "the hole is empty");
        let m2 = render_focus(&p, (512, 512));
        assert_eq!(m.len(), m2.len(), "seeded = deterministic");
    }

    /// The COLOUR pin (owner repro 2026-08-22). Every generator here inked
    /// `[ONE; 4]` — opaque white — from the first commit, so a placed layer
    /// showed nothing on a white page. Every other test in this module reads
    /// channel 3 alone, which is exactly why nothing caught it; this one
    /// reads channels 0..3 for all four generators.
    #[test]
    fn every_generator_inks_black_not_white() {
        let focus = render_focus(
            &FocusLinesParams {
                center: [256.0, 256.0],
                r_in: 100.0,
                r_out: 240.0,
                count: 64,
                width: 6.0,
                angle_jitter: 0.5,
                width_jitter: 0.5,
                length_jitter: 0.2,
                taper: 0.0,
                seed: 7,
                ..Default::default()
            },
            (512, 512),
        );
        let speed = render_speed(
            &SpeedLinesParams {
                angle_deg: 20.0,
                count: 80,
                len_min: 100.0,
                len_max: 300.0,
                width: 4.0,
                taper: 0.0,
                converge: None,
                seed: 3,
                ..Default::default()
            },
            (512, 512),
        );
        let urchin = |solid| {
            render_urchin(
                &UrchinParams {
                    center: [256.0, 256.0],
                    r_in: 80.0,
                    r_out: 240.0,
                    count: 24,
                    width: 18.0,
                    angle_jitter: 0.2,
                    length_jitter: 0.2,
                    core_jit: 0.0,
                    solid,
                    seed: 5,
                    ..Default::default()
                },
                (512, 512),
            )
        };
        for (what, m) in [
            ("focus", &focus),
            ("speed", &speed),
            ("urchin", &urchin(false)),
            ("solid flash", &urchin(true)),
        ] {
            let mut inked = 0u32;
            for t in m.values() {
                for y in 0..TILE_SIZE {
                    for x in 0..TILE_SIZE {
                        let p = t.pixel(x, y);
                        if p[3] == 0 {
                            continue;
                        }
                        inked += 1;
                        assert_eq!(
                            [p[0], p[1], p[2]],
                            [0, 0, 0],
                            "{what}: inked pixel at ({x}, {y}) is not black"
                        );
                    }
                }
            }
            assert!(inked > 0, "{what}: drew nothing to check");
        }
    }

    /// A position-sensitive fingerprint of a rendered layer: tile count,
    /// inked pixels, and a checksum that moves if a single pixel moves.
    pub(super) fn fingerprint(m: &HashMap<TileIdx, Arc<Tile>>) -> (usize, u64, u64) {
        let mut keys: Vec<_> = m.keys().copied().collect();
        keys.sort_by_key(|i| (i.y, i.x));
        let (mut px, mut sum) = (0u64, 0u64);
        for k in keys {
            let (ox, oy) = k.origin();
            for y in 0..TILE_SIZE {
                for x in 0..TILE_SIZE {
                    if m[&k].pixel(x, y)[3] > 0 {
                        px += 1;
                        let gx = (ox + x as i32) as u64;
                        let gy = (oy + y as i32) as u64;
                        sum = sum
                            .wrapping_mul(0x0100_0000_01B3)
                            .wrapping_add(gx.wrapping_mul(65_537).wrapping_add(gy));
                    }
                }
            }
        }
        (m.len(), px, sum)
    }

    /// BIT-STABILITY PIN (flash round, 2026-08-22): the two original
    /// renderers must keep drawing exactly what they drew before `kind`,
    /// `taper` and `converge` existed — every effect-line layer in every
    /// saved file regenerates through them. The numbers were taken from
    /// the pre-round code; a change that moves them has silently redrawn
    /// the owner's archive.
    #[test]
    fn legacy_renders_are_bit_stable() {
        let f = FocusLinesParams {
            center: [256.0, 256.0],
            r_in: 100.0,
            r_out: 240.0,
            count: 64,
            width: 6.0,
            angle_jitter: 0.5,
            width_jitter: 0.5,
            length_jitter: 0.2,
            taper: 0.0,
            seed: 7,
            ..Default::default()
        };
        assert_eq!(
            fingerprint(&render_focus(&f, (512, 512))),
            (52, 37446, 14_909_681_065_247_512_801)
        );
        let s = SpeedLinesParams {
            angle_deg: 20.0,
            count: 80,
            len_min: 100.0,
            len_max: 300.0,
            width: 4.0,
            taper: 0.0,
            converge: None,
            seed: 3,
            ..Default::default()
        };
        assert_eq!(
            fingerprint(&render_speed(&s, (512, 512))),
            (57, 25119, 6_096_450_357_538_070_854)
        );

        // Density round, 2026-08-23: the legacy speed set THROUGH THE
        // SPEC, with every new attribute at its serde default. `gap_px` 0
        // must still mean the uniform scatter, the split jitters 0 the
        // single `jitter`, `gap_deg` 0 "use `count`" and `color` black —
        // so a saved layer regenerates onto the pixels it was saved with.
        // (The focus half is pinned the same way by
        // `pre_flash_specs_load_with_the_old_meaning`, which compares the
        // spec's raster to explicit params rather than to a constant.)
        assert_eq!(
            fingerprint(
                &GenLinesSpec {
                    focus: false,
                    a: 20.0,
                    b: 100.0,
                    c: 300.0,
                    count: 80,
                    width: 4.0,
                    seed: 3,
                    ..Default::default()
                }
                .render((512, 512))
            ),
            (57, 25119, 6_096_450_357_538_070_854)
        );
    }

    /// `gap_deg` is the same fan expressed in CSP's unit: 360/gap rays,
    /// and setting it to the gap the count already implied draws the same
    /// set. 0 keeps the count (pinned above).
    #[test]
    fn gap_deg_derives_the_ray_count() {
        let by_count = GenLinesSpec {
            focus: true,
            a: 256.0,
            b: 256.0,
            c: 100.0,
            d: 240.0,
            count: 90,
            width: 6.0,
            jitter: 0.3,
            seed: 7,
            ..Default::default()
        };
        assert_eq!(by_count.ray_count(), 90);
        let by_gap = GenLinesSpec {
            count: 1,
            gap_deg: 4.0,
            ..by_count
        };
        assert_eq!(by_gap.ray_count(), 90, "360 / 4°");
        assert_eq!(
            fingerprint(&by_count.render((512, 512))),
            fingerprint(&by_gap.render((512, 512))),
            "the same fan, said the other way round"
        );
        // A silly gap is capped rather than allowed to hang the UI.
        assert_eq!(
            GenLinesSpec {
                gap_deg: 0.01,
                ..by_count
            }
            .ray_count(),
            4096
        );
    }

    /// Speed lines: horizontal runs at many heights (0° set).
    #[test]
    fn speed_lines_parallel_bands() {
        let p = SpeedLinesParams {
            angle_deg: 0.0,
            count: 80,
            len_min: 100.0,
            len_max: 300.0,
            width: 4.0,
            taper: 0.0,
            converge: None,
            seed: 3,
            ..Default::default()
        };
        let m = render_speed(&p, (512, 512));
        let mut rows = 0;
        for y in (0..512).step_by(2) {
            if ink_at(&m, 256, y) {
                rows += 1;
            }
        }
        // ~30% of runs cross x=256 at these lengths; each covers ~2 of
        // the 2-px samples → tens of hits.
        assert!(rows >= 20, "many horizontal bands ({rows})");
        // And the spread reaches BOTH halves of the canvas.
        let top = (0..256).step_by(2).any(|y| ink_at(&m, 256, y));
        let bot = (256..512).step_by(2).any(|y| ink_at(&m, 256, y));
        assert!(top && bot, "runs scatter over the full normal extent");
    }

    /// Taper thins a run toward its TAIL: sampled across the same run,
    /// the head still inks at the full half-width and the tail no longer
    /// does. 0 changes nothing (pinned separately by the bit-stability
    /// fingerprint).
    #[test]
    fn focus_lines_taper_needles_at_the_centre() {
        // Jitters off so ray 0 sits exactly on angle 0 (the +x axis): the
        // ray's cross-section near r_in must be materially thinner than
        // near r_out under taper, and identical without it.
        let p = |taper: f32| FocusLinesParams {
            center: [256.0, 256.0],
            r_in: 40.0,
            r_out: 220.0,
            count: 8,
            width: 12.0,
            angle_jitter: 0.0,
            width_jitter: 0.0,
            length_jitter: 0.0,
            taper,
            seed: 7,
            ..Default::default()
        };
        let cross = |m: &HashMap<TileIdx, Arc<Tile>>, x: i32| {
            (0..40).filter(|dy| ink_at(m, x, 256 - 20 + dy)).count() as i32
        };
        let flat = render_focus(&p(0.0), (512, 512));
        assert_eq!(
            cross(&flat, 256 + 50),
            cross(&flat, 256 + 210),
            "taper 0: constant width end to end"
        );
        let tapered = render_focus(&p(0.9), (512, 512));
        let inner = cross(&tapered, 256 + 50);
        let outer = cross(&tapered, 256 + 210);
        assert!(
            inner * 2 < outer && outer >= 10,
            "needles at the convergence, weight at the rim ({inner} vs {outer})"
        );
    }

    #[test]
    fn speed_lines_taper_thins_the_tail() {
        // The ramp itself, measured on one PLACED run so no scatter is in
        // the way: how tall is the run's column at the head, and at the
        // far end?
        let col = |taper: f32, x: i32| {
            let mut m: HashMap<TileIdx, Tile> = HashMap::new();
            segment(
                &mut m,
                [50.0, 256.0],
                [450.0, 256.0],
                8.0,
                Profile::taper(taper),
                (512, 512),
            );
            (0..512)
                .filter(|y| {
                    let idx = TileIdx::of_pixel(x, *y);
                    m.get(&idx).is_some_and(|t| {
                        let (ox, oy) = idx.origin();
                        t.pixel((x - ox) as usize, (*y - oy) as usize)[3] > 0
                    })
                })
                .count()
        };
        assert_eq!(col(0.0, 60), col(0.0, 400), "0 = constant width, as before");
        assert_eq!(col(1.0, 60), col(0.0, 60), "the head keeps its width");
        let tail = col(1.0, 400);
        assert!(
            tail * 3 < col(1.0, 60),
            "the tail thinned to a needle ({tail})"
        );
        assert!(tail >= 1, "but the run still reaches its end");

        // And the parameter is plumbed through the generator.
        let p = SpeedLinesParams {
            angle_deg: 0.0,
            count: 40,
            len_min: 300.0,
            len_max: 300.0,
            width: 10.0,
            taper: 0.0,
            converge: None,
            seed: 5,
            ..Default::default()
        };
        let flat = fingerprint(&render_speed(&p, (512, 512)));
        let tapered = fingerprint(&render_speed(
            &SpeedLinesParams {
                taper: 0.8,
                ..p.clone()
            },
            (512, 512),
        ));
        assert!(
            flat.1 > 0 && tapered.1 * 4 < flat.1 * 3,
            "less ink on the page"
        );
    }

    /// Convergence leans the runs at a point instead of leaving them
    /// parallel: with the vanishing point straight above the canvas the
    /// block fans, so the runs no longer share one direction.
    #[test]
    fn speed_lines_converge_on_a_point() {
        let p = SpeedLinesParams {
            angle_deg: 0.0,
            count: 24,
            len_min: 200.0,
            len_max: 200.0,
            width: 3.0,
            taper: 0.0,
            converge: Some([256.0, -4000.0]),
            seed: 9,
            ..Default::default()
        };
        let m = render_speed(&p, (512, 512));
        assert!(!m.is_empty(), "the fan landed on the canvas");
        let par = render_speed(
            &SpeedLinesParams {
                converge: None,
                ..p.clone()
            },
            (512, 512),
        );
        assert_ne!(
            fingerprint(&m),
            fingerprint(&par),
            "aiming at the point moved the runs"
        );
    }

    /// Sea-urchin flash: filled spikes, wide at the rim and pointed at
    /// the hole — so a ring of probes just inside `r_out` finds far more
    /// ink than the same ring just outside `r_in`, and the hole is empty.
    #[test]
    fn urchin_flash_spikes_are_filled_wedges() {
        let p = UrchinParams {
            center: [256.0, 256.0],
            r_in: 60.0,
            r_out: 240.0,
            count: 32,
            width: 26.0,
            angle_jitter: 0.2,
            length_jitter: 0.1,
            core_jit: 0.0,
            solid: false,
            seed: 11,
            ..Default::default()
        };
        let m = render_urchin(&p, (512, 512));
        let ring = |r: f32| {
            (0..720)
                .filter(|k| {
                    let a = *k as f32 * std::f32::consts::TAU / 720.0;
                    let (s, c) = a.sin_cos();
                    ink_at(&m, (256.0 + c * r) as i32, (256.0 + s * r) as i32)
                })
                .count()
        };
        let rim = ring(230.0);
        let near = ring(70.0);
        assert!(rim > 150, "the rim is mostly ink ({rim}/720)");
        assert!(near * 3 < rim, "and the points are thin ({near} vs {rim})");
        assert!(!ink_at(&m, 256, 256), "the hole stays empty");
        // A wedge is FILLED, not an outline: walk a rim spoke inward and
        // it stays inked for a long unbroken run.
        let mut best = 0;
        let mut run = 0;
        for k in 0..720 {
            let a = k as f32 * std::f32::consts::TAU / 720.0;
            let (s, c) = a.sin_cos();
            run = if ink_at(&m, (256.0 + c * 230.0) as i32, (256.0 + s * 230.0) as i32) {
                run + 1
            } else {
                0
            };
            best = best.max(run);
        }
        assert!(best >= 8, "a spike is a solid band at the rim ({best})");
        assert_eq!(
            fingerprint(&m),
            fingerprint(&render_urchin(&p, (512, 512))),
            "seeded = deterministic"
        );
    }

    /// Solid flash is the SAME teeth cut out of a solid ring: the hole
    /// is still empty, the ring is mostly ink where the urchin is mostly
    /// gaps, and the two are complementary inside the annulus.
    #[test]
    fn solid_flash_inverts_the_ring() {
        let mut p = UrchinParams {
            center: [256.0, 256.0],
            r_in: 60.0,
            r_out: 240.0,
            count: 32,
            width: 26.0,
            angle_jitter: 0.2,
            length_jitter: 0.0,
            core_jit: 0.0,
            solid: false,
            seed: 11,
            ..Default::default()
        };
        let spikes = render_urchin(&p, (512, 512));
        p.solid = true;
        let solid = render_urchin(&p, (512, 512));
        assert!(!ink_at(&solid, 256, 256), "the hole stays empty");
        // Just inside the hole's edge the solid variant is unbroken ink
        // (the teeth are needle-thin there).
        let ring = |m: &HashMap<TileIdx, Arc<Tile>>, r: f32| {
            (0..720)
                .filter(|k| {
                    let a = *k as f32 * std::f32::consts::TAU / 720.0;
                    let (s, c) = a.sin_cos();
                    ink_at(m, (256.0 + c * r) as i32, (256.0 + s * r) as i32)
                })
                .count()
        };
        // Not 720/720: the teeth are still ~1 px wide this close to the
        // apex, so they nick a couple of probes each.
        assert!(ring(&solid, 70.0) > 600, "solid at the hole's edge");
        assert!(
            ring(&solid, 70.0) > ring(&spikes, 70.0) * 8,
            "and the polarity really is the other way round"
        );
        assert!(
            ring(&solid, 230.0) < ring(&spikes, 230.0),
            "gaps at the rim"
        );
        // Complementary: no pixel of the annulus carries ink in both.
        let mut both = 0;
        for k in 0..2000 {
            let a = k as f32 * 0.031;
            let r = 65.0 + (k % 170) as f32;
            let (x, y) = ((256.0 + a.cos() * r) as i32, (256.0 + a.sin() * r) as i32);
            if ink_at(&spikes, x, y) && ink_at(&solid, x, y) {
                both += 1;
            }
        }
        // Edge pixels of a tooth can round into both scans; a handful is
        // the rasterizer's seam, a flood would mean the polarity is off.
        assert!(both < 40, "the two polarities barely overlap ({both})");
    }

    /// The rows a set of horizontal runs occupies at one column, as the
    /// gaps between consecutive bands. The measure the density round is
    /// actually about: a hand-ruled 流線 block has ONE gap repeated, the
    /// old uniform-random scatter has gaps from 1 px to a bald strip.
    fn band_gaps(m: &HashMap<TileIdx, Arc<Tile>>, w: i32, h: i32) -> Vec<i32> {
        let mut centres = Vec::new();
        let mut run: Option<(i32, i32)> = None;
        for y in 0..h {
            if (0..w).any(|x| ink_at(m, x, y)) {
                run = Some(match run {
                    Some((a, _)) => (a, y),
                    None => (y, y),
                });
            } else if let Some((a, b)) = run.take() {
                centres.push((a + b) / 2);
            }
        }
        if let Some((a, b)) = run {
            centres.push((a + b) / 2);
        }
        centres.windows(2).map(|w| w[1] - w[0]).collect()
    }

    /// `gap_px` walks the normal extent instead of scattering, so the
    /// spacing is EVEN — the clumping the owner called "dogshit".
    ///
    /// Short runs on purpose: the along-the-direction start offset can
    /// still drop a run clean off the canvas (that is the legacy scatter
    /// and stays), and its odds fall with the run length, so a stray
    /// double gap does not have to be tolerated by a loose bound.
    #[test]
    fn speed_lines_gap_spacing_is_even() {
        let p = SpeedLinesParams {
            angle_deg: 0.0,
            count: 0,
            len_min: 20.0,
            len_max: 20.0,
            width: 2.0,
            gap_px: 16.0,
            seed: 5,
            ..Default::default()
        };
        let m = render_speed(&p, (512, 512));
        let gaps = band_gaps(&m, 512, 512);
        assert!(
            gaps.len() > 20,
            "the walk filled the extent ({})",
            gaps.len()
        );
        let lo = *gaps.iter().min().unwrap();
        let hi = *gaps.iter().max().unwrap();
        let nominal = gaps.iter().filter(|g| (**g - 16).abs() <= 1).count();
        assert!(
            hi <= lo * 2 + 2 && nominal * 10 >= gaps.len() * 9,
            "one gap, repeated ({lo}..{hi}, {nominal}/{} at 16)",
            gaps.len()
        );

        // And the same number of runs through the SCATTER path clumps —
        // this is the before picture, and it is why the field exists.
        let sg = band_gaps(
            &render_speed(
                &SpeedLinesParams {
                    count: gaps.len() as u32 + 1,
                    gap_px: 0.0,
                    ..p.clone()
                },
                (512, 512),
            ),
            512,
            512,
        );
        let s_hi = *sg.iter().max().unwrap();
        let s_lo = *sg.iter().min().unwrap();
        assert!(
            s_hi > s_lo * 4,
            "the old scatter really is uneven ({s_lo}..{s_hi})"
        );
    }

    /// まとまり: bundles of `group` runs with a hole between them — so the
    /// gap histogram has two values, not one, and the hole is the bigger.
    #[test]
    fn speed_lines_grouping_leaves_holes() {
        let m = render_speed(
            &SpeedLinesParams {
                angle_deg: 0.0,
                count: 0,
                len_min: 20.0,
                len_max: 20.0,
                width: 2.0,
                gap_px: 12.0,
                group: 3,
                group_gap: 3.0,
                seed: 5,
                ..Default::default()
            },
            (512, 512),
        );
        let gaps = band_gaps(&m, 512, 512);
        assert!(gaps.len() > 10, "enough bands to see the pattern");
        let tight = *gaps.iter().min().unwrap();
        assert!((tight - 12).abs() <= 1, "the bundle's own gap ({tight})");
        // The hole is 3 × the gap, and there are two tight gaps (a bundle
        // of three) for every one of them. A run that the along-the-
        // direction offset dropped off the canvas merges two neighbours,
        // so the counts are compared loosely — the SHAPE is the claim.
        let holes = gaps.iter().filter(|g| (**g - 36).abs() <= 2).count();
        let tights = gaps.iter().filter(|g| (**g - 12).abs() <= 1).count();
        assert!(holes >= 5, "bundles stand apart ({gaps:?})");
        assert!(
            tights * 2 >= holes * 3,
            "and each bundle is three tight runs ({tights} tight, {holes} holes)"
        );
    }

    /// The colour field: a white run on a black page is the knockout the
    /// black-only generator could not draw. Alpha is untouched.
    #[test]
    fn spec_color_paints_the_ink() {
        let mut spec = GenLinesSpec {
            focus: true,
            a: 256.0,
            b: 256.0,
            c: 60.0,
            d: 240.0,
            count: 32,
            width: 6.0,
            jitter: 0.2,
            seed: 7,
            ..Default::default()
        };
        let black = spec.render((512, 512));
        spec.color = [255, 255, 255];
        let white = spec.render((512, 512));
        assert_eq!(
            fingerprint(&black),
            fingerprint(&white),
            "colour moves no pixel"
        );
        let mut seen = 0;
        for t in white.values() {
            for y in 0..TILE_SIZE {
                for x in 0..TILE_SIZE {
                    let p = t.pixel(x, y);
                    if p[3] > 0 {
                        seen += 1;
                        assert_eq!([p[0], p[1], p[2]], [FIX15_ONE as u16; 3], "white ink");
                    }
                }
            }
        }
        assert!(seen > 0, "something was inked to check");
    }

    /// A degenerate flash (radius smaller than one spike) must not panic:
    /// the half-width cap is floored before `clamp`, which PANICS on
    /// min > max and would abort through wndproc (audit B).
    #[test]
    fn tiny_flash_does_not_panic() {
        for solid in [false, true] {
            let p = UrchinParams {
                center: [10.0, 10.0],
                r_in: 0.0,
                r_out: 0.2,
                count: 512,
                width: 40.0,
                angle_jitter: 1.0,
                length_jitter: 1.0,
                core_jit: 0.0,
                solid,
                seed: 1,
                ..Default::default()
            };
            let _ = render_urchin(&p, (64, 64));
        }
    }

    // --- parity round, 2026-09-06 --------------------------------------
    // Everything below measures ONE of the five gaps the reference pages
    // showed up (weight mix, stroke profile, skewed lengths, angular
    // bundling, runs that hang off a line). None of them re-pins a
    // fingerprint: the pins above are the guard that the zero value of
    // every knob here is still the old raster.

    /// The angular positions of a radial set's rays at radius `r`, in
    /// degrees, as the gaps between consecutive inked arcs. The angular
    /// twin of [`band_gaps`], and the same claim: a bundled set has TWO
    /// gap values, a tight one and a hole.
    fn ray_gaps(m: &HashMap<TileIdx, Arc<Tile>>, c: [f32; 2], r: f32) -> Vec<f32> {
        const N: usize = 3600;
        let hit: Vec<bool> = (0..N)
            .map(|k| {
                let a = k as f32 * std::f32::consts::TAU / N as f32;
                let (s, cs) = a.sin_cos();
                ink_at(m, (c[0] + cs * r) as i32, (c[1] + s * r) as i32)
            })
            .collect();
        let mut centres = Vec::new();
        let mut run: Option<(usize, usize)> = None;
        for (k, on) in hit.iter().enumerate() {
            if *on {
                run = Some(match run {
                    Some((a, _)) => (a, k),
                    None => (k, k),
                });
            } else if let Some((a, b)) = run.take() {
                centres.push((a + b) as f32 * 0.5);
            }
        }
        if let Some((a, b)) = run {
            centres.push((a + b) as f32 * 0.5);
        }
        centres
            .windows(2)
            .map(|w| (w[1] - w[0]) * 360.0 / N as f32)
            .collect()
    }

    /// まとまり in ANGLE space — the owner's "doesn't seem to have a
    /// grouping setting". Bundles of `group` rays a `gap_deg` apart, then
    /// a hole: the gap histogram has two values, and the hole is the
    /// bigger. The mirror of `speed_lines_grouping_leaves_holes`.
    #[test]
    fn radial_grouping_leaves_angular_holes() {
        let m = render_focus(
            &FocusLinesParams {
                center: [512.0, 512.0],
                r_in: 100.0,
                r_out: 500.0,
                count: 72,
                width: 2.0,
                gap_deg: 3.0,
                group: 3,
                group_gap: 3.0,
                seed: 5,
                ..Default::default()
            },
            (1024, 1024),
        );
        let gaps = ray_gaps(&m, [512.0, 512.0], 300.0);
        assert!(gaps.len() > 30, "enough rays to see the pattern");
        let tight = gaps.iter().filter(|g| (**g - 3.0).abs() < 0.6).count();
        let holes = gaps.iter().filter(|g| (**g - 9.0).abs() < 1.0).count();
        assert!(holes >= 15, "bundles stand apart ({holes} holes in {gaps:?})");
        assert!(
            tight >= holes,
            "and each bundle is several tight rays ({tight} tight, {holes} holes)"
        );

        // The same rays WITHOUT bundling: one gap, repeated — the even
        // pitch that reads as machine-made.
        let even = render_focus(
            &FocusLinesParams {
                center: [512.0, 512.0],
                r_in: 100.0,
                r_out: 500.0,
                count: 72,
                width: 2.0,
                seed: 5,
                ..Default::default()
            },
            (1024, 1024),
        );
        let eg = ray_gaps(&even, [512.0, 512.0], 300.0);
        let hi = eg.iter().copied().fold(0.0f32, f32::max);
        let lo = eg.iter().copied().fold(f32::INFINITY, f32::min);
        assert!(hi - lo < 0.5, "the unbundled set is dead even ({lo}..{hi})");
    }

    /// ref-11's left panel: a burst whose centre is off the page fills a
    /// FAN. `sweep_deg` is the only way to say that — clipping a full
    /// circle keeps the rays pointing the wrong way.
    #[test]
    fn sweep_limits_the_arc() {
        let p = FocusLinesParams {
            center: [512.0, 512.0],
            r_in: 60.0,
            r_out: 480.0,
            count: 120,
            width: 3.0,
            sweep_deg: 90.0,
            sweep_center_deg: 0.0,
            seed: 9,
            ..Default::default()
        };
        let m = render_focus(&p, (1024, 1024));
        let ring = |lo: i32, hi: i32| {
            (lo..hi)
                .filter(|d| {
                    let a = (*d as f32).to_radians();
                    let (s, c) = a.sin_cos();
                    ink_at(&m, (512.0 + c * 300.0) as i32, (512.0 + s * 300.0) as i32)
                })
                .count()
        };
        assert!(ring(-40, 40) > 20, "the arc carries the rays");
        assert_eq!(ring(60, 300), 0, "and nothing outside it");
        // Full circle for comparison: the same set answers everywhere.
        let full = render_focus(
            &FocusLinesParams {
                sweep_deg: 0.0,
                ..p.clone()
            },
            (1024, 1024),
        );
        assert!(
            (60..300).any(|d| {
                let a = (d as f32).to_radians();
                let (s, c) = a.sin_cos();
                ink_at(&full, (512.0 + c * 300.0) as i32, (512.0 + s * 300.0) as i32)
            }),
            "sweep 0 is still the whole ring"
        );
    }

    /// A sweep with a GAP has to walk. It used to fall through to the
    /// even-spread branch, where `count` is whatever `ray_count()` says —
    /// 360/gap for a gap-driven set — so a 3° fan swept to half a circle
    /// drew all 120 rays inside 180°, at double density. 180° at 3° is
    /// 60 rays, and the ring still measures 3° between them.
    #[test]
    fn sweep_keeps_the_gap() {
        let full = GenLinesSpec {
            focus: true,
            a: 512.0,
            b: 512.0,
            c: 80.0,
            d: 500.0,
            count: 1,
            width: 3.0,
            gap_deg: 3.0,
            seed: 11,
            ..Default::default()
        };
        assert_eq!(full.ray_count(), 120, "360 / 3°");
        let swept = GenLinesSpec {
            sweep_deg: 180.0,
            ..full
        };
        // 60 steps of 3° across 180°, ±1 for whether the ray that lands
        // ON the far edge clears the float compare — not 120.
        let n = swept.ray_count();
        assert!((59..=61).contains(&n), "180 / 3°, not 120 squeezed in ({n})");
        let gaps = ray_gaps(&swept.render((1024, 1024)), [512.0, 512.0], 300.0);
        let tight = gaps.iter().filter(|g| (**g - 3.0).abs() < 0.6).count();
        let doubled = gaps.iter().filter(|g| (**g - 1.5).abs() < 0.4).count();
        assert!(tight > 40, "the arc keeps its pitch ({tight} of {gaps:?})");
        assert_eq!(doubled, 0, "and nothing sits at half the gap");
    }

    /// The thicknesses of a horizontal run block, band by band, sampled
    /// down three columns — how the weight MIX is measured.
    ///
    /// Bands that touch the top or bottom edge are DROPPED: half a
    /// heavy stroke measures the same as a whole hairline, which is
    /// exactly the reading that would make this test lie.
    fn band_widths(m: &HashMap<TileIdx, Arc<Tile>>, h: i32) -> Vec<i32> {
        let mut out = Vec::new();
        for x in [128, 256, 384] {
            let mut start: Option<i32> = None;
            for y in 0..h {
                if ink_at(m, x, y) {
                    if start.is_none() {
                        start = Some(y);
                    }
                } else if let Some(s) = start.take() {
                    if s > 0 {
                        out.push(y - s);
                    }
                }
            }
        }
        out
    }

    /// The weight MIX, the biggest single gap against a printed page:
    /// many hairlines and a FEW heavy strokes in one set. The old width
    /// jitter could only THIN a line, so every set had one weight.
    #[test]
    fn accents_are_wider_than_the_rest() {
        let base = SpeedLinesParams {
            angle_deg: 0.0,
            len_min: 400.0,
            len_max: 400.0,
            width: 4.0,
            gap_px: 40.0,
            seed: 5,
            ..Default::default()
        };
        let plain = render_speed(&base, (512, 512));
        let pw = band_widths(&plain, 512);
        assert!(pw.len() > 5, "runs to measure ({pw:?})");
        assert!(
            pw.iter().max() == pw.iter().min(),
            "without accents there is ONE weight ({pw:?})"
        );
        let thin = pw[0];

        let mixed = render_speed(
            &SpeedLinesParams {
                mix: Mix {
                    accent_frac: 0.3,
                    accent_mul: 4.0,
                    ..Default::default()
                },
                ..base.clone()
            },
            (512, 512),
        );
        let mw = band_widths(&mixed, 512);
        let heavy = mw.iter().filter(|w| **w >= thin * 3).count();
        let hair = mw.iter().filter(|w| **w <= thin).count();
        assert!(heavy >= 1, "a few strokes carry real weight ({mw:?})");
        assert!(hair > heavy, "and most of them are still hairlines ({mw:?})");
    }

    /// 入り: with an entry ramp the stroke starts as a POINT and swells,
    /// so the block is spindles rather than round-capped bars — ref-07's
    /// streaks. `entry` 0 keeps the cap (pinned by the fingerprints).
    #[test]
    fn entry_taper_starts_at_a_point() {
        let col = |prof: Profile, x: i32| {
            let mut m: HashMap<TileIdx, Tile> = HashMap::new();
            segment(&mut m, [50.0, 256.0], [450.0, 256.0], 8.0, prof, (512, 512));
            (0..512)
                .filter(|y| {
                    let idx = TileIdx::of_pixel(x, *y);
                    m.get(&idx).is_some_and(|t| {
                        let (ox, oy) = idx.origin();
                        t.pixel((x - ox) as usize, (*y - oy) as usize)[3] > 0
                    })
                })
                .count()
        };
        let capped = Profile::taper(0.0);
        let spindle = Profile {
            taper: 1.0,
            entry: 0.25,
            needle: 1.0,
        };
        assert!(col(capped, 52) >= 14, "a round cap is full width at once");
        assert!(
            col(spindle, 52) <= 3,
            "the spindle enters as a point ({})",
            col(spindle, 52)
        );
        let belly = col(spindle, 150);
        assert!(
            belly >= col(spindle, 52) * 3 && belly >= 8,
            "and swells to a belly ({belly})"
        );
    }

    /// The needle exponent: a wedge that thins fast and then runs a long
    /// thin point (ref-08's rays) instead of the straight ramp.
    #[test]
    fn needle_exponent_thins_faster() {
        let col = |needle: f32, x: i32| {
            let mut m: HashMap<TileIdx, Tile> = HashMap::new();
            segment(
                &mut m,
                [50.0, 256.0],
                [450.0, 256.0],
                8.0,
                Profile {
                    taper: 0.8,
                    entry: 0.0,
                    needle,
                },
                (512, 512),
            );
            (0..512)
                .filter(|y| {
                    let idx = TileIdx::of_pixel(x, *y);
                    m.get(&idx).is_some_and(|t| {
                        let (ox, oy) = idx.origin();
                        t.pixel((x - ox) as usize, (*y - oy) as usize)[3] > 0
                    })
                })
                .count()
        };
        assert_eq!(col(0.0, 52), col(2.5, 52), "both start at the full width");
        let straight = col(0.0, 350);
        let needled = col(2.5, 350);
        assert!(
            needled * 2 < straight,
            "the needle is well past the wedge by three quarters ({needled} vs {straight})"
        );
        // And the exponent below 1 keeps a belly — the other direction.
        assert!(col(0.5, 350) > straight, "k < 1 holds its weight longer");
    }

    /// A pen needle stays a 1 px LINE until it ends. Before the
    /// half-pixel floor in `segment`, `taper 1` + `needle 1.2` drove the
    /// ramped half-width under ~0.35 px over the last sixth of the
    /// stroke, and a hard-edged distance test then caught only the odd
    /// pixel centre — the tail printed as a dashed line (it was visible
    /// in `saturated-line-crop.png`). One long ray, walked base to tip:
    /// no hole longer than 1 px anywhere before the tip.
    #[test]
    fn needles_end_solid_not_dashed() {
        let m = render_focus(
            &FocusLinesParams {
                center: [520.0, 512.0],
                r_in: 40.0,
                r_out: 480.0,
                count: 1,
                width: 6.0,
                taper: 1.0,
                mix: Mix {
                    needle: 1.2,
                    ..Mix::default()
                },
                seed: 3,
                ..Default::default()
            },
            (1024, 1024),
        );
        // Ray 0 runs along +x from the centre, so the axis is y = 512.0
        // and the stroke spans x = 560 (tip, the inner end taper aims at)
        // to x = 1000 (base).
        let on: Vec<bool> = (560..=1000)
            .map(|x| ink_at(&m, x, 511) || ink_at(&m, x, 512))
            .collect();
        let first = on.iter().position(|v| *v).expect("the ray drew");
        let last = on.iter().rposition(|v| *v).unwrap();
        let (mut gap, mut worst) = (0usize, 0usize);
        for v in &on[first..=last] {
            gap = if *v { 0 } else { gap + 1 };
            worst = worst.max(gap);
        }
        assert!(worst <= 1, "the tail is solid, not dashed ({worst} px hole)");
        assert!(
            first <= 2 && last >= on.len() - 3,
            "and it runs the whole ray ({first}..{last} of {})",
            on.len()
        );
    }

    /// A heavy ray that stops INSIDE the panel must exit as a spindle
    /// point, not as a half-disc. `segment` caps a stroke by distance to
    /// its endpoint, so with no `entry` a 12 px-wide accent whose outer
    /// end landed mid-field printed a 12 px semicircular blob — "a
    /// felt-tip dot, not a G-pen exit", the single defect that failed the
    /// two best presets (gauntlet critic, round 2).
    ///
    /// Measured on the ray's own axis: the ink width perpendicular to it,
    /// at the outer end and a quarter of the way in.
    #[test]
    fn outer_ends_are_needles_not_caps() {
        // One ray along +x, outer end at x = 900, inner at x = 300.
        let p = FocusLinesParams {
            center: [300.0, 512.0],
            r_in: 0.0,
            r_out: 600.0,
            count: 1,
            width: 24.0,
            taper: 0.9,
            seed: 5,
            ..Default::default()
        };
        // The ink's half-height at a given x, measured off the axis.
        let hh = |m: &HashMap<TileIdx, Arc<Tile>>, x: i32| {
            (0..200).take_while(|d| ink_at(m, x, 512 + d) || ink_at(m, x, 512 - d)).count()
        };
        let blunt = render_focus(&p, (1024, 1024));
        let needled = render_focus(
            &FocusLinesParams {
                mix: Mix {
                    entry: 0.25,
                    ..Mix::default()
                },
                ..p.clone()
            },
            (1024, 1024),
        );
        // The cap: full width right up to the last pixel of the stroke.
        assert!(
            hh(&blunt, 898) >= 10,
            "the round cap this test exists to catch is 12 px wide at its own end ({} px)",
            hh(&blunt, 898)
        );
        // With `entry`, the same end is a point and the body is untouched.
        assert!(
            hh(&needled, 898) <= 2,
            "the outer end needles ({} px half-height)",
            hh(&needled, 898)
        );
        assert!(
            hh(&needled, 750) >= 8,
            "…and it is back to full weight a quarter in ({} px)",
            hh(&needled, 750)
        );
    }

    /// A HEAVY tapered stroke has to end in a point too. `taper` 0.9
    /// leaves `(1 − 0.9)^needle` of the nib standing at the tip: on a
    /// hairline that is under half a pixel and the floor in `segment`
    /// reads it as a needle, but on a 24 px accent it is a rounded 4 px
    /// stub — the one round cap left on the sheet after round 2
    /// (`dark-burst-crop` (182,149)). The residue runs out over the last
    /// tenth of the length, at every weight.
    #[test]
    fn heavy_tapered_ends_run_out_to_a_point() {
        // The arithmetic, first: the tip is zero and the body is not
        // touched. 0.158 is what the tip used to be.
        assert!(
            width_at(1.0, 0.9, 0.0, 0.8) == 0.0,
            "the tip is a point ({})",
            width_at(1.0, 0.9, 0.0, 0.8)
        );
        let body = width_at(0.8, 0.9, 0.0, 0.8);
        assert!(
            (body - (1.0f32 - 0.9 * 0.8).powf(0.8)).abs() < 1e-6,
            "and everything before the last tenth is the shape it was ({body})"
        );

        // …and on the page. One 48 px ray, inner end at x = 600, drawn
        // from an outer end off the right of the canvas.
        let p = FocusLinesParams {
            center: [300.0, 512.0],
            r_in: 300.0,
            r_out: 900.0,
            count: 1,
            width: 48.0,
            taper: 0.9,
            mix: Mix {
                needle: 0.8,
                ..Mix::default()
            },
            seed: 5,
            ..Default::default()
        };
        let m = render_focus(&p, (1024, 1024));
        let hh = |x: i32| {
            (0..200)
                .take_while(|d| ink_at(&m, x, 512 + d) || ink_at(&m, x, 512 - d))
                .count()
        };
        assert!(
            hh(604) <= 2,
            "the inner end needles ({} px half-height)",
            hh(604)
        );
        assert!(
            hh(700) >= 6,
            "…and it is still a real stroke a fifth in ({} px)",
            hh(700)
        );
    }

    /// `group_jit` has to make bundles of DIFFERENT sizes, in both walks.
    /// Bundles of exactly two at a near-constant pitch is a picket fence,
    /// and the critic could read the period off the page (round 2).
    #[test]
    fn bundle_sizes_vary() {
        // Sizes between the holes: a step bigger than 1.5 × the gap ends
        // a bundle.
        let sizes = |v: &[f32], gap: f32| {
            let mut out = Vec::new();
            let mut n = 1;
            for w in v.windows(2) {
                if w[1] - w[0] > gap * 1.5 {
                    out.push(n);
                    n = 1;
                } else {
                    n += 1;
                }
            }
            out
        };
        let mut seed = 9u64 | 1;
        let runs = walk_offsets(0.0, 512.0, 8.0, 5, 3.0, 0.8, &mut seed);
        let a = sizes(&runs, 8.0);
        let mut set: Vec<u32> = a.clone();
        set.sort_unstable();
        set.dedup();
        assert!(
            set.len() >= 3,
            "the speed walk's bundles come in at least three sizes over 512 px, got {a:?}"
        );

        let rays = radial_angles(0, 2.0, 180.0, 0.0, 5, 3.0, 0.8, 9).expect("a walked sweep");
        let b = sizes(&rays, 2f32.to_radians());
        let mut rset: Vec<u32> = b.clone();
        rset.sort_unstable();
        rset.dedup();
        assert!(
            rset.len() >= 3,
            "and so do the radial walk's, {b:?}"
        );

        // …and with the knob off, every bundle is exactly `group` — the
        // behaviour every file saved before this field still gets.
        let mut s0 = 9u64 | 1;
        let flat = sizes(&walk_offsets(0.0, 512.0, 8.0, 5, 3.0, 0.0, &mut s0), 8.0);
        assert!(
            flat.iter().all(|n| *n == 5),
            "group_jit 0 is the old constant bundle, got {flat:?}"
        );
    }

    /// Two NEIGHBOURING lines are never both accents: at `dark-burst`'s
    /// 1° pitch two adjacent 40 px wedges merge into a slab and the white
    /// sliver left between them dashes out. The roll still happens every
    /// line, so nothing else in the random sequence moves.
    #[test]
    fn accents_never_land_on_neighbours() {
        let mix = Mix {
            accent_frac: 0.9,
            accent_mul: 6.0,
            ..Mix::default()
        };
        let mut seed = 12_345u64 | 1;
        let (mut prev, mut last, mut n) = (false, false, 0);
        let mut acc = 0.0f32;
        for _ in 0..500 {
            let is = mix.accent(2.0, &mut seed, &mut prev, &mut acc) > 2.0;
            assert!(!(is && last), "two neighbours both drew as accents");
            n += is as u32;
            last = is;
        }
        // A 0.9 share refused after every hit still lands on about half.
        assert!(n > 150, "…but accents still happen ({n} of 500)");
    }

    /// The rails have to be spread ACROSS the panel, not merely present.
    /// An independent coin per run has the right expected fraction and
    /// the wrong distribution at the counts a panel holds: `stream-line`
    /// carries ~7 accents, and seven coin flips landing in one half is
    /// ordinary luck — the critic measured the thickest stroke at 7 px in
    /// the top half against 26 px in the bottom (round 3), which reads as
    /// two thirds of the paper at one weight. The stratified accumulator
    /// in [`Mix::accent`] spends the same fraction evenly.
    #[test]
    fn accents_spread_across_the_panel() {
        // A stream set at panel scale: 64 runs across the normal extent,
        // 15 % of them accents — the shipped `stream-line` numbers.
        let p = SpeedLinesParams {
            angle_deg: 0.0,
            len_min: 900.0,
            len_max: 900.0,
            width: 4.0,
            gap_px: 16.0,
            mix: Mix {
                accent_frac: 0.15,
                accent_mul: 8.0,
                ..Mix::default()
            },
            seed: 21,
            ..Default::default()
        };
        let m = render_speed(&p, (1024, 1024));
        // Vertical ink runs down the middle of the block: at 0° every run
        // is horizontal, so a run's height IS its width. Only an accent
        // clears twice the 4 px nib.
        let mut per_quarter = [0usize; 4];
        let mut y = 0;
        while y < 1024 {
            if ink_at(&m, 512, y) {
                let s = y;
                while y < 1024 && ink_at(&m, 512, y) {
                    y += 1;
                }
                if y - s >= 8 {
                    per_quarter[((s + y) / 2 / 256).min(3) as usize] += 1;
                }
            } else {
                y += 1;
            }
        }
        let lo = *per_quarter.iter().min().unwrap();
        let hi = *per_quarter.iter().max().unwrap();
        assert!(
            lo >= 1,
            "every quarter of the panel carries a rail, got {per_quarter:?}"
        );
        assert!(
            hi <= lo * 3,
            "and no quarter hoards them (max {hi}, min {lo}, {per_quarter:?})"
        );
    }

    /// The mean inner end of a 集中線, ray by ray: how far in the ink
    /// reaches along each exact ray angle.
    fn inner_radii(m: &HashMap<TileIdx, Arc<Tile>>, c: [f32; 2], n: u32, r_out: f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let a = i as f32 * std::f32::consts::TAU / n as f32;
                let (s, cs) = a.sin_cos();
                let mut inner = r_out;
                for r in 4..(r_out as i32) {
                    let r = r as f32;
                    if ink_at(m, (c[0] + cs * r) as i32, (c[1] + s * r) as i32) {
                        inner = r;
                        break;
                    }
                }
                inner
            })
            .collect()
    }

    fn mean_inner_radius(m: &HashMap<TileIdx, Arc<Tile>>, c: [f32; 2], n: u32, r_out: f32) -> f32 {
        let v = inner_radii(m, c, n, r_out);
        v.iter().sum::<f32>() / v.len() as f32
    }

    /// How far the inner ends scatter, in px.
    fn inner_spread(m: &HashMap<TileIdx, Arc<Tile>>, c: [f32; 2], n: u32, r_out: f32) -> f32 {
        let v = inner_radii(m, c, n, r_out);
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        (v.iter().map(|r| (r - mean) * (r - mean)).sum::<f32>() / v.len() as f32).sqrt()
    }

    /// The white core of a printed 集中線 is a ragged BLOB — the inner
    /// ends sit in a band, not on a circle (gauntlet critic, round 1:
    /// "inner ends form a visible ring around a clean circular core").
    ///
    /// `length_jitter` alone cannot make one: it only pulls an end
    /// OUTWARD from `r_in`, and with a length skew most rays draw a tiny
    /// pull and land back on `r_in` exactly. `core_jit` pulls the other
    /// way, so the ends straddle the hole radius.
    ///
    /// Angle and width jitters off, so what is measured is the radial
    /// draw and nothing else. Two claims, because one number cannot carry
    /// both:
    ///
    /// - ISOLATED (`length_jitter` 0): without `core_jit` every ray ends
    ///   on `r_in` to the pixel — a compass circle, σ ≈ 0. With it, the
    ///   draw is uniform over `0.3 × 150 = 45 px`, σ ≈ 13 by arithmetic
    ///   and 12.1 measured (0.5 px without it).
    /// - AS SHIPPED (`length_jitter` 0.3, the presets' shape): the two
    ///   draws together measure σ = 17.4 px on a 150 px hole, past the
    ///   0.1 · r_in the eye needs to stop reading a circle. One-sided
    ///   uniform noise can never reach that on its own — 0.3/√12 is
    ///   0.087 · r_in — which is why both halves are stated.
    #[test]
    fn inner_ends_stagger_not_ring() {
        let p = |length_jitter: f32, core_jit: f32| FocusLinesParams {
            center: [512.0, 512.0],
            r_in: 150.0,
            r_out: 480.0,
            count: 72,
            width: 6.0,
            angle_jitter: 0.0,
            width_jitter: 0.0,
            length_jitter,
            core_jit,
            seed: 11,
            ..Default::default()
        };
        let sd = |lj: f32, cj: f32| {
            inner_spread(
                &render_focus(&p(lj, cj), (1024, 1024)),
                [512.0, 512.0],
                72,
                480.0,
            )
        };
        let ring = sd(0.0, 0.0);
        let staggered = sd(0.0, 0.3);
        assert!(ring < 1.0, "without it, one circle ({ring:.1} px sd)");
        assert!(
            staggered > 10.0,
            "core_jit alone scatters the ends ({staggered:.1} px sd on a 150 px hole)"
        );
        let shipped = sd(0.3, 0.3);
        assert!(
            shipped >= 15.0,
            "and the shipped mix clears 0.1 · r_in ({shipped:.1} px)"
        );
    }

    /// A printed burst is mostly LONG rays with a few stubs; a uniform
    /// draw is an even fuzzy ring. `len_skew` pulls the inner ends in.
    #[test]
    fn len_skew_biases_long() {
        let p = |len_skew: f32| FocusLinesParams {
            center: [512.0, 512.0],
            r_in: 100.0,
            r_out: 400.0,
            count: 36,
            width: 4.0,
            length_jitter: 1.0,
            mix: Mix {
                len_skew,
                ..Default::default()
            },
            seed: 11,
            ..Default::default()
        };
        let flat = mean_inner_radius(&render_focus(&p(0.0), (1024, 1024)), [512.0, 512.0], 36, 400.0);
        let skewed =
            mean_inner_radius(&render_focus(&p(1.0), (1024, 1024)), [512.0, 512.0], 36, 400.0);
        assert!(
            skewed < flat - 20.0,
            "skewed rays reach further in ({skewed:.1} vs {flat:.1})"
        );
        assert!(skewed > 100.0, "but never past the hole ({skewed:.1})");
    }

    /// ref-09's ゴ… drips hang off the panel's top edge: every run STARTS
    /// on one line and ends where its own length ran out. The scatter
    /// cannot do that — it leaves half the runs floating.
    #[test]
    fn anchored_runs_start_on_the_reference_line() {
        let p = |start_mode: u8| SpeedLinesParams {
            // 90° = straight down the page.
            angle_deg: 90.0,
            len_min: 200.0,
            len_max: 200.0,
            width: 2.0,
            gap_px: 16.0,
            jit_len: 0.7,
            start_mode,
            anchor: Some([256.0, 50.0]),
            seed: 5,
            ..Default::default()
        };
        let m = render_speed(&p(1), (512, 512));
        let row = |y: i32| (0..512).filter(|x| ink_at(&m, *x, y)).count();
        assert_eq!(
            (0..46).map(row).sum::<usize>(),
            0,
            "nothing above the reference line"
        );
        assert!(row(55) >= 15, "and every run hangs off it ({})", row(55));
        // Ragged tails: the far end thins out as the short runs stop.
        assert!(
            row(240) * 2 < row(55),
            "lengths differ ({} at the top, {} deep)",
            row(55),
            row(240)
        );

        // The scatter puts runs above the line — that is the before
        // picture, and why the mode exists.
        let loose = render_speed(&p(0), (512, 512));
        assert!(
            (0..46).any(|y| (0..512).any(|x| ink_at(&loose, x, y))),
            "the scatter really does start anywhere"
        );
    }
}

// --- SF-004/005 (TRIAGE 140, r85): the generator's parameters persist on
// the layer, so effect lines stay EDITABLE — the dialog reopens with the
// layer's own values and re-applies in place ("a week later" is the
// point). The dialog's (focus, a..d, count, width, jitter, seed) tuple
// is the serialized form; the two render fns remain the raster source.

/// A generated effect-line layer's parameters, as the dialog holds them.
///
/// `kind` is the generator discriminant, added 2026-08-22 with the flash
/// round. EVERY file written before that date has no such attribute, so
/// `#[serde(default)]` = `0` MUST keep meaning exactly what those files
/// meant — pinned by `pre_flash_specs_load_with_the_old_meaning`:
///
/// - `0` — the original pair, chosen by `focus`: 集中線 focus lines
///   (`a`,`b` = centre, `c` = r_in, `d` = r_out) or 流線 speed lines
///   (`a` = angle°, `b` = len_min, `c` = len_max, `d` unused).
/// - `1` — ウニフラッシュ sea-urchin flash: filled triangular spikes,
///   focus geometry, `width` = the spike base width in px at `r_out`.
/// - `2` — solid flash: the same teeth cut out of a solid ring.
///
/// An UNKNOWN kind falls back to the kind-0 reading rather than
/// rendering nothing: a file from a future build should look wrong, not
/// vanish. Kinds 1/2 keep `focus = true` on purpose — the Object tool's
/// driver handles and their drag clamps key on that flag, and a flash is
/// aimed exactly like a focus-line burst.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GenLinesSpec {
    pub focus: bool,
    /// focus: center.x, center.y, r_in, r_out; speed: angle_deg, len_min, len_max (d unused).
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub count: u32,
    pub width: f32,
    pub jitter: f32,
    pub seed: u64,
    /// Generator discriminant — see the type doc. Absent = 0 = legacy.
    #[serde(default)]
    pub kind: u8,
    /// Speed lines only: [`SpeedLinesParams::taper`]. Absent = 0 = the
    /// constant-width runs every older file drew.
    #[serde(default)]
    pub taper: f32,
    /// Speed lines only: [`SpeedLinesParams::converge`]. Absent = None.
    #[serde(default)]
    pub converge: Option<[f32; 2]>,

    // --- density round, 2026-08-23. EVERY field below is
    // `#[serde(default)]` and 0 MUST keep meaning exactly what a file
    // written before them meant, same rule as `kind`: the bit-stability
    // pin and `pre_flash_specs_load_with_the_old_meaning` are the guards.
    /// Radial kinds: the angular gap in DEGREES between neighbouring rays
    /// — CSP's tutorials size a 集中線 by gap (≈3° dense, ≈10° sparse),
    /// not by a count that means something different on every page size.
    /// >0 derives `count`; 0 keeps the stored `count`.
    #[serde(default)]
    pub gap_deg: f32,
    /// Speed lines: [`SpeedLinesParams::gap_px`]. 0 = the old scatter.
    #[serde(default)]
    pub gap_px: f32,
    /// Speed lines: [`SpeedLinesParams::group`] (まとまり).
    #[serde(default)]
    pub group: u32,
    /// Speed lines: [`SpeedLinesParams::group_gap`].
    #[serde(default)]
    pub group_gap: f32,
    /// 0 = fall back to the single `jitter` (which is what every older
    /// file has). Split because a printed set wants a lot of length
    /// wobble and almost no angular wobble, and one knob cannot say that.
    #[serde(default)]
    pub jit_gap: f32,
    #[serde(default)]
    pub jit_len: f32,
    #[serde(default)]
    pub jit_width: f32,
    /// The ink colour, sRGB. Absent = `[0, 0, 0]` = the black every older
    /// file drew — which is also the only value that touches no pixel
    /// (see [`recolor`]), so the legacy raster is bit-identical.
    #[serde(default)]
    pub color: [u8; 3],

    // --- parity round, 2026-09-06 (plan `2026-09-06-effect-lines-parity`).
    // Same rule again, and it is the load-bearing one: every field below
    // is `#[serde(default)]` and its zero MUST be exactly today's raster,
    // because every effect-line layer in every saved file regenerates
    // through this struct. Each of them guards its own `rand()` call, so
    // an absent field does not even shift the random sequence.
    /// 0..1 — [`Mix::accent_frac`]. 0 = no accents, one weight as before.
    #[serde(default)]
    pub accent_frac: f32,
    /// [`Mix::accent_mul`]. 0 reads as 1.
    #[serde(default)]
    pub accent_mul: f32,
    /// 0..1 — [`Mix::entry`]. 0 = the round cap every older file drew.
    #[serde(default)]
    pub entry: f32,
    /// [`Mix::needle`]. 0 = k = 1 = the straight wedge.
    #[serde(default)]
    pub needle: f32,
    /// 0..1 — [`Mix::len_skew`]. 0 = the uniform length draw.
    #[serde(default)]
    pub len_skew: f32,
    /// Radial kinds: [`FocusLinesParams::sweep_deg`], centred on
    /// `hand_deg`. 0 = the full 360°.
    #[serde(default)]
    pub sweep_deg: f32,
    /// Speed lines: [`SpeedLinesParams::start_mode`]. 0 = the scatter.
    #[serde(default)]
    pub start_mode: u8,
    /// Speed lines: [`SpeedLinesParams::jit_start`].
    #[serde(default)]
    pub jit_start: f32,
    /// Radial kinds: [`FocusLinesParams::core_jit`] (and the flashes'
    /// [`UrchinParams::core_jit`]). 0 = every inner end on the same
    /// circle, which is what every older file drew.
    #[serde(default)]
    pub core_jit: f32,

    // --- gauntlet round 2, 2026-09-06. Same rule a third time: every
    // field here is `#[serde(default)]`, its zero is exactly the raster
    // above it, and each guards its own `rand()`.
    /// Radial kinds: [`FocusLinesParams::jit_len_out`]. 0 = the outer end
    /// jitters by `jit_len` like the inner one, which is the round cap.
    #[serde(default)]
    pub jit_len_out: f32,
    /// 0..1 — [`SpeedLinesParams::group_jit`] /
    /// [`FocusLinesParams::group_jit`]. 0 = constant-size bundles.
    #[serde(default)]
    pub group_jit: f32,
    /// Speed lines, DEGREES: [`SpeedLinesParams::jit_angle`]. 0 = dead
    /// parallel runs.
    #[serde(default)]
    pub jit_angle: f32,

    // --- Lane F, 2026-09-07 (plan `2026-09-07-lane-F-brief`). The rule a
    // fourth time: `#[serde(default)]`, and 0 is exactly the raster above
    // it. The flash kinds' own guards are in `render_urchin`.
    /// Flash kinds: [`UrchinParams::width_frac`]. 0 = `width` in px, which
    /// is what every saved flash carries.
    #[serde(default)]
    pub width_frac: f32,
    /// Flash kinds: [`UrchinParams::tip_out`]. `false` = the point is at
    /// the hole, which is what every saved flash drew.
    #[serde(default)]
    pub tip_out: bool,
    /// Solid flash: [`UrchinParams::field`]. 0 = the black stops with
    /// the teeth, which is what every saved solid flash drew.
    #[serde(default)]
    pub field: f32,

    // --- placement geometry. These were screen-side only until the
    // parity round: `hand_deg` now also aims a radial `sweep_deg` and
    // `anchor` now also holds a stream's `start_mode 1` reference line.
    // Both readings are behind the new field's own `> 0` guard, so a file
    // that predates them still renders from geometry alone.
    /// Radial kinds: the angle (degrees) the r_in/r_out driver handles sit
    /// at — the direction the placing drag was made in, so the handles
    /// land where the gesture did instead of always due east (and off the
    /// page for a burst near the right edge). 0 = +x, the old placement.
    #[serde(default)]
    pub hand_deg: f32,
    /// Speed lines: where the blue reference line and its handles are
    /// anchored — the placing drag's midpoint. `None` = the canvas
    /// centre, which is where they used to be for every run on the page.
    #[serde(default)]
    pub anchor: Option<[f32; 2]>,
}

/// Repaint an already-rendered set from black to `color`, premultiplied.
///
/// A post-pass rather than a colour argument threaded through four
/// rasterizers: the generators ink FULL alpha only, so premultiplied
/// recolouring is exact, and `[0, 0, 0]` returns without touching a pixel
/// — which is what keeps every saved layer bit-identical.
fn recolor(map: &mut HashMap<TileIdx, Arc<Tile>>, color: [u8; 3]) {
    if color == [0, 0, 0] {
        return;
    }
    let c = color.map(|v| ((v as u32 * FIX15_ONE as u32) / 255) as u16);
    for tile in map.values_mut() {
        let t = Arc::make_mut(tile);
        let d = t.data_mut();
        for px in d.chunks_exact_mut(4) {
            if px[3] > 0 {
                px[0] = c[0];
                px[1] = c[1];
                px[2] = c[2];
            }
        }
    }
}

impl GenLinesSpec {
    /// Scale the generator's px geometry about the canvas origin —
    /// `IO-060`'s share. The rendered raster resamples with the layer; this
    /// keeps the re-editable spec pointing at the same place on the paper,
    /// so reopening the dialog after a resolution change does not re-burst
    /// the page from the old centre at the old radius.
    ///
    /// `a`..`d` mean different things per generator (see the field docs):
    /// radial kinds hold `(cx, cy, r_in, r_out)`, speed lines hold
    /// `(angle_deg, len_min, len_max, —)`. An ANGLE must not be scaled,
    /// which is why this branches instead of multiplying all four.
    pub fn scale(&mut self, sx: f32, sy: f32, s: f32) {
        if self.radial() {
            self.a *= sx;
            self.b *= sy;
            self.c *= s;
            self.d *= s;
        } else {
            // a = angle_deg: dimensionless.
            self.b *= s;
            self.c *= s;
        }
        self.width *= s;
        self.gap_px *= s;
        self.group_gap *= s;
        if let Some(c) = &mut self.converge {
            c[0] *= sx;
            c[1] *= sy;
        }
        if let Some(a) = &mut self.anchor {
            a[0] *= sx;
            a[1] *= sy;
        }
    }

    /// The layer name a fresh generation gets — one place, so the app,
    /// the Materials bank and the dialog cannot disagree.
    pub fn layer_name(&self) -> &'static str {
        match self.kind {
            1 => "Urchin flash",
            2 => "Solid flash",
            _ if self.focus => "Focus lines",
            _ => "Speed lines",
        }
    }

    /// Does this generator converge on a point (focus lines and both
    /// flashes)? The aim-at-the-click paste rule keys on this — keying on
    /// `focus` alone left kind 1/2 materials placing at their stored
    /// centre instead of the cursor (M7 audit finding).
    pub fn radial(&self) -> bool {
        self.kind == 1 || self.kind == 2 || self.focus
    }

    /// How many rays/spikes a radial kind draws: the length of the
    /// angular walk when a sweep or a bundle asks for one, else
    /// gap-driven when `gap_deg` is set (CSP's own unit for a 集中線),
    /// else the stored count. Capped — a 0.05° gap is 7 200 rays and a
    /// UI hang.
    ///
    /// Only kind 0 consults the walk: the flashes count their teeth and
    /// spread them over the full circle by construction, so a sweep on
    /// one would return a number the tooth renderer does not honour.
    pub fn ray_count(&self) -> u32 {
        if let Some(v) = self.radial_angles() {
            return v.len() as u32;
        }
        if self.gap_deg > 0.0 {
            ((360.0 / self.gap_deg).ceil() as u32).clamp(1, 4096)
        } else {
            self.count.max(1)
        }
    }

    /// This spec's ray angles, or `None` for the even full circle.
    fn radial_angles(&self) -> Option<Vec<f32>> {
        if self.kind != 0 {
            return None;
        }
        radial_angles(
            self.count,
            self.gap_deg,
            self.sweep_deg,
            self.hand_deg,
            self.group,
            self.group_gap,
            self.group_jit,
            self.seed,
        )
    }

    /// The parity-round knobs both renderers share, read off the spec.
    fn mix(&self) -> Mix {
        Mix {
            accent_frac: self.accent_frac,
            accent_mul: self.accent_mul,
            entry: self.entry,
            needle: self.needle,
            len_skew: self.len_skew,
        }
    }

    /// One of the split jitters, falling back to the single legacy
    /// `jitter` while it is 0 — the whole back-compat rule in one place.
    fn jit(&self, v: f32) -> f32 {
        if v > 0.0 { v } else { self.jitter }
    }

    /// Rasterize the spec into tiles (the shared source with the dialog).
    pub fn render(&self, size: (u32, u32)) -> HashMap<TileIdx, Arc<Tile>> {
        let mut map = if self.kind == 1 || self.kind == 2 {
            render_urchin(
                &UrchinParams {
                    center: [self.a, self.b],
                    r_in: self.c,
                    r_out: self.d,
                    // The stored count is the PITCH count — how many
                    // teeth a full circle would hold at this spacing. The
                    // bundle walk and the sweep then decide how many are
                    // actually drawn, inside the renderer, so `ray_count`
                    // (which reports the walk's length for kind 0) must
                    // not be applied here or the pitch would be derived
                    // from its own result.
                    count: self.count.max(1),
                    width: self.width,
                    width_frac: self.width_frac,
                    angle_jitter: self.jit(self.jit_gap),
                    length_jitter: self.jit(self.jit_len),
                    jit_len_out: self.jit_len_out,
                    core_jit: self.core_jit,
                    mix: self.mix(),
                    group: self.group,
                    group_gap: self.group_gap,
                    group_jit: self.group_jit,
                    sweep_deg: self.sweep_deg,
                    sweep_center_deg: self.hand_deg,
                    tip_out: self.tip_out,
                    field: self.field,
                    solid: self.kind == 2,
                    seed: self.seed,
                },
                size,
            )
        } else if self.focus {
            render_focus(
                &FocusLinesParams {
                    center: [self.a, self.b],
                    r_in: self.c,
                    r_out: self.d,
                    count: self.ray_count(),
                    width: self.width,
                    angle_jitter: self.jit(self.jit_gap),
                    width_jitter: self.jit(self.jit_width),
                    length_jitter: self.jit(self.jit_len),
                    jit_len_out: self.jit_len_out,
                    taper: self.taper.clamp(0.0, 1.0),
                    mix: self.mix(),
                    gap_deg: self.gap_deg,
                    sweep_deg: self.sweep_deg,
                    // The drag's direction aims the arc; with sweep 0 it
                    // is not read at all, which is what keeps every file
                    // saved before the sweep bit-identical.
                    sweep_center_deg: self.hand_deg,
                    group: self.group,
                    group_gap: self.group_gap,
                    group_jit: self.group_jit,
                    core_jit: self.core_jit,
                    seed: self.seed,
                },
                size,
            )
        } else {
            render_speed(
                &SpeedLinesParams {
                    angle_deg: self.a,
                    count: self.count.max(1),
                    len_min: self.b,
                    len_max: self.c,
                    width: self.width,
                    taper: self.taper,
                    converge: self.converge,
                    gap_px: self.gap_px,
                    group: self.group,
                    group_gap: self.group_gap,
                    group_jit: self.group_jit,
                    jit_gap: self.jit_gap,
                    jit_len: self.jit_len,
                    jit_width: self.jit_width,
                    jit_angle: self.jit_angle,
                    mix: self.mix(),
                    start_mode: self.start_mode,
                    jit_start: self.jit_start,
                    anchor: self.anchor,
                    seed: self.seed,
                },
                size,
            )
        };
        recolor(&mut map, self.color);
        map
    }
}

#[cfg(test)]
mod spec_tests {
    use super::*;
    use crate::doc::Document;

    /// HARD REQUIREMENT (flash round, 2026-08-22): an .ora or a
    /// `.gen.json` material written before `kind`/`taper`/`converge`
    /// existed carries only the nine original attributes, and must
    /// deserialize into a spec that renders exactly what it rendered
    /// then — not "close", the same tiles.
    #[test]
    fn pre_flash_specs_load_with_the_old_meaning() {
        let legacy_focus = r#"{"focus":true,"a":256.0,"b":256.0,"c":100.0,"d":240.0,"count":64,"width":6.0,"jitter":0.5,"seed":7}"#;
        let s: GenLinesSpec = serde_json::from_str(legacy_focus).expect("old spec still loads");
        assert_eq!(s.kind, 0, "no attribute = the original pair");
        assert_eq!(s.taper, 0.0);
        assert_eq!(s.converge, None);
        assert_eq!(s.layer_name(), "Focus lines");
        let old = render_focus(
            &FocusLinesParams {
                center: [256.0, 256.0],
                r_in: 100.0,
                r_out: 240.0,
                count: 64,
                width: 6.0,
                angle_jitter: 0.5,
                width_jitter: 0.5,
                length_jitter: 0.5,
                taper: 0.0,
                seed: 7,
                ..Default::default()
            },
            (512, 512),
        );
        let new = s.render((512, 512));
        assert_eq!(old.len(), new.len(), "same tiles");
        for (idx, t) in &old {
            assert_eq!(t.data(), new[idx].data(), "legacy focus raster moved");
        }

        let legacy_speed = r#"{"focus":false,"a":20.0,"b":100.0,"c":300.0,"d":0.0,"count":80,"width":4.0,"jitter":0.0,"seed":3}"#;
        let q: GenLinesSpec = serde_json::from_str(legacy_speed).unwrap();
        assert_eq!((q.kind, q.taper, q.converge), (0, 0.0, None));
        assert_eq!(q.layer_name(), "Speed lines");
        let speed = q.render((512, 512));
        assert_eq!(
            super::tests::fingerprint(&speed),
            (57, 25119, 6_096_450_357_538_070_854),
            "the pre-round speed raster, from the pre-round attributes"
        );

        // And the round trip out is still readable BY an old build: the
        // three new attributes are the only additions, and each of them
        // reads back as the value a missing one defaults to.
        let back: GenLinesSpec = serde_json::from_str(&serde_json::to_string(&q).unwrap()).unwrap();
        assert_eq!(back, q);
    }

    /// The flash kinds ride the same field set, survive ORA, and stay
    /// distinguishable — a saved urchin does not reload as focus lines.
    #[test]
    fn flash_kinds_round_trip_through_ora() {
        for (kind, name) in [(1u8, "Urchin flash"), (2, "Solid flash")] {
            let spec = GenLinesSpec {
                focus: true,
                a: 200.0,
                b: 200.0,
                c: 40.0,
                d: 180.0,
                count: 40,
                width: 20.0,
                jitter: 0.25,
                seed: 7,
                kind,
                ..Default::default()
            };
            assert_eq!(spec.layer_name(), name);
            let mut doc = Document::new(400, 400);
            let li = doc.add_layer(name);
            doc.layers[li].genlines = Some(spec);
            assert!(doc.regen_genlines(li, spec), "{name} inked");

            let mut buf = std::io::Cursor::new(Vec::new());
            crate::ora::save_to(&doc, &mut buf).unwrap();
            let re = crate::ora::load_from(std::io::Cursor::new(buf.into_inner())).unwrap();
            let g = re.layers[li].genlines.expect("spec survived");
            assert_eq!(g, spec, "{name}: kind survived the save");
        }
    }

    /// SF-004/005: the spec persists through ORA and regen renders from
    /// it — a re-applied layer keeps its stack position, the tiles follow
    /// the new params.
    #[test]
    fn spec_round_trips_and_regens_in_place() {
        let mut doc = Document::new(400, 400);
        doc.add_layer("Focus lines");
        let spec = GenLinesSpec {
            focus: true,
            a: 200.0,
            b: 200.0,
            c: 20.0,
            d: 180.0,
            count: 40,
            width: 2.0,
            jitter: 0.2,
            seed: 7,
            ..Default::default()
        };
        let li = doc.layers.len() - 1;
        doc.layers[li].genlines = Some(spec);
        assert!(doc.regen_genlines(li, spec));
        assert!(doc.layers[li].tiles().next().is_some(), "focus lines inked");

        let mut buf = std::io::Cursor::new(Vec::new());
        crate::ora::save_to(&doc, &mut buf).unwrap();
        let re = crate::ora::load_from(std::io::Cursor::new(buf.into_inner())).unwrap();
        let gl = re
            .layers
            .iter()
            .position(|l| l.name == "Focus lines")
            .unwrap();
        let g = re.layers[gl].genlines.expect("spec survived");
        assert_eq!(g, spec);

        // Change a param: regen follows, same layer, and the new spec is
        // stored BY the regen (it owns both halves now).
        let mut doc = re;
        let mut s2 = g;
        s2.count = 80;

        assert!(doc.regen_genlines(gl, s2));
        assert!(doc.layers[gl].tiles().next().is_some(), "regen inked");
        assert_eq!(doc.layers[gl].genlines, Some(s2), "the spec went on");
        assert!(
            !doc.regen_genlines(usize::MAX, s2),
            "out-of-bounds index, no regen"
        );
        // A real layer that carries NO spec also refuses (audit H: the
        // old test only exercised the out-of-bounds arm).
        let plain = doc.add_layer("plain");
        assert!(
            !doc.regen_genlines(plain, s2),
            "layer without spec, no regen"
        );
    }

    #[test]
    fn failed_regen_keeps_spec_and_tiles_agreeing() {
        // Audit F, 2026-08-19: a regen that renders nothing must move
        // NEITHER half — the stored spec still describes the pixels that
        // are on screen (the store now happens inside regen_genlines, so
        // this pins both halves rather than the app's old dance).
        let mut doc = Document::new(400, 400);
        doc.add_layer("Focus lines");
        let li = doc.layers.len() - 1;
        let spec = GenLinesSpec {
            focus: true,
            a: 200.0,
            b: 200.0,
            c: 20.0,
            d: 180.0,
            count: 40,
            width: 2.0,
            jitter: 0.2,
            seed: 7,
            ..Default::default()
        };
        doc.layers[li].genlines = Some(spec);
        assert!(doc.regen_genlines(li, spec));
        let tiles_before: Vec<_> = doc.layers[li]
            .tiles()
            .map(|(i, t)| (i, t.clone()))
            .collect();
        assert!(!tiles_before.is_empty());

        // A spec that renders nothing (convergence point far off the
        // canvas — the clip drops every tile): regen refuses, the inked
        // raster stays exactly as it was.
        let mut dead = spec;
        dead.a = -10000.0;
        dead.b = -10000.0;
        assert!(!doc.regen_genlines(li, dead), "nothing rendered, no regen");
        assert_eq!(
            doc.layers[li].genlines,
            Some(spec),
            "the dead spec was not stored"
        );
        let tiles_after: Vec<_> = doc.layers[li]
            .tiles()
            .map(|(i, t)| (i, t.clone()))
            .collect();
        assert_eq!(tiles_before.len(), tiles_after.len(), "tiles unchanged");
        for ((i0, t0), (i1, t1)) in tiles_before.iter().zip(tiles_after.iter()) {
            assert_eq!(i0, i1);
            assert_eq!(t0.data(), t1.data());
        }
    }

    #[test]
    fn regen_is_one_undo_step_and_keeps_the_layers_history() {
        // Audit F's old shape: replace_tiles swapped the raster wholesale,
        // past the copy-on-write recording, so the regen was not undoable
        // and had to purge the layer's pre-images to stay consistent. It
        // now writes through set_tile inside the op bracket — ONE step, and
        // the ink that was on the layer before the regen still undoes.
        let mut doc = Document::new(400, 400);
        let li = doc.add_layer("Focus lines");
        let spec = GenLinesSpec {
            focus: true,
            a: 200.0,
            b: 200.0,
            c: 20.0,
            d: 180.0,
            count: 40,
            width: 2.0,
            jitter: 0.2,
            seed: 7,
            ..Default::default()
        };
        // A first generation, then an ordinary tile write on top of it:
        // two steps for the regen under test to sit above.
        doc.layers[li].genlines = Some(spec);
        assert!(doc.regen_genlines(li, spec));
        doc.begin_op_on(li);
        doc.set_op_label("Stroke");
        doc.layers[li].set_tile(
            crate::tile::TileIdx::new(0, 0),
            Some(std::sync::Arc::new(crate::tile::Tile::default())),
        );
        doc.end_op();
        assert_eq!(
            doc.undo_labels(),
            ["New layer", "Regenerate lines", "Stroke"],
            "the setup's structural add records too"
        );
        let snap = |d: &Document| -> std::collections::BTreeMap<crate::tile::TileIdx, Vec<u16>> {
            d.layers[li]
                .tiles()
                .map(|(i, t)| (i, t.data().to_vec()))
                .collect()
        };
        let before = snap(&doc);

        let mut s2 = spec;
        s2.count = 90;
        s2.seed = 11;
        assert!(doc.regen_genlines(li, s2));
        let regenerated = snap(&doc);
        assert_ne!(before, regenerated, "the regen changed the raster");
        assert_eq!(
            doc.undo_labels(),
            [
                "New layer",
                "Regenerate lines",
                "Stroke",
                "Regenerate lines"
            ],
            "one step for the regen, and the older steps survived it"
        );

        assert!(doc.undo(), "the regen undoes");
        assert_eq!(snap(&doc), before, "pixels back, bit for bit");
        assert_eq!(doc.layers[li].genlines, Some(spec), "and the parameters");
        assert!(doc.redo(), "and redoes");
        assert_eq!(snap(&doc), regenerated);
        assert_eq!(doc.layers[li].genlines, Some(s2));

        // The pre-regen history is still walkable: the stroke, then the
        // first generation.
        assert!(doc.undo() && doc.undo(), "back past the stroke");
        assert!(doc.undo(), "back past the first generation");
        assert!(
            doc.layers[li].tiles().next().is_none(),
            "the layer is empty again"
        );
    }
}
