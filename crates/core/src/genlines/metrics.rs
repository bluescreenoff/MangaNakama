//! MEASURING a generated effect-line panel, so a preset is tuned to
//! numbers instead of to an opinion.
//!
//! Written for Lane F (plan `2026-09-07-lean-gauntlet-effects`, "the lean
//! loop"): the last gauntlet spent a whole builder round on a defect the
//! builder had eyeballed wrong, and a whole critic round re-deriving
//! numbers by hand. Everything here is what the critics measured off the
//! PNGs, re-implemented once so the example and the regression test agree
//! by construction.
//!
//! It lives in the library rather than in the example because two callers
//! need it: `examples/effect_metrics.rs` prints the table a critic reads,
//! and the target table in `presets::tests` fails CI when a preset drifts
//! off it. A second copy would drift the first time either moved.
//!
//! Nothing here is part of the drawing path — it renders a spec and reads
//! the raster back, exactly as an outside judge would.

use std::collections::HashMap;
use std::sync::Arc;

use super::GenLinesSpec;
use crate::tile::{TILE_SIZE, Tile, TileIdx};

/// A rendered panel as one flat bit per pixel — the only thing every
/// measurement below wants, and a `HashMap` of tiles is the wrong shape
/// for a scan that walks a circle or erodes a neighbourhood.
pub struct Ink {
    pub w: usize,
    pub h: usize,
    px: Vec<bool>,
}

impl Ink {
    /// Render `spec` and flatten it.
    pub fn of(spec: &GenLinesSpec, size: (u32, u32)) -> Self {
        let tiles: HashMap<TileIdx, Arc<Tile>> = spec.render(size);
        let (w, h) = (size.0 as usize, size.1 as usize);
        let mut px = vec![false; w * h];
        for (idx, t) in &tiles {
            let (ox, oy) = idx.origin();
            for y in 0..TILE_SIZE {
                for x in 0..TILE_SIZE {
                    let (gx, gy) = (ox + x as i32, oy + y as i32);
                    if gx < 0 || gy < 0 || gx >= size.0 as i32 || gy >= size.1 as i32 {
                        continue;
                    }
                    if t.pixel(x, y)[3] > 0 {
                        px[gy as usize * w + gx as usize] = true;
                    }
                }
            }
        }
        Self { w, h, px }
    }

    #[inline]
    pub fn inside(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h
    }

    /// Ink at a pixel. Outside the panel is NOT ink — every scan below
    /// keeps its own "was that inside?" flag, so this can be total.
    #[inline]
    pub fn at(&self, x: i32, y: i32) -> bool {
        self.inside(x, y) && self.px[y as usize * self.w + x as usize]
    }

    /// Sample at a float position (nearest pixel).
    #[inline]
    fn at_f(&self, x: f32, y: f32) -> bool {
        self.at(x.floor() as i32, y.floor() as i32)
    }
}

/// One panel's numbers. Millimetres everywhere a length appears: a
/// preset is authored in mm and a critic judges a printed page, and px
/// at 600 dpi is neither.
#[derive(Clone, Debug, Default)]
pub struct Metrics {
    /// Where the cross-section was taken. Radial sets: the radius, in mm
    /// from the centre. Streams: `None` — the cut is a straight line
    /// across the panel's middle, perpendicular to the run direction.
    pub r_meas_mm: Option<f32>,
    /// Complete strokes crossed per 25 mm of that cut.
    pub per_25mm: f32,
    /// How many complete strokes the cut crossed (the sample `per_25mm`,
    /// the percentiles and the bundle histogram are all drawn from).
    pub strokes: usize,
    /// Stroke width across the cut, in mm: 5th / 50th / 95th percentile.
    /// For a filled flash this is the tooth; for the solid one it is the
    /// BLACK spike between two cut teeth, which is the mark on the paper
    /// either way.
    pub w_p5: f32,
    pub w_p50: f32,
    pub w_p95: f32,
    /// Share of strokes at least twice the median width — "how many of
    /// these are accents", read off the paper rather than off the knob.
    pub accent_pct: f32,
    /// Ends that stop dead instead of running out to a point. See
    /// [`round_caps`].
    pub caps: usize,
    /// Marks that do not belong to anything. See [`fragments`]: dirt
    /// specks anywhere, plus — on a solid flash, whose black is meant to
    /// be ONE field — every black island that floats free of it. Target 0.
    pub frags: usize,
    /// The ink percentage of the EMPTIEST of the four corner boxes
    /// (12 % × 12 % of the panel). A ベタフラ is "an all-black rectangle
    /// with a white burst punched through" (ref-20), so its corners are
    /// unbroken black and this is 100. For every other kind it is just a
    /// reading.
    pub corner_ink: f32,
    /// Ink percentage per panel quarter, reading order (TL, TR, BL, BR).
    pub quarter_ink: [f32; 4],
    /// Runs of strokes whose separating gaps fall in the small of the two
    /// gap populations on the cut, as `(bundle size, how many)` sorted by
    /// size — see [`bundle_sizes`]. All `1×` means the set has one pitch
    /// and no packs; a spread means it clusters the way a hand does.
    pub bundles: Vec<(usize, usize)>,
    /// Radial sets only: sweeping rays out of the centre, the radius at
    /// which each FIRST meets ink — the hole of a 集中線 or a ウニフラ,
    /// and (rendered as it is, black field around a white burst) the
    /// white region of a ベタフラ. Mean and standard deviation, in mm.
    /// `None` for a stream.
    ///
    /// This is the measurement `REFS.md` quotes for every reference in
    /// the pack, so ours and theirs are the same number.
    pub hole_mean_mm: Option<f32>,
    pub hole_sigma_mm: Option<f32>,
    /// The same sweep's longest and shortest reach as a multiple of its
    /// MEDIAN — REFS target 4 states a solid flash's white region as
    /// 1.5–2.0× median at the longest and 0.55–0.65× at the shortest.
    pub hole_hi_ratio: Option<f32>,
    pub hole_lo_ratio: Option<f32>,
    /// The same sweep's LAST ink per ray: the set's outer silhouette.
    /// `None` when that silhouette is the panel frame rather than the
    /// effect (a burst placed to fill the panel has no outline to judge),
    /// which is decided by how many rays end clear of the border.
    ///
    /// REFS target 3, the sharpest pass/fail in the pack: a ウニフラ's
    /// hole must be ROUGHER than its outline (ref-12 measures 20 % inner
    /// against 11 % outer). Smoother, and the build has made a 密フラッシュ.
    pub outer_mean_mm: Option<f32>,
    pub outer_sigma_mm: Option<f32>,
}

