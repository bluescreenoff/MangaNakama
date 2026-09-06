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
    /// 0..1 — the fraction of lines drawn heavy (see
    /// [`super::Mix::accent_frac`]).
    pub accent_frac: f32,
    /// An accent's width multiplier.
    pub accent_mul: f32,
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
    fn from_mm(dpi: u32, width_mm: f32, gap_mm: f32) -> Self {
        let px = |mm: f32| mm / 25.4 * dpi as f32;
        Self {
            count: 60,
            width: px(width_mm).max(0.5),
            gap_px: px(gap_mm),
            taper: 1.0,
            accent_mul: 1.0,
            seed: 1,
            ..Self::default()
        }
    }

    /// 流線, the everyday one: 1 mm between runs, 0.20 mm wide, bundles
    /// of 4 with a two-and-a-half-gap hole. The spindle (`entry` 0.35
    /// against a full taper) is what ref-07's streak block is made of —
    /// thin, thick, thin, no round cap anywhere on the page.
    pub fn stream(dpi: u32) -> Self {
        Self {
            group: 4,
            group_gap: 2.5,
            jit_gap: 0.25,
            jit_len: 0.5,
            jit_width: 0.4,
            accent_frac: 0.08,
            accent_mul: 3.0,
            entry: 0.35,
            needle: 1.2,
            len_skew: 0.4,
            ..Self::from_mm(dpi, 0.20, 1.0)
        }
    }

    /// A tighter block: 0.6 mm gap, 0.15 mm lines, bundles of 6.
    pub fn dense_stream(dpi: u32) -> Self {
        Self {
            group: 6,
            group_gap: 2.0,
            jit_len: 0.4,
            entry: 0.3,
            accent_frac: 0.06,
            ..Self::stream(dpi)
        }
        .with_mm(dpi, 0.15, 0.6)
    }

    /// The same rule read the other way: gaps you can see between the
    /// runs, so no bundling on top of them.
    pub fn sparse_stream(dpi: u32) -> Self {
        Self {
            group: 0,
            group_gap: 0.0,
            jit_gap: 0.3,
            jit_width: 0.3,
            accent_frac: 0.1,
            accent_mul: 2.5,
            entry: 0.4,
            needle: 1.0,
            jit_len: 0.6,
            len_skew: 0.3,
            ..Self::stream(dpi)
        }
        .with_mm(dpi, 0.30, 2.5)
    }

    /// The perspective block: the same streaks, aimed at a point beyond
    /// the drag so they fan into the impact (ref-08's second panel).
    pub fn perspective_stream(dpi: u32) -> Self {
        Self {
            entry: 0.3,
            accent_frac: 0.1,
            jit_len: 0.6,
            len_skew: 0.5,
            converge_far: 2.5,
            ..Self::stream(dpi)
        }
    }

    /// ref-09's ゴ… drips: sparse thin verticals hanging off one edge,
    /// each a different length, no bundles. The `start_mode` is the
    /// whole preset — scattered, half of them would float in mid-air.
    pub fn drip_lines(dpi: u32) -> Self {
        Self {
            group: 0,
            group_gap: 0.0,
            jit_gap: 0.4,
            jit_width: 0.3,
            accent_frac: 0.0,
            accent_mul: 1.0,
            entry: 0.0,
            needle: 1.5,
            jit_len: 0.7,
            len_skew: 0.0,
            start_mode: 1,
            jit_start: 0.15,
            ..Self::stream(dpi)
        }
        .with_mm(dpi, 0.12, 2.5)
    }

    /// 集中線: a 3° gap, 0.30 mm rays needling to the convergence, a
    /// 35 % hole for the art, and bundles of 4 — the rays come in
    /// clumps on every reference sheet, never at one even pitch.
    ///
    /// No `entry`: a focus ray is heavy at the RIM and needles at the
    /// centre, so it has one point, not two.
    pub fn focus(dpi: u32) -> Self {
        Self {
            gap_deg: 3.0,
            gap_px: 0.0,
            r_in_frac: 0.35,
            group: 4,
            group_gap: 1.5,
            jit_gap: 0.35,
            jit_len: 0.9,
            jit_width: 0.5,
            accent_frac: 0.12,
            accent_mul: 4.0,
            entry: 0.0,
            needle: 1.2,
            len_skew: 0.6,
            ..Self::from_mm(dpi, 0.30, 0.0)
        }
    }

    /// The dense end of CSP's 3°/10° rule: a 2° gap on 0.25 mm rays.
    pub fn dense_focus(dpi: u32) -> Self {
        Self {
            gap_deg: 2.0,
            group: 5,
            group_gap: 1.3,
            accent_frac: 0.10,
            accent_mul: 3.5,
            ..Self::focus(dpi)
        }
        .with_mm(dpi, 0.25, 0.0)
    }

    /// A black burst: rays at a 1.5° gap, nearly twice the weight, a
    /// third of them heavy, small hole (ref-08's top panel).
    pub fn dark_burst(dpi: u32) -> Self {
        Self {
            gap_deg: 1.5,
            r_in_frac: 0.2,
            group: 3,
            group_gap: 1.2,
            jit_gap: 0.4,
            accent_frac: 0.30,
            accent_mul: 4.0,
            needle: 1.0,
            len_skew: 0.7,
            ..Self::focus(dpi)
        }
        .with_mm(dpi, 0.50, 0.0)
    }

    /// The two flash kinds ride the same centre-out gesture, but their
    /// `width` is a spike BASE in px and their teeth are counted, not
    /// gapped — so they keep the count-driven preset, and the parity
    /// knobs stay off: the teeth carry their own shape.
    pub fn flash(dpi: u32, count: u32, width_mm: f32, r_in_frac: f32) -> Self {
        Self {
            count,
            gap_deg: 0.0,
            jitter: 0.25,
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
            (dir_deg, cross, cross, 0.0)
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
            jit_gap: self.jit_gap,
            jit_len: self.jit_len,
            jit_width: self.jit_width,
            accent_frac: self.accent_frac,
            accent_mul: self.accent_mul,
            entry: self.entry,
            needle: self.needle,
            len_skew: self.len_skew,
            sweep_deg: if radial { self.sweep_deg } else { 0.0 },
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
            anchor: (!radial).then(|| {
                if self.start_mode == 1 {
                    a
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