impl Metrics {
    /// σ as a percentage of the mean — how every REFS target is stated.
    pub fn hole_rel(&self) -> Option<f32> {
        match (self.hole_mean_mm, self.hole_sigma_mm) {
            (Some(m), Some(s)) if m > 0.0 => Some(100.0 * s / m),
            _ => None,
        }
    }

    pub fn outer_rel(&self) -> Option<f32> {
        match (self.outer_mean_mm, self.outer_sigma_mm) {
            (Some(m), Some(s)) if m > 0.0 => Some(100.0 * s / m),
            _ => None,
        }
    }
}

/// Measure one placed spec on a `size` panel authored at `dpi`.
pub fn measure(spec: &GenLinesSpec, size: (u32, u32), dpi: u32) -> Metrics {
    let ink = &Ink::of(spec, size);
    let px_mm = 25.4 / dpi as f32;
    let (widths, gaps_per_cut, cut_px, r_meas) = cross_section(ink, spec);
    let mut w_mm: Vec<f32> = widths.iter().map(|v| v * px_mm).collect();
    w_mm.sort_by(f32::total_cmp);
    let p = |q: f32| {
        if w_mm.is_empty() {
            0.0
        } else {
            w_mm[(((w_mm.len() - 1) as f32) * q).round() as usize]
        }
    };
    let (w_p5, w_p50, w_p95) = (p(0.05), p(0.50), p(0.95));
    let accent_pct = if w_mm.is_empty() {
        0.0
    } else {
        100.0 * w_mm.iter().filter(|v| **v >= 2.0 * w_p50).count() as f32 / w_mm.len() as f32
    };
    let sw = spec
        .radial()
        .then(|| sweep(ink, [spec.a, spec.b], spec.kind == 2));
    let (mut hole_mean_mm, mut hole_sigma_mm) = (None, None);
    let (mut hole_hi_ratio, mut hole_lo_ratio) = (None, None);
    let (mut outer_mean_mm, mut outer_sigma_mm) = (None, None);
    if let Some(sw) = &sw {
        // WHICH white shape the "hole" columns describe. A ウニフラ's is
        // the empty middle, so first-ink per ray says it. A ベタフラ's is
        // the WHOLE white burst — core plus every white sliver running
        // out of it — and that is the shape REFS measures on ref-20/21
        // ("254 white spike tips, white-region radius median 193 px").
        // First-ink cannot say that once the slivers are as fine as the
        // pack's: a ray that leaves the core at any angle meets a black
        // thread within a millimetre, so the reading collapses onto the
        // core and its σ collapses with it. So for the solid kind the
        // sweep is taken over the white REGION connected to the centre.
        let src = if spec.kind == 2 {
            &sw.white
        } else {
            &sw.inner
        };
        if let Some((m, s)) = mean_sigma(src) {
            hole_mean_mm = Some(m * px_mm);
            hole_sigma_mm = Some(s * px_mm);
            let mut v = src.clone();
            v.sort_by(f32::total_cmp);
            let med = v[v.len() / 2].max(1e-3);
            hole_hi_ratio = Some(v[v.len() - 1] / med);
            hole_lo_ratio = Some(v[0] / med);
        }
        // Half the rays have to end clear of the frame before the outer
        // reading is the effect's outline rather than the panel's.
        if sw.free * 2 >= sw.inner.len().max(1) {
            if let Some((m, s)) = mean_sigma(&sw.outer) {
                outer_mean_mm = Some(m * px_mm);
                outer_sigma_mm = Some(s * px_mm);
            }
        }
    }
    Metrics {
        r_meas_mm: r_meas.map(|r| r * px_mm),
        // Strokes per 25 mm of the cut that was actually on the panel —
        // the density number every reference sheet is quoted in. The
        // denominator is the cut's own on-panel length, so a circle that
        // leaves the frame and a line that crosses it are comparable.
        per_25mm: if cut_px > 0.0 {
            widths.len() as f32 / (cut_px * px_mm) * 25.0
        } else {
            0.0
        },
        strokes: widths.len(),
        w_p5,
        w_p50,
        w_p95,
        accent_pct,
        caps: round_caps(ink),
        frags: fragments(ink, spec.kind == 2),
        corner_ink: corner_ink(ink),
        quarter_ink: quarter_ink(ink),
        bundles: bundle_sizes(&gaps_per_cut),
        hole_mean_mm,
        hole_sigma_mm,
        hole_hi_ratio,
        hole_lo_ratio,
        outer_mean_mm,
        outer_sigma_mm,
    }
}

// --- the cross-section -----------------------------------------------

/// The scan a critic does by eye: cut across the set and write down every
/// stroke you cross and every gap between them.
///
/// Radial kinds are cut on a CIRCLE (the ring's mid radius, pulled in
/// until enough of it is on the panel — an off-panel centre's mid radius
/// can be almost entirely outside the frame); streams on a straight line
/// through the panel's middle, perpendicular to the run direction, which
/// is the only cut that measures a run's width rather than its length.
///
/// Returns the stroke widths in px, the gaps grouped per continuous
/// stretch of the cut (so a bundle is never counted across a hole in the
/// panel), the length of cut that was actually ON the panel (px), and the
/// radius the cut was taken at.
fn cross_section(ink: &Ink, spec: &GenLinesSpec) -> (Vec<f32>, Vec<Vec<f32>>, f32, Option<f32>) {
    let (samples, unit, circular, r_meas) = if spec.radial() {
        let c = [spec.a, spec.b];
        // Step in until at least a twentieth of the circle is on the
        // panel. A `saturated-line-centre-below` mid radius sits mostly
        // below the frame, and a cut with four samples on it is noise.
        let mut r = ((spec.c + spec.d) * 0.5).max(2.0);
        let mut s = circle_samples(ink, c, r);
        for _ in 0..40 {
            if s.iter().filter(|v| v.is_some()).count() * 20 >= s.len() {
                break;
            }
            r *= 0.9;
            s = circle_samples(ink, c, r);
        }
        let unit = r * std::f32::consts::TAU / s.len() as f32;
        (s, unit, true, Some(r))
    } else {
        (line_samples(ink, spec.a), 0.5, false, None)
    };
    let on = samples.iter().filter(|v| v.is_some()).count() as f32 * unit;
    let (w, g) = runs(&samples, unit, circular);
    (w, g, on, r_meas)
}

/// `None` = that sample is off the panel, `Some(ink?)` otherwise.
fn circle_samples(ink: &Ink, c: [f32; 2], r: f32) -> Vec<Option<bool>> {
    // Half a pixel of arc per sample: fine enough that a 1 px hairline
    // cannot slip between two samples.
    let n = ((std::f32::consts::TAU * r / 0.5) as usize).clamp(720, 400_000);
    (0..n)
        .map(|i| {
            let a = i as f32 * std::f32::consts::TAU / n as f32;
            let (s, co) = a.sin_cos();
            let (x, y) = (c[0] + co * r, c[1] + s * r);
            ink.inside(x.floor() as i32, y.floor() as i32)
                .then(|| ink.at_f(x, y))
        })
        .collect()
}

/// A straight cut through the panel's middle, perpendicular to
/// `angle_deg` — the direction a stream's runs travel.
fn line_samples(ink: &Ink, angle_deg: f32) -> Vec<Option<bool>> {
    let (w, h) = (ink.w as f32, ink.h as f32);
    let rad = angle_deg.to_radians();
    let nrm = [-rad.sin(), rad.cos()];
    let half = w.hypot(h) * 0.5;
    let n = (half * 4.0) as usize;
    (0..n)
        .map(|i| {
            let t = -half + i as f32 * 0.5;
            let (x, y) = (w * 0.5 + nrm[0] * t, h * 0.5 + nrm[1] * t);
            ink.inside(x.floor() as i32, y.floor() as i32)
                .then(|| ink.at_f(x, y))
        })
        .collect()
}

/// Ink runs and the gaps between them along a cut, in the cut's own unit.
///
/// A run touching the edge of a continuous stretch is DROPPED: it was cut
/// by the panel border, not by the stroke ending, and counting it would
/// report a frame-clipped rail as a hairline. `circular` closes the loop
/// so a run that straddles angle 0 is still one run.
fn runs(samples: &[Option<bool>], unit: f32, circular: bool) -> (Vec<f32>, Vec<Vec<f32>>) {
    let mut s: Vec<Option<bool>> = samples.to_vec();
    if circular {
        // Start the walk on a boundary — a sample off the panel if there
        // is one, else a white sample — so nothing wraps mid-run.
        let start = s
            .iter()
            .position(|v| v.is_none())
            .or_else(|| s.iter().position(|v| *v == Some(false)));
        match start {
            Some(k) => {
                s.rotate_left(k);
                if s[0].is_some() {
                    // Every sample is on the panel: close the circle by
                    // repeating the (white) first one at the end.
                    s.push(s[0]);
                }
            }
            // A cut that is ink all the way round has no strokes to
            // count, only a solid field.
            None => return (Vec::new(), Vec::new()),
        }
    }
    let mut widths = Vec::new();
    let mut per_stretch: Vec<Vec<f32>> = Vec::new();
    let mut i = 0usize;
    while i < s.len() {
        if s[i].is_none() {
            i += 1;
            continue;
        }
        let lo = i;
        while i < s.len() && s[i].is_some() {
            i += 1;
        }
        let hi = i; // exclusive
        // Runs inside [lo, hi), dropping the ones that touch either end.
        let mut gaps: Vec<f32> = Vec::new();
        let mut j = lo;
        let mut last_run_end: Option<usize> = None;
        while j < hi {
            let v = s[j] == Some(true);
            let k0 = j;
            while j < hi && (s[j] == Some(true)) == v {
                j += 1;
            }
            let touches_edge = k0 == lo || j == hi;
            if v && !touches_edge {
                widths.push((j - k0) as f32 * unit);
                if let Some(e) = last_run_end {
                    gaps.push((k0 - e) as f32 * unit);
                }
                last_run_end = Some(j);
            }
        }
        if !gaps.is_empty() {
            per_stretch.push(gaps);
        }
    }
    (widths, per_stretch)
}

/// Bundle sizes: a run of strokes whose separating gaps all fall in the
/// SMALL of the two gap populations a bundled walk produces.
///
/// It used to threshold at 0.6× the MEDIAN gap, and that could never see
/// a bundle at all (Lane F, Builder B). A walk of `group` lines at pitch
/// `p` followed by one hole of `group_gap × p` emits `group − 1`
/// within-bundle gaps per hole, so for any `group ≥ 3` the median gap IS
/// the within-bundle gap `p` — and no gap is under 0.6 p. Every panel on
/// the sheet reported `1×N` ("almost every stroke a loner", critic 1)
/// while the renderer was walking packs of five.
///
/// The honest question is "are there two gap populations here", so ask it
/// that way: one-dimensional 2-means over the gaps, seeded at the
/// extremes. If the two centroids differ by less than
/// [`BUNDLE_SPLIT`] the set has ONE rhythm — a comb — and every stroke is
/// its own bundle, which is what a `1×N` line should mean. Otherwise the
/// split sits at the geometric mean of the two centroids.
fn bundle_sizes(gaps_per_stretch: &[Vec<f32>]) -> Vec<(usize, usize)> {
    let all: Vec<f32> = gaps_per_stretch.iter().flatten().copied().collect();
    if all.is_empty() {
        return Vec::new();
    }
    let tight = gap_split(&all);
    let mut hist: HashMap<usize, usize> = HashMap::new();
    for gaps in gaps_per_stretch {
        // n gaps = n + 1 strokes on this stretch.
        let mut size = 1usize;
        for g in gaps {
            if *g < tight {
                size += 1;
            } else {
                *hist.entry(size).or_default() += 1;
                size = 1;
            }
        }
        *hist.entry(size).or_default() += 1;
    }
    let mut out: Vec<(usize, usize)> = hist.into_iter().collect();
    out.sort_unstable();
    out
}

/// How far apart the two gap populations have to be before a set counts
/// as bundled. ref-22 puts the between-bundle gap at 4–8 sliver widths
/// against about one inside a bundle, and REFS target 1 asks for 3–8×;
/// 1.6× is well under either, so it admits a hand-wobbled walk without
/// admitting a comb whose pitch merely jitters.
const BUNDLE_SPLIT: f32 = 1.6;

/// The gap width that separates "inside a bundle" from "between
/// bundles", or 0 if the set has only one rhythm. See [`bundle_sizes`].
fn gap_split(gaps: &[f32]) -> f32 {
    let (lo0, hi0) = gaps.iter().fold((f32::INFINITY, 0.0f32), |(a, b), g| {
        (a.min(*g), b.max(*g))
    });
    if !(lo0 > 0.0) || hi0 < lo0 * BUNDLE_SPLIT {
        return 0.0;
    }
    let (mut a, mut b) = (lo0, hi0);
    for _ in 0..40 {
        let mid = (a + b) * 0.5;
        let (mut sa, mut na, mut sb, mut nb) = (0.0f32, 0usize, 0.0f32, 0usize);
        for g in gaps {
            if *g <= mid {
                sa += *g;
                na += 1;
            } else {
                sb += *g;
                nb += 1;
            }
        }
        if na == 0 || nb == 0 {
            return 0.0;
        }
        let (na2, nb2) = (sa / na as f32, sb / nb as f32);
        if (na2 - a).abs() < 1e-4 && (nb2 - b).abs() < 1e-4 {
            break;
        }
        (a, b) = (na2, nb2);
    }
    if b < a * BUNDLE_SPLIT {
        0.0
    } else {
        (a * b).sqrt()
    }
}

/// A connected black blob this small is dirt on the scan, not a mark:
/// 20 px at 600 dpi is under 0.09 mm² — a fifth of a millimetre square.
/// Critic 1 found "isolated 1–3 px black dots inside the white hole of
/// `solid-flash`" and asked for components under about 6 px to go; this
/// is that, with room for a diagonal 3-px hook.
const SPECK_PX: usize = 20;

/// Marks that belong to nothing.
///
/// Two things, counted together because both are the same complaint —
/// "nothing may float" (critic 1, fix 3):
///
/// - **specks**, on any kind: a black component of at most [`SPECK_PX`]
///   pixels. On paper these read as dirt.
/// - **islands**, on a solid flash only: a black component that touches
///   no panel border and is not the field itself. ref-20 and ref-21 are
///   one unbroken black mass; critic 1 found "a detached triangular
///   island that never joins the black mass" at 1:1. A ウニフラ is made
///   OF separate teeth, so the rule cannot apply to it.
///
/// Target 0 on both counts.
pub fn fragments(ink: &Ink, solid: bool) -> usize {
    let (w, h) = (ink.w as i32, ink.h as i32);
    let mut seen = vec![false; ink.w * ink.h];
    let mut stack: Vec<i32> = Vec::new();
    // (area, touches the border) per component, so the biggest island
    // can be excused as the field itself.
    let mut islands: Vec<usize> = Vec::new();
    let mut specks = 0usize;
    for sy in 0..h {
        for sx in 0..w {
            let s0 = (sy * w + sx) as usize;
            if !ink.at(sx, sy) || seen[s0] {
                continue;
            }
            seen[s0] = true;
            stack.clear();
            stack.push(s0 as i32);
            let (mut area, mut edge) = (0usize, false);
            while let Some(i) = stack.pop() {
                let (x, y) = (i % w, i / w);
                area += 1;
                edge |= x == 0 || y == 0 || x == w - 1 || y == h - 1;
                for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)]
                {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w || ny >= h {
                        continue;
                    }
                    let j = (ny * w + nx) as usize;
                    if ink.at(nx, ny) && !seen[j] {
                        seen[j] = true;
                        stack.push(j as i32);
                    }
                }
            }
            if area <= SPECK_PX {
                specks += 1;
            } else if solid && !edge {
                islands.push(area);
            }
        }
    }
    // The field itself, if it happens not to reach a border, is not an
    // island — drop the largest one.
    if !islands.is_empty() {
        islands.sort_unstable();
        islands.pop();
    }
    specks + islands.len()
}

/// The ink percentage of the emptiest corner box: 12 % of the panel each
/// way, at each of the four corners. REFS' picture of a ベタフラ is "an
/// all-black rectangle with a white oval burst punched through", so a
/// solid row reads 100 here and critic 1's `solid-flash` read 0.
fn corner_ink(ink: &Ink) -> f32 {
    let (bw, bh) = ((ink.w * 12 / 100).max(1), (ink.h * 12 / 100).max(1));
    let mut worst = 100.0f32;
    for (ox, oy) in [(0, 0), (ink.w - bw, 0), (0, ink.h - bh), (ink.w - bw, ink.h - bh)] {
        let mut n = 0usize;
        for y in oy..oy + bh {
            for x in ox..ox + bw {
                if ink.at(x as i32, y as i32) {
                    n += 1;
                }
            }
        }
        worst = worst.min(100.0 * n as f32 / (bw * bh) as f32);
    }
    worst
}

/// Ink percentage per panel quarter, reading order (TL, TR, BL, BR).
fn quarter_ink(ink: &Ink) -> [f32; 4] {
    let (hw, hh) = (ink.w / 2, ink.h / 2);
    let mut n = [0usize; 4];
    let mut d = [0usize; 4];
    for y in 0..ink.h {
        for x in 0..ink.w {
            let q = (y >= hh) as usize * 2 + (x >= hw) as usize;
            d[q] += 1;
            if ink.at(x as i32, y as i32) {
                n[q] += 1;
            }
        }
    }
    std::array::from_fn(|i| {
        if d[i] == 0 {
            0.0
        } else {
            100.0 * n[i] as f32 / d[i] as f32
        }
    })
}

/// The radial sweep `REFS.md` measures every reference with: 2160 rays
/// out of the centre, recording where each FIRST meets ink and where it
/// LAST does. In pixels.
///
/// The first-ink radius is the hole of a 集中線 or a ウニフラ and, because
/// a ベタフラ renders as a black field around a white burst, the white
/// region of one — the same number the pack quotes ("hole radius runs
/// 51px to 132px, σ 17px = 20 % of the mean").
///
/// The last-ink radius is the outer silhouette, and it is only returned
/// when it IS a silhouette: a ray whose last ink sits on the frame was
/// cut by the panel, not by the effect, and averaging those in measures
/// the rectangle. Under half the rays clear of the border and the outline
/// is reported as absent rather than as a number that means the frame.
struct Sweep {
    inner: Vec<f32>,
    outer: Vec<f32>,
    /// The outer radius of the WHITE region connected to the burst's
    /// middle, per ray — the shape a ベタフラ actually is (see the call
    /// site). Same length and order as `inner`; all zero when the flood
    /// was not asked for.
    white: Vec<f32>,
    /// How many rays ended clear of the panel border, out of `inner.len()`.
    free: usize,
}

fn sweep(ink: &Ink, c: [f32; 2], want_white: bool) -> Sweep {
    const RAYS: usize = 2160;
    // Far enough to leave the panel from any centre, inside or outside it.
    let reach = (ink.w as f32).hypot(ink.h as f32) * 2.0;
    let mut out = Sweep {
        inner: Vec::new(),
        outer: Vec::new(),
        white: Vec::new(),
        free: 0,
    };
    let mut dirs: Vec<(f32, f32)> = Vec::with_capacity(RAYS);
    for i in 0..RAYS {
        let a = i as f32 * std::f32::consts::TAU / RAYS as f32;
        let (s, co) = a.sin_cos();
        let (mut first, mut last) = (None, 0.0f32);
        let mut r = 1.0f32;
        while r < reach {
            let (x, y) = (c[0] + co * r, c[1] + s * r);
            if ink.at_f(x, y) {
                first.get_or_insert(r);
                last = r;
            }
            r += 1.0;
        }
        if let Some(f) = first {
            out.inner.push(f);
            out.outer.push(last);
            dirs.push((co, s));
            // Clear of the border by more than the erosion radius, so the
            // end is the effect's and not the frame's.
            let (x, y) = (c[0] + co * last, c[1] + s * last);
            if x > 8.0 && y > 8.0 && x < ink.w as f32 - 8.0 && y < ink.h as f32 - 8.0 {
                out.free += 1;
            }
        }
    }
    out.white = vec![0.0; out.inner.len()];
    if want_white && !out.inner.is_empty() {
        // Flood the white the burst's middle is made of, seeded halfway
        // out to each ray's first ink — inside the core by construction,
        // and it works for an off-panel centre too, where the middle is
        // not a pixel we could name.
        let (w, h) = (ink.w, ink.h);
        let mut seen = vec![false; w * h];
        let mut stack: Vec<usize> = Vec::new();
        let push = |x: i32, y: i32, seen: &mut Vec<bool>, stack: &mut Vec<usize>| {
            if ink.inside(x, y) && !ink.at(x, y) {
                let i = y as usize * w + x as usize;
                if !seen[i] {
                    seen[i] = true;
                    stack.push(i);
                }
            }
        };
        for (k, (co, s)) in dirs.iter().enumerate() {
            // Several fractions, because for an OFF-PANEL centre the
            // halfway point can itself be off the panel and would seed
            // nothing at all.
            for f in [0.5f32, 0.7, 0.85, 0.95] {
                let r = out.inner[k] * f;
                push(
                    (c[0] + co * r).floor() as i32,
                    (c[1] + s * r).floor() as i32,
                    &mut seen,
                    &mut stack,
                );
            }
        }
        while let Some(i) = stack.pop() {
            let (x, y) = ((i % w) as i32, (i / w) as i32);
            push(x - 1, y, &mut seen, &mut stack);
            push(x + 1, y, &mut seen, &mut stack);
            push(x, y - 1, &mut seen, &mut stack);
            push(x, y + 1, &mut seen, &mut stack);
        }
        for (k, (co, s)) in dirs.iter().enumerate() {
            let mut r = out.inner[k] * 0.5;
            let mut best = r;
            while r < reach {
                // ±1.5 px ACROSS the ray, because a white sliver is
                // radial and so is the ray: at 600 dpi a sliver is a
                // pixel wide near its tip while the rays are 3 px apart
                // out there, so a bare point sample walks straight past
                // most of them and the reading collapses onto the core.
                // REFS measured this on ~900 px photographs where the
                // same ink is several pixels wide; the tolerance puts a
                // 600 dpi render on that footing instead of rewarding it
                // for being finer.
                let hit = [-1.5f32, 0.0, 1.5].iter().any(|o| {
                    let (x, y) = (
                        (c[0] + co * r - s * o).floor() as i32,
                        (c[1] + s * r + co * o).floor() as i32,
                    );
                    ink.inside(x, y) && seen[y as usize * w + x as usize]
                });
                if hit {
                    best = r;
                }
                r += 1.0;
            }
            out.white[k] = best;
        }
    }
    out
}

/// Mean and σ of a sample, or `None` if there is not enough of it.
fn mean_sigma(v: &[f32]) -> Option<(f32, f32)> {
    if v.len() < 16 {
        return None;
    }
    let mean = v.iter().sum::<f32>() / v.len() as f32;
    let var = v.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>() / v.len() as f32;
    Some((mean, var.sqrt()))
}

// --- the round-cap detector -------------------------------------------

/// The erosion radius: a stroke has to be about `2·CORE_R + 1` px wide
/// to leave a core. Thinner than that and there is no cap to see — the
/// half-pixel floor holds a hairline to one pixel and it ends when it
/// ends.
///
/// It was 3 (a 7 px floor) until Lane F Builder B. Critic 1 named, at
/// 1:1, "two black rectangles, blunt at both ends" about 9 px across and
/// a square-cut accent bar in `sea-urchin-flash-crop`, none of which the
/// probe reported. 2 puts the floor at 5 px = 0.21 mm at 600 dpi, which
/// is a mark a G-pen can make and a reader can see the end of.
const CORE_R: i32 = 2;

/// How much of the neighbourhood round a stroke end has to be ink before
/// that end counts as BURIED — inside a black mass, where no reader can
/// see whether it was a point or a cut.
///
/// This replaces the old "core zone" excuse, which skipped every end
/// inside `hole_mean + 2σ` of a flash's centre. On `sea-urchin-flash`
/// that zone reached 18.6 mm while the hole itself was 13 mm, so it
/// swallowed a third of the ring — including both of the blunt
/// rectangles critic 1 found by eye. Burial is the property that actually
/// excuses a straight cut: ref-17's fat stroke-starts stack against the
/// hole "with visible gaps and overlaps" into a band, and ref-21's white
/// slivers run into the white core. An end in open paper is a defect
/// wherever it sits.
const BURIED_INK: f32 = 0.62;

/// Ends that stop dead instead of running out to a point.
///
/// Round 2's probe, re-implemented from the recipe in
/// `docs/plans/2026-09-06-gauntlet-REPORT.md` ("Round caps across the
/// sheet"): erode to the cores of strokes at least 6 px wide, take each
/// core's outermost point along its OWN axis, and march outward from
/// there through the full ink, allowing ±2 px sideways because a 1 px
/// white sliver between two near-merged rails is not a stroke end. A
/// needle keeps inking for a long way past its core; a round or blunt cap
/// reaches only about its own radius and stops.
///
/// Ends at the panel border are not counted: a stroke the frame cuts is
/// what a printed effect line does on purpose. Neither is a BURIED end —
/// one whose neighbourhood is [`BURIED_INK`] ink or more, i.e. one that
/// sits inside a black mass where there is no end to see.
pub fn round_caps(ink: &Ink) -> usize {
    let core = erode(ink);
    let (w, h) = (ink.w as i32, ink.h as i32);
    let mut seen = vec![false; core.len()];
    let mut caps = 0usize;
    let mut stack: Vec<i32> = Vec::new();
    let mut blob: Vec<(i32, i32)> = Vec::new();
    for sy in 0..h {
        for sx in 0..w {
            let s0 = (sy * w + sx) as usize;
            if !core[s0] || seen[s0] {
                continue;
            }
            blob.clear();
            stack.clear();
            stack.push(s0 as i32);
            seen[s0] = true;
            while let Some(i) = stack.pop() {
                let (x, y) = (i % w, i / w);
                blob.push((x, y));
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let (nx, ny) = (x + dx, y + dy);
                        if nx < 0 || ny < 0 || nx >= w || ny >= h {
                            continue;
                        }
                        let j = (ny * w + nx) as usize;
                        if core[j] && !seen[j] {
                            seen[j] = true;
                            stack.push(j as i32);
                        }
                    }
                }
            }
            caps += blob_caps(ink, &blob);
        }
    }
    caps
}

/// Erode with a square of side `2·CORE_R + 1`, separably and with a
/// running count, so the whole pass is two linear scans rather than 49
/// tests per pixel — this runs on every panel of the sheet inside a CI
/// test.
fn erode(ink: &Ink) -> Vec<bool> {
    let (w, h) = (ink.w, ink.h);
    let r = CORE_R as usize;
    let mut horiz = vec![false; w * h];
    for y in 0..h {
        // `white` = how many non-ink pixels the window holds.
        let mut white = 0i32;
        for x in 0..(r.min(w)) {
            if !ink.at(x as i32, y as i32) {
                white += 1;
            }
        }
        for x in 0..w {
            if x + r < w && !ink.at((x + r) as i32, y as i32) {
                white += 1;
            }
            // A window that runs off the panel is not a core.
            horiz[y * w + x] = white == 0 && x >= r && x + r < w;
            if x >= r && !ink.at((x - r) as i32, y as i32) {
                white -= 1;
            }
        }
    }
    let mut out = vec![false; w * h];
    for x in 0..w {
        let mut white = 0i32;
        for y in 0..(r.min(h)) {
            if !horiz[y * w + x] {
                white += 1;
            }
        }
        for y in 0..h {
            if y + r < h && !horiz[(y + r) * w + x] {
                white += 1;
            }
            out[y * w + x] = white == 0 && y >= r && y + r < h;
            if y >= r && !horiz[(y - r) * w + x] {
                white -= 1;
            }
        }
    }
    out
}

/// One core's two ends, judged.
fn blob_caps(ink: &Ink, blob: &[(i32, i32)]) -> usize {
    if blob.len() < 40 {
        return 0;
    }
    let n = blob.len() as f32;
    let (mut mx, mut my) = (0.0f32, 0.0f32);
    for (x, y) in blob {
        mx += *x as f32;
        my += *y as f32;
    }
    mx /= n;
    my /= n;
    let (mut sxx, mut sxy, mut syy) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in blob {
        let (dx, dy) = (*x as f32 - mx, *y as f32 - my);
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
    }
    // Principal axis of a 2×2 covariance, closed form.
    let t = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let u = [t.cos(), t.sin()];
    let mut lo = (f32::INFINITY, (0i32, 0i32));
    let mut hi = (f32::NEG_INFINITY, (0i32, 0i32));
    for (x, y) in blob {
        let p = (*x as f32 - mx) * u[0] + (*y as f32 - my) * u[1];
        if p < lo.0 {
            lo = (p, (*x, *y));
        }
        if p > hi.0 {
            hi = (p, (*x, *y));
        }
    }
    let len = (hi.0 - lo.0).max(1.0);
    // The core's half-width, and then the STROKE's: erosion took CORE_R
    // off each side.
    let half = n / len * 0.5;
    let stroke_half = half + CORE_R as f32;
    // A ring, a blob or a solid field is not a stroke and has no ends to
    // judge. Two-and-a-half to one is the loosest thing that still reads
    // as a mark rather than as a mass.
    if len < 2.5 * (2.0 * half).max(1.0) {
        return 0;
    }
    let mut caps = 0;
    for (end, dir) in [(hi.1, 1.0f32), (lo.1, -1.0f32)] {
        // An end swallowed by a black mass is not an end anyone sees.
        if buried(ink, end, stroke_half * 3.0 + 6.0) {
            continue;
        }
        if march(ink, end, [u[0] * dir, u[1] * dir], stroke_half) {
            caps += 1;
        }
    }
    caps
}

/// Is this end sunk in ink? The ink fraction of the disc of radius `rad`
/// around it, against [`BURIED_INK`]. Pixels off the panel count as ink:
/// an end in the corner of a solid field is as buried as one in the
/// middle of it.
fn buried(ink: &Ink, at: (i32, i32), rad: f32) -> bool {
    let r = rad.max(3.0) as i32;
    let (mut n, mut d) = (0usize, 0usize);
    for dy in -r..=r {
        for dx in -r..=r {
            if dx * dx + dy * dy > r * r {
                continue;
            }
            d += 1;
            let (x, y) = (at.0 + dx, at.1 + dy);
            if !ink.inside(x, y) || ink.at(x, y) {
                n += 1;
            }
        }
    }
    d > 0 && n as f32 / d as f32 >= BURIED_INK
}

/// March outward from a core's end. `true` = the ink stops within about
/// its own width, i.e. a cap; `false` = it keeps going (a needle), or it
/// leaves the panel (the frame cut it).
fn march(ink: &Ink, from: (i32, i32), dir: [f32; 2], stroke_half: f32) -> bool {
    let perp = [-dir[1], dir[0]];
    // Far enough that a real needle always clears it, near enough that a
    // cap never does.
    let limit = (stroke_half * 3.0 + 8.0) as i32;
    let mut reach = 0i32;
    for k in 1..=limit {
        let (cx, cy) = (
            from.0 as f32 + dir[0] * k as f32,
            from.1 as f32 + dir[1] * k as f32,
        );
        if !ink.inside(cx.floor() as i32, cy.floor() as i32) {
            return false;
        }
        // ±2 px sideways: a 1 px white sliver between two near-merged
        // rails is not a stroke end (round 2 read five of those as caps).
        let hit = (-2..=2).any(|j| {
            let (x, y) = (cx + perp[0] * j as f32, cy + perp[1] * j as f32);
            ink.at(x.floor() as i32, y.floor() as i32)
        });
        if !hit {
            break;
        }
        reach = k;
    }
    (reach as f32) < stroke_half + 3.0
}
