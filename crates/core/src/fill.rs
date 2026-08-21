//! Bucket fill with gap closing — the manga essential (ベタ and flatting are
//! unpaintable by hand at print resolution). PLAN.md phase 1 item 6.
//!
//! Algorithm (the Krita recipe, simplified):
//! 1. Sample source pixels (visible composite, or the active layer alone) over
//!    white — manga mental model: empty canvas is paper.
//! 2. Barrier = pixels farther from the seed colour than `tolerance`.
//! 3. **Gap closing**: dilate the barrier by `gap_close_px` so line gaps up to
//!    ~2× that seal shut, then flood-fill from the seed.
//! 4. Recover: dilate the filled region back by the same amount, but never
//!    across an *original* barrier pixel — the fill hugs real lines again
//!    without leaking through the gap.
//! 5. `expand_px`: unconditional dilation (CSP's "area scaling") so fills tuck
//!    under anti-aliased lineart. It is SIGNED — negative erodes instead, which
//!    is CSP's underfill (the fill pulls back inside the area).
//! 6. Clip to the selection, then write the colour opaquely into the active
//!    layer inside an undo op.
//!
//! The same machinery aims three other ways: [`magic_select`] selects instead
//! of painting (the wand), [`magic_select_path`] selects every pocket a
//! freehand path crosses (SE-020 shrink-select), and [`enclose_and_fill`]
//! PAINTS that same pocket set (FI-003 — the flatting workhorse).

use crate::blend::f32_to_fix15;
use crate::doc::Document;
use crate::export::{self, Background};
use crate::tile::{TILE_SIZE, TileIdx};

#[derive(Clone, Copy, Debug)]
pub struct FillOpts {
    /// 0..1 colour distance (max RGB channel difference) that still counts as
    /// "the same area".
    pub tolerance: f32,
    /// Close line gaps up to roughly 2× this many pixels.
    pub gap_close_px: u32,
    /// CSP's SIGNED area scaling (FI-016). Positive grows the final region
    /// under the lineart by that many pixels (overfill); negative erodes it
    /// by that many (underfill — the fill pulls back off the line).
    pub expand_px: i32,
    /// What the flood samples (CSP 参照): the visible composite, the active
    /// layer alone, or the reference layer.
    pub refer: FillRefer,
    /// Sample draft layers (CSP 下書き) when referring to all layers.
    pub refer_drafts: bool,
    /// FI-022 (CSP 画像の縁を参照, "Refer to image border"): treat the
    /// canvas's outer perimeter as a drawn border line, so a fill that
    /// escapes into the margin cannot run all the way round the page.
    /// Defaults OFF — the behaviour every earlier build had.
    pub refer_border: bool,
    /// Measure `gap_close_px` and `expand_px` from the lineart around the
    /// click instead of taking them from the fields ([`measure_auto`]).
    /// Defaults OFF: with it off every field above is honoured verbatim,
    /// so a build that never touches the switch fills pixel-identically.
    pub auto: bool,
}

/// What [`measure_auto`] read off the artwork, for the status line and the
/// greyed-out Tool Property rows. `gap_close_px`/`expand_px` are the values
/// the fill actually ran with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoFill {
    /// Median lineart thickness, in px, across the flooded area's boundary.
    pub line_px: f32,
    pub gap_close_px: u32,
    pub expand_px: i32,
    /// Boundary crossings the median came from — 0 never happens (a
    /// measurement with no samples is reported as `None`, not as a guess).
    pub samples: u32,
}

/// CSP fill/wand 参照 modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FillRefer {
    /// The visible composite ("refer other layers").
    #[default]
    All,
    /// The active layer only ("editing layer").
    Active,
    /// The reference layer, even when hidden (参照レイヤー).
    Reference,
}

impl Default for FillOpts {
    fn default() -> Self {
        Self {
            tolerance: 0.08,
            gap_close_px: 2,
            expand_px: 1,
            refer: FillRefer::All,
            refer_drafts: true,
            refer_border: false,
            auto: false,
        }
    }
}

/// Steps 1–5 of the fill recipe: the flooded region as a canvas-sized bool
/// mask. `None` when the seed is out of bounds. Shared by [`bucket_fill`] and
/// [`magic_select`] (the Auto-select wand is a fill that selects instead of
/// painting).
pub fn flood_region(doc: &Document, seed: (i32, i32), opts: &FillOpts) -> Option<Vec<bool>> {
    flood_region_measured(doc, seed, opts).map(|(region, _)| region)
}

/// [`flood_region`] plus what `opts.auto` measured (`None` when auto is off,
/// or when the artwork gave nothing measurable and the manual numbers stood).
/// One source composite for both the measuring pass and the real flood.
pub fn flood_region_measured(
    doc: &Document,
    seed: (i32, i32),
    opts: &FillOpts,
) -> Option<(Vec<bool>, Option<AutoFill>)> {
    let (w, h) = (doc.size.0 as usize, doc.size.1 as usize);
    let (sx, sy) = seed;
    if sx < 0 || sy < 0 || sx as usize >= w || sy as usize >= h {
        return None;
    }
    let src = source_pixels(doc, opts);
    debug_assert_eq!(src.len(), w * h);
    let start = sy as usize * w + sx as usize;
    let (opts, auto) = resolve_with_src(&src, w, h, start, opts);
    Some((region_from_src(&src, w, h, start, &opts), auto))
}

/// Resolve `opts.auto` into concrete numbers without flooding for real —
/// for callers that run several floods off one click (enclose-and-fill), so
/// every flood in the gesture shares ONE measurement. Returns the options to
/// use (always `auto: false`) and what was measured.
pub fn resolve_auto(
    doc: &Document,
    seed: (i32, i32),
    opts: &FillOpts,
) -> (FillOpts, Option<AutoFill>) {
    let (w, h) = (doc.size.0 as usize, doc.size.1 as usize);
    let (sx, sy) = seed;
    if !opts.auto || sx < 0 || sy < 0 || sx as usize >= w || sy as usize >= h {
        return (
            FillOpts {
                auto: false,
                ..*opts
            },
            None,
        );
    }
    let src = source_pixels(doc, opts);
    resolve_with_src(&src, w, h, sy as usize * w + sx as usize, opts)
}

/// 1. Source pixels, straight RGB over white paper.
fn source_pixels(doc: &Document, opts: &FillOpts) -> Vec<[u8; 3]> {
    match opts.refer {
        FillRefer::Active => active_over_white(doc),
        FillRefer::Reference => {
            // The reference SET (RF-001), composited bottom→top — the
            // layers' eye state does not matter.
            let refs = doc.reference_layers();
            if refs.is_empty() {
                // No reference set: fall back to what you see.
                export::composite_for_fill(doc, Background::White, opts.refer_drafts)
                    .pixels()
                    .map(|p| [p.0[0], p.0[1], p.0[2]])
                    .collect()
            } else {
                layers_over_white(doc, &refs)
            }
        }
        FillRefer::All => export::composite_for_fill(doc, Background::White, opts.refer_drafts)
            .pixels()
            .map(|p| [p.0[0], p.0[1], p.0[2]])
            .collect(),
    }
}

/// 2. Barrier mask from the seed colour: pixels farther than `tolerance`.
fn barrier_from(src: &[[u8; 3]], seed_px: [u8; 3], tolerance: f32) -> Vec<bool> {
    let tol = (tolerance.clamp(0.0, 1.0) * 255.0) as i16;
    src.iter()
        .map(|p| {
            let d = (p[0] as i16 - seed_px[0] as i16)
                .abs()
                .max((p[1] as i16 - seed_px[1] as i16).abs())
                .max((p[2] as i16 - seed_px[2] as i16).abs());
            d > tol
        })
        .collect()
}

/// Steps 2–5 against an already-sampled source. `opts.auto` is ignored here:
/// resolution happens once, above.
fn region_from_src(
    src: &[[u8; 3]],
    w: usize,
    h: usize,
    start: usize,
    opts: &FillOpts,
) -> Vec<bool> {
    let n = w * h;
    let mut barrier = barrier_from(src, src[start], opts.tolerance);
    let barrier_orig = barrier.clone();

    // 2b. FI-022: the page's outer perimeter counts as a drawn border line
    // (CSP's own words). Walled in the FLOOD barrier only, never in
    // `barrier_orig` — the step-4 recovery below is then free to give the
    // rim strip back, so the switch costs a fill nothing except the
    // escape route it exists to close. A seed ON the rim lands on a
    // barrier and takes the fallback branch, i.e. it fills unwalled;
    // that is the same graceful degradation gap-closing already has.
    if opts.refer_border {
        for x in 0..w {
            barrier[x] = true;
            barrier[(h - 1) * w + x] = true;
        }
        for y in 0..h {
            barrier[y * w] = true;
            barrier[y * w + w - 1] = true;
        }
    }

    // 3. Fatten the barrier to seal gaps.
    for _ in 0..opts.gap_close_px {
        barrier = dilate(&barrier, w, h);
    }

    // Flood (4-connected BFS) over non-barrier. A seed that landed on the
    // FATTENED barrier means gap closing ate it: fall back to the seed's own
    // contiguous same-colour blob, flooded against the real lines only.
    let mut region = if barrier[start] {
        flood(&barrier_orig, w, h, start)
    } else {
        flood(&barrier, w, h, start)
    };

    // 4. Recover the margin the fat barrier stole — but never cross real lines.
    for _ in 0..opts.gap_close_px {
        let grown = dilate(&region, w, h);
        for i in 0..n {
            if grown[i] && !barrier_orig[i] {
                region[i] = true;
            }
        }
    }
    // 5. Signed area scaling (FI-016). Positive = overfill, tucking the
    // region under the anti-aliased lineart; negative = underfill, eroding
    // it so a hard-edged fill does not touch the line at all. Erosion is
    // dilation of the complement, the same identity `Selection::shrink`
    // uses — and it inherits that identity's edge rule: `dilate` clamps at
    // the canvas border, so a region running off the page does not pull
    // back from the page edge, only from real boundaries.
    for _ in 0..opts.expand_px.max(0) {
        region = dilate(&region, w, h);
    }
    for _ in 0..(-opts.expand_px).max(0) {
        region = erode(&region, w, h);
    }
    region
}

/// 4-connected BFS over everything `blocked` does not mark.
fn flood(blocked: &[bool], w: usize, h: usize, start: usize) -> Vec<bool> {
    let mut region = vec![false; w * h];
    let mut queue = std::collections::VecDeque::from([start]);
    region[start] = true;
    while let Some(i) = queue.pop_front() {
        let (x, y) = (i % w, i / w);
        let mut push = |j: usize| {
            if !region[j] && !blocked[j] {
                region[j] = true;
                queue.push_back(j);
            }
        };
        if x > 0 {
            push(i - 1);
        }
        if x + 1 < w {
            push(i + 1);
        }
        if y > 0 {
            push(i - w);
        }
        if y + 1 < h {
            push(i + w);
        }
    }
    region
}

// --- auto gap & fringe ---------------------------------------------------
//
// The seven numeric options are the reason bucket fill is a tuning chore: the
// two that actually change per drawing are gap closing and area scaling, and
// both follow from ONE property of the artwork — how thick the lines are. So
// measure that and derive them, instead of asking.

/// A barrier run longer than this is not a line — it is ベタ, a filled panel,
/// or the page margin. Such samples are dropped rather than averaged in.
const MAX_LINE_PX: u32 = 64;

/// Boundary crossings the median is taken over. Bounded so the measurement
/// costs the same on a 600 px doodle and a 300 dpi B4 page.
const MAX_SAMPLES: usize = 256;

/// Measure the lineart thickness around the area `seed` falls in, and derive
/// gap closing and area scaling from it. Ignores `opts.auto` — this IS the
/// measurement; the flag only decides whether a fill calls it.
///
/// `None` when nothing measurable bounded the area (a blank canvas, or an
/// area walled only by solid black): the caller then keeps the manual
/// numbers rather than inventing one.
pub fn measure_auto(doc: &Document, seed: (i32, i32), opts: &FillOpts) -> Option<AutoFill> {
    let (w, h) = (doc.size.0 as usize, doc.size.1 as usize);
    let (sx, sy) = seed;
    if sx < 0 || sy < 0 || sx as usize >= w || sy as usize >= h {
        return None;
    }
    let src = source_pixels(doc, opts);
    measure_with_src(&src, w, h, sy as usize * w + sx as usize, opts)
}

fn measure_with_src(
    src: &[[u8; 3]],
    w: usize,
    h: usize,
    start: usize,
    opts: &FillOpts,
) -> Option<AutoFill> {
    // The measuring flood is the raw one: gap closing and area scaling are
    // exactly what is being decided, so neither may colour the region whose
    // boundary decides them. FI-022's rim wall is off for the same reason —
    // the page edge is not a stroke and must not be measured as one.
    // The barrier is relative to the seed COLOUR, so a click on the lineart
    // measures the paper between the strokes — the fill family's existing
    // seed honesty (clicking ink floods ink), not a case to special-case.
    let barrier = barrier_from(src, src[start], opts.tolerance);
    let region = flood(&barrier, w, h, start);
    let (line_px, samples) = median_line_px(&barrier, &region, w, h)?;
    Some(AutoFill {
        line_px,
        // A gap where two strokes fail to meet is about one stroke wide, and
        // step 3 seals gaps up to ~2× this — so one line width is the sane
        // multiple. Capped at the manual slider's own range: a measured
        // value the user cannot dial in by hand would be a lie in a
        // read-only field, and past ~8 px the dilation passes get expensive.
        gap_close_px: line_px.round().clamp(1.0, 8.0) as u32,
        // The halo is the line's anti-aliased skirt, which scales with the
        // pen. Half the line width is the honest ceiling: expansion is
        // unconditional (step 5 crosses barriers), so anything ≥ half a line
        // width walks through the line into the neighbouring area.
        expand_px: ((line_px / 2.0).floor() as i32).clamp(1, 4),
        samples,
    })
}

fn resolve_with_src(
    src: &[[u8; 3]],
    w: usize,
    h: usize,
    start: usize,
    opts: &FillOpts,
) -> (FillOpts, Option<AutoFill>) {
    let base = FillOpts {
        auto: false,
        ..*opts
    };
    if !opts.auto {
        return (base, None);
    }
    match measure_with_src(src, w, h, start, opts) {
        Some(a) => (
            FillOpts {
                gap_close_px: a.gap_close_px,
                expand_px: a.expand_px,
                ..base
            },
            Some(a),
        ),
        // Nothing measurable: the manual numbers stand, and the caller says
        // so. Silently filling with invented values is the failure mode this
        // feature exists to remove.
        None => (base, None),
    }
}

/// Median thickness of the barrier the region touches, plus the sample count.
/// Each sample is one boundary pixel's THINNEST axis crossing: a scanline
/// through a slanted line reads long, the perpendicular one reads true, and
/// the minimum of the two available axes is the closer of the pair.
fn median_line_px(barrier: &[bool], region: &[bool], w: usize, h: usize) -> Option<(f32, u32)> {
    let mut boundary: Vec<usize> = Vec::new();
    for (i, &r) in region.iter().enumerate() {
        if !r {
            continue;
        }
        let (x, y) = (i % w, i / w);
        let touches = (x > 0 && barrier[i - 1])
            || (x + 1 < w && barrier[i + 1])
            || (y > 0 && barrier[i - w])
            || (y + 1 < h && barrier[i + w]);
        if touches {
            boundary.push(i);
        }
    }
    if boundary.is_empty() {
        return None;
    }
    let stride = boundary.len().div_ceil(MAX_SAMPLES);
    let mut runs: Vec<u32> = Vec::new();
    for &i in boundary.iter().step_by(stride) {
        let (x, y) = (i % w, i / w);
        let thinnest = [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)]
            .into_iter()
            .filter_map(|(dx, dy)| barrier_run(barrier, w, h, x, y, dx, dy))
            .min();
        if let Some(t) = thinnest {
            runs.push(t);
        }
    }
    if runs.is_empty() {
        return None;
    }
    runs.sort_unstable();
    Some((runs[runs.len() / 2] as f32, runs.len() as u32))
}

/// Length of the unbroken barrier run starting one step from `(x, y)` along
/// `(dx, dy)`. `None` when that step is not barrier at all, when the run
/// leaves the canvas (an open edge measures nothing), or when it exceeds
/// [`MAX_LINE_PX`].
fn barrier_run(
    barrier: &[bool],
    w: usize,
    h: usize,
    x: usize,
    y: usize,
    dx: i32,
    dy: i32,
) -> Option<u32> {
    let (mut cx, mut cy) = (x as i32 + dx, y as i32 + dy);
    let mut len = 0u32;
    loop {
        if cx < 0 || cy < 0 || cx as usize >= w || cy as usize >= h {
            return None;
        }
        if !barrier[cy as usize * w + cx as usize] {
            return (len > 0).then_some(len);
        }
        len += 1;
        if len > MAX_LINE_PX {
            return None;
        }
        cx += dx;
        cy += dy;
    }
}

/// CSP Auto select (magic wand): flood from `seed` with the fill machinery —
/// same tolerance/gap-closing/expand semantics — but return a [`Selection`]
/// instead of painting. `None` when the seed is out of bounds.
pub fn magic_select(
    doc: &Document,
    seed: (i32, i32),
    opts: &FillOpts,
) -> Option<crate::selection::Selection> {
    let region = flood_region(doc, seed, opts)?;
    let w = doc.size.0 as usize;
    Some(crate::selection::Selection::from_mask(doc, &region, w))
}

/// The shared geometry behind SE-020 shrink-select and FI-003 enclose-and-fill:
/// a freehand path seeds a UNION of floods, and the canvas-edge-reachable OUTER
/// space is subtracted, so what comes back is every CLOSED pocket the path
/// crossed. Seeds landing inside an already-covered pocket are SKIPPED, so the
/// cost is one flood per distinct pocket, not per seed point. Returns the
/// canvas-sized mask and how many CLOSED areas it holds (for the status
/// line — floods wholly inside the subtracted outer space do not count);
/// `None` when nothing enclosed was found.
fn enclosed_pockets(
    doc: &Document,
    seeds: &[(i32, i32)],
    opts: &FillOpts,
) -> Option<(Vec<bool>, u32)> {
    let (w, h) = (doc.size.0 as usize, doc.size.1 as usize);
    if w == 0 || h == 0 || seeds.is_empty() {
        return None;
    }
    // Auto is measured ONCE for the whole gesture, from the first seed that
    // lands on the page. Per-seed measurement would flood every pocket twice
    // and — worse — let the outer set and the pockets disagree about which
    // gaps are sealed, which is exactly the asymmetry FI-016 already had to
    // be fixed for below.
    let opts = &seeds
        .iter()
        .find(|&&(x, y)| x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h)
        .map_or(
            FillOpts {
                auto: false,
                ..*opts
            },
            |&s| resolve_auto(doc, s, opts).0,
        );
    // The OUTER space: everything empty-reachable from the canvas edges.
    // CSP's semantics — you draw AROUND the drawing, and only the CLOSED
    // areas inside it select — so the region the path travels through is
    // excluded by construction. (A fully-bordered page has no outer
    // space; then nothing is excluded — recorded as the v1 edge case.)
    // FI-022 is forced OFF here: "the page rim is a line" would wall the
    // corner seeds in and there would BE no outer space to subtract.
    // FI-016's area scaling is forced off too: the outer set only says
    // which space is edge-reachable, and a dilated outer eats exactly the
    // margin the pockets' own expansion tucks under the lineart — the two
    // cancel at every shared boundary. `gap_close_px` stays symmetric (both
    // sides must agree on which gaps are sealed).
    let outer_opts = FillOpts {
        refer_border: false,
        expand_px: 0,
        ..*opts
    };
    let mut outer: Vec<bool> = vec![false; w * h];
    for corner in [
        (0i32, 0i32),
        (w as i32 - 1, 0),
        (0, h as i32 - 1),
        (w as i32 - 1, h as i32 - 1),
    ] {
        if outer[corner.1 as usize * w + corner.0 as usize] {
            continue;
        }
        if let Some(region) = flood_region(doc, corner, &outer_opts) {
            for (o, r) in outer.iter_mut().zip(region) {
                *o |= r;
            }
        }
    }
    let mut acc: Vec<bool> = vec![false; w * h];
    let mut pockets = 0u32;
    for &(x, y) in seeds {
        if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
            continue;
        }
        if acc[y as usize * w + x as usize] {
            continue; // this pocket is already covered
        }
        if let Some(region) = flood_region(doc, (x, y), opts) {
            // The count is what the status line calls "closed areas": a
            // flood the subtraction below erases wholesale does not count.
            // That is every seed the path drops in the OPEN space — it
            // floods the outer set itself, so N pockets reported N+1.
            if region.iter().zip(&outer).any(|(r, o)| *r && !*o) {
                pockets += 1;
            }
            for (a, r) in acc.iter_mut().zip(region) {
                *a |= r;
            }
        }
    }
    if pockets == 0 {
        return None;
    }
    // Subtract the outer space: what remains is the closed pockets.
    for (a, o) in acc.iter_mut().zip(outer) {
        *a &= !o;
    }
    acc.iter().any(|&a| a).then_some((acc, pockets))
}

/// SE-020 shrink-select (CSP 選択範囲シュリンク): a freehand path through
/// the EMPTY SPACE seeds a UNION of floods — every closed area the path
/// crosses becomes selected, in one action. The fast way to grab a page of
/// flats: drag loosely across the drawing and every pocket between the
/// lineart floods to its own barriers. Returns the selection and the
/// number of closed areas it holds (for the status line).
pub fn magic_select_path(
    doc: &Document,
    seeds: &[(i32, i32)],
    opts: &FillOpts,
) -> Option<(crate::selection::Selection, u32)> {
    let w = doc.size.0 as usize;
    let (mask, floods) = enclosed_pockets(doc, seeds, opts)?;
    Some((
        crate::selection::Selection::from_mask(doc, &mask, w),
        floods,
    ))
}

/// FI-003 Enclose and fill (CSP 囲って塗る) — [`magic_select_path`]'s fill
/// twin, and the flatting workhorse. Lasso roughly around a messy region and
/// every closed area inside it takes the colour at once, as ONE undo step.
/// Returns the pixels written and the number of pockets flooded; `(0, 0)`
/// when the path enclosed nothing.
pub fn enclose_and_fill(
    doc: &mut Document,
    seeds: &[(i32, i32)],
    color: [f32; 3],
    opts: &FillOpts,
) -> (usize, u32) {
    let Some((region, floods)) = enclosed_pockets(doc, seeds, opts) else {
        return (0, 0);
    };
    (
        paint_region(doc, &region, color, "Enclose and fill"),
        floods,
    )
}

/// Flood-fill from `seed` with `color` (straight RGB 0..1, painted opaque).
/// Returns the number of pixels written (0 = seed out of bounds, seed on a
/// barrier of its own colour never happens — the seed area always fills).
pub fn bucket_fill(
    doc: &mut Document,
    seed: (i32, i32),
    color: [f32; 3],
    opts: &FillOpts,
) -> usize {
    bucket_fill_measured(doc, seed, color, opts).0
}

/// [`bucket_fill`] plus what `opts.auto` measured, for the status line.
pub fn bucket_fill_measured(
    doc: &mut Document,
    seed: (i32, i32),
    color: [f32; 3],
    opts: &FillOpts,
) -> (usize, Option<AutoFill>) {
    let Some((region, auto)) = flood_region_measured(doc, seed, opts) else {
        return (0, None);
    };
    (paint_region(doc, &region, color, "Fill"), auto)
}

/// Step 6 for every member of the fill family: write `color` opaquely into
/// the active layer wherever `region` is set, clipped to the selection, as
/// one labelled undo step. Returns the pixels written (0 leaves no undo
/// entry behind). `region` is canvas-sized, row-major.
fn paint_region(doc: &mut Document, region: &[bool], color: [f32; 3], label: &str) -> usize {
    let (w, h) = (doc.size.0 as usize, doc.size.1 as usize);
    debug_assert_eq!(region.len(), w * h);

    // 6. Selection clip.
    let sel = doc.selection.clone();

    // Write inside an undo op.
    let fill_px: [u16; 4] = {
        let a = f32_to_fix15(1.0);
        [
            f32_to_fix15(color[0]),
            f32_to_fix15(color[1]),
            f32_to_fix15(color[2]),
            a,
        ]
    };
    doc.begin_op();
    doc.set_op_label(label);
    let li = doc.active;
    let layer = &mut doc.layers[li];
    let mut written = 0usize;
    let (tw, th) = (w.div_ceil(TILE_SIZE), h.div_ceil(TILE_SIZE));
    for ty in 0..th {
        for tx in 0..tw {
            // Skip tiles with no region pixels before paying for tile_mut.
            let (x0, y0) = (tx * TILE_SIZE, ty * TILE_SIZE);
            let (x1, y1) = ((x0 + TILE_SIZE).min(w), (y0 + TILE_SIZE).min(h));
            let mut any = false;
            'scan: for y in y0..y1 {
                for x in x0..x1 {
                    if region[y * w + x] {
                        any = true;
                        break 'scan;
                    }
                }
            }
            if !any {
                continue;
            }
            let idx = TileIdx::new(tx as i32, ty as i32);
            let t = layer.tile_mut(idx);
            let data = t.data_mut();
            for y in y0..y1 {
                for x in x0..x1 {
                    if !region[y * w + x] {
                        continue;
                    }
                    let cov = match &sel {
                        Some(s) => s.coverage(x as i32, y as i32),
                        None => 255,
                    };
                    if cov == 0 {
                        continue;
                    }
                    let o = ((y - y0) * TILE_SIZE + (x - x0)) * 4;
                    if cov == 255 {
                        data[o..o + 4].copy_from_slice(&fill_px);
                    } else {
                        // Partial coverage: src-over with scaled source.
                        let m = cov as u32;
                        let sa = (fill_px[3] as u32 * m + 127) / 255;
                        for c in 0..4 {
                            let s = (fill_px[c] as u32 * m + 127) / 255;
                            let d = data[o + c] as u32;
                            data[o + c] = (s + (d * (32768 - sa) >> 15)) as u16;
                        }
                    }
                    written += 1;
                }
            }
        }
    }
    if written > 0 {
        // Transparent-pixel lock applies to the bucket too.
        if doc.layers[li].lock_alpha {
            doc.mask_op_to_alpha();
        }
        doc.end_op();
    } else {
        doc.cancel_op();
    }
    written
}

/// The active layer unpremultiplied over white, straight RGB.
fn active_over_white(doc: &Document) -> Vec<[u8; 3]> {
    layer_over_white(doc.active_layer(), doc.size)
}

/// The reference SET composited bottom→top over white (RF-001): each
/// layer's premultiplied ink blends onto the accumulating straight RGB,
/// so stacked references sample as their merged image. `indices` must be
/// in stack order (bottom first) — `Document::reference_layers` returns
/// exactly that.
fn layers_over_white(doc: &Document, indices: &[usize]) -> Vec<[u8; 3]> {
    let (w, h) = (doc.size.0 as usize, doc.size.1 as usize);
    // Accumulate in fix15 straight-RGB (white paper), quantize once at the end.
    let mut acc = vec![[32768u32; 3]; w * h];
    for &li in indices {
        let Some(layer) = doc.layers.get(li) else {
            continue;
        };
        for (idx, tile) in layer.display_tiles() {
            let (ox, oy) = idx.origin();
            for py in 0..TILE_SIZE {
                let y = oy as i64 + py as i64;
                if y < 0 || y >= h as i64 {
                    continue;
                }
                for px in 0..TILE_SIZE {
                    let x = ox as i64 + px as i64;
                    if x < 0 || x >= w as i64 {
                        continue;
                    }
                    let p = tile.pixel(px, py);
                    let a = p[3] as u32;
                    let inv = 32768 - a;
                    let o = &mut acc[y as usize * w + x as usize];
                    for c in 0..3 {
                        o[c] = p[c] as u32 + o[c] * inv / 32768;
                    }
                }
            }
        }
    }
    acc.iter()
        .map(|p| {
            [
                ((p[0] * 255 + 16384) / 32768) as u8,
                ((p[1] * 255 + 16384) / 32768) as u8,
                ((p[2] * 255 + 16384) / 32768) as u8,
            ]
        })
        .collect()
}

/// One layer unpremultiplied over white, straight RGB, canvas-sized.
fn layer_over_white(layer: &crate::doc::Layer, size: (u32, u32)) -> Vec<[u8; 3]> {
    let (w, h) = (size.0 as usize, size.1 as usize);
    let mut out = vec![[255u8, 255, 255]; w * h];
    for (idx, tile) in layer.display_tiles() {
        let (ox, oy) = idx.origin();
        for py in 0..TILE_SIZE {
            let y = oy as i64 + py as i64;
            if y < 0 || y >= h as i64 {
                continue;
            }
            for px in 0..TILE_SIZE {
                let x = ox as i64 + px as i64;
                if x < 0 || x >= w as i64 {
                    continue;
                }
                let p = tile.pixel(px, py);
                // over white: out = c + 1·(1−a), all premultiplied 0..1.
                let a = p[3] as u32;
                let ch = |c: u16| -> u8 {
                    let v = c as u32 + (32768 - a);
                    ((v.min(32768) * 255 + 16384) / 32768) as u8
                };
                out[y as usize * w + x as usize] = [ch(p[0]), ch(p[1]), ch(p[2])];
            }
        }
    }
    out
}

/// One step of 8-connected erosion — dilation of the complement, the same
/// identity `Selection::shrink` is built on. Used by the negative half of
/// FI-016's signed area scaling.
fn erode(mask: &[bool], w: usize, h: usize) -> Vec<bool> {
    let inv: Vec<bool> = mask.iter().map(|&m| !m).collect();
    let grown = dilate(&inv, w, h);
    grown.iter().map(|&g| !g).collect()
}

/// One step of 8-connected dilation.
fn dilate(mask: &[bool], w: usize, h: usize) -> Vec<bool> {
    let mut out = mask.to_vec();
    for y in 0..h {
        for x in 0..w {
            if mask[y * w + x] {
                continue;
            }
            let x0 = x.saturating_sub(1);
            let x1 = (x + 1).min(w - 1);
            let y0 = y.saturating_sub(1);
            let y1 = (y + 1).min(h - 1);
            'n: for ny in y0..=y1 {
                for nx in x0..=x1 {
                    if mask[ny * w + nx] {
                        out[y * w + x] = true;
                        break 'n;
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::FIX15_ONE;

    const INK: [u16; 4] = [0, 0, 0, FIX15_ONE as u16];

    /// Draw a hollow rectangle outline of black ink on the active layer,
    /// leaving a `gap`-pixel hole in the middle of the top edge.
    fn draw_box_with_gap(doc: &mut Document, x0: i32, y0: i32, x1: i32, y1: i32, gap: i32) {
        let gap_from = (x0 + x1) / 2 - gap / 2;
        let gap_to = gap_from + gap;
        for x in x0..=x1 {
            if !(gap_from..gap_to).contains(&x) {
                paint(doc, x, y0);
            }
            paint(doc, x, y1);
        }
        for y in y0..=y1 {
            paint(doc, x0, y);
            paint(doc, x1, y);
        }
    }

    fn paint(doc: &mut Document, x: i32, y: i32) {
        paint_px(doc, x, y, INK);
    }

    fn paint_px(doc: &mut Document, x: i32, y: i32, px: [u16; 4]) {
        let idx = TileIdx::of_pixel(x, y);
        let (ox, oy) = idx.origin();
        doc.active_layer_mut()
            .tile_mut(idx)
            .set_pixel((x - ox) as usize, (y - oy) as usize, px);
    }

    /// A rectangular outline `t` px THICK (drawn inward), with a `gap`-px
    /// hole in the middle of the top band. The auto tests need lines whose
    /// width is a known number, which the 1 px `draw_box_with_gap` cannot say.
    fn draw_thick_box(doc: &mut Document, x0: i32, y0: i32, x1: i32, y1: i32, t: i32, gap: i32) {
        let gap_from = (x0 + x1) / 2 - gap / 2;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let d = (x - x0).min(x1 - x).min(y - y0).min(y1 - y);
                let in_gap = (gap_from..gap_from + gap).contains(&x) && y - y0 < t;
                if d < t && !in_gap {
                    paint(doc, x, y);
                }
            }
        }
    }

    fn px(doc: &Document, x: i32, y: i32) -> [u16; 4] {
        let idx = TileIdx::of_pixel(x, y);
        let (ox, oy) = idx.origin();
        doc.active_layer()
            .tile(idx)
            .map(|t| t.pixel((x - ox) as usize, (y - oy) as usize))
            .unwrap_or([0; 4])
    }

    #[test]
    fn fill_stays_inside_a_closed_box_and_is_undoable() {
        let mut doc = Document::new(256, 256);
        draw_box_with_gap(&mut doc, 40, 40, 200, 200, 0);
        let wrote = bucket_fill(
            &mut doc,
            (120, 120),
            [1.0, 0.0, 0.0],
            &FillOpts {
                gap_close_px: 0,
                expand_px: 0,
                ..Default::default()
            },
        );
        assert!(wrote > 0);
        assert_eq!(px(&doc, 120, 120)[0], FIX15_ONE as u16, "inside filled red");
        assert_eq!(px(&doc, 10, 10)[3], 0, "outside untouched");
        assert!(doc.undo(), "fill is one undo step");
        assert_eq!(px(&doc, 120, 120)[3], 0);
    }

    #[test]
    fn gap_closing_seals_a_leak() {
        // A 3px gap in the outline: a plain fill leaks, gap_close_px=2 seals.
        let mut doc = Document::new(256, 256);
        draw_box_with_gap(&mut doc, 40, 40, 200, 200, 3);

        let leaky = bucket_fill(
            &mut doc,
            (120, 120),
            [0.0, 1.0, 0.0],
            &FillOpts {
                gap_close_px: 0,
                expand_px: 0,
                ..Default::default()
            },
        );
        assert!(
            px(&doc, 10, 10)[3] > 0,
            "without gap closing it leaks outside"
        );
        doc.undo();
        assert!(leaky > 0);

        let sealed = bucket_fill(
            &mut doc,
            (120, 120),
            [0.0, 1.0, 0.0],
            &FillOpts {
                gap_close_px: 2,
                expand_px: 0,
                ..Default::default()
            },
        );
        assert!(sealed > 0);
        assert!(px(&doc, 120, 120)[3] > 0, "inside filled");
        assert_eq!(px(&doc, 10, 10)[3], 0, "gap sealed, no leak");
    }

    #[test]
    fn magic_select_stays_inside_the_box() {
        let mut doc = Document::new(256, 256);
        draw_box_with_gap(&mut doc, 40, 40, 200, 200, 0);
        let sel = magic_select(
            &mut doc,
            (120, 120),
            &FillOpts {
                gap_close_px: 0,
                expand_px: 0,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(sel.coverage(120, 120), 255, "inside selected");
        assert_eq!(sel.coverage(10, 10), 0, "outside not selected");
        assert!(!sel.outline.is_empty(), "outline traced for display");
        assert!(magic_select(&doc, (-5, 0), &FillOpts::default()).is_none());
    }

    /// SE-020: a freehand path through the empty space selects EVERY
    /// closed area it crosses in one action — the flats grabber. Two
    /// separate pockets, a path seeding both, plus skip-if-covered (a
    /// pile of seeds in one pocket costs one flood).
    #[test]
    fn shrink_select_grabs_every_pocket_the_path_crosses() {
        let mut doc = Document::new(256, 256);
        // Two closed boxes with a wall between them; the "drag" runs
        // through both interiors and the outer space.
        draw_box_with_gap(&mut doc, 40, 40, 100, 100, 0);
        draw_box_with_gap(&mut doc, 140, 140, 200, 200, 0);
        let opts = FillOpts {
            gap_close_px: 0,
            expand_px: 0,
            ..Default::default()
        };
        let path: Vec<(i32, i32)> = (0..=20)
            .map(|i| (40 + i * 8, 40 + i * 8)) // diagonal through both boxes
            .collect();
        let (sel, floods) = magic_select_path(&mut doc, &path, &opts).unwrap();
        assert!(
            floods >= 2,
            "the pockets flooded (line-crossing seeds add line fragments: {floods})"
        );
        assert_eq!(sel.coverage(70, 70), 255, "pocket A interior");
        assert_eq!(sel.coverage(170, 170), 255, "pocket B interior");
        assert_eq!(sel.coverage(120, 120), 0, "the wall between them");
        assert_eq!(sel.coverage(10, 10), 0, "the outer space");
        // A path that only crosses ONE pocket (and piles seeds into it).
        let one: Vec<(i32, i32)> = (0..10).map(|i| (60 + i, 60)).collect();
        let (s1, f1) = magic_select_path(&doc, &one, &opts).unwrap();
        assert_eq!(f1, 1, "covered seeds are skipped");
        assert_eq!(s1.coverage(70, 70), 255);
        assert_eq!(s1.coverage(170, 170), 0, "the other pocket untouched");
        // A fully out-of-bounds path selects nothing. (A path ON the
        // lineart selects that line — the wand family's seed-honesty,
        // same as clicking the wand on ink.)
        let oob: Vec<(i32, i32)> = vec![(-5, -5), (-20, -20)];
        assert!(magic_select_path(&doc, &oob, &opts).is_none());
    }

    /// FI-016 must not cancel itself inside the pocket finder: the outer
    /// set only says which space is edge-reachable, so it floods with the
    /// area scaling OFF. With it inherited, the outer flood's own
    /// dilation ate exactly the pixel the pocket tucks under the lineart,
    /// and the path wand disagreed with the click wand on the same
    /// pocket (shrink-select stopping 1 px short of the line).
    #[test]
    fn path_wand_matches_the_click_wand_under_the_lineart() {
        let mut doc = Document::new(128, 128);
        draw_box_with_gap(&mut doc, 40, 40, 100, 100, 0);
        let opts = FillOpts::default(); // expand_px 1 = the tuck-under
        let click = magic_select(&doc, (70, 70), &opts).unwrap();
        let (path, pockets) = magic_select_path(&doc, &[(70, 70)], &opts).unwrap();
        assert_eq!(pockets, 1);
        assert_eq!(click.coverage(40, 70), 255, "the click wand tucks under");
        assert_eq!(path.coverage(40, 70), 255, "and so must the path wand");
        for y in 0..128 {
            for x in 0..128 {
                assert_eq!(
                    click.coverage(x, y),
                    path.coverage(x, y),
                    "the two wands disagree at ({x},{y})"
                );
            }
        }
    }

    /// The status line counts CLOSED areas, not floods: a gesture starts
    /// in the open space, and that seed floods the outer set which is
    /// subtracted whole — one pocket must report one.
    #[test]
    fn enclosed_pockets_counts_pockets_not_floods() {
        let mut doc = Document::new(128, 128);
        draw_box_with_gap(&mut doc, 40, 40, 100, 100, 0);
        let opts = FillOpts {
            gap_close_px: 0,
            expand_px: 0,
            ..Default::default()
        };
        // Two seeds outside the box, two inside it (the second of each
        // pair is skipped as already covered).
        let seeds = [(10, 70), (25, 70), (70, 70), (85, 70)];
        let (sel, pockets) = magic_select_path(&doc, &seeds, &opts).unwrap();
        assert_eq!(pockets, 1, "one closed area, however far the drag ran");
        assert_eq!(sel.coverage(70, 70), 255, "the pocket is selected");
        assert_eq!(sel.coverage(10, 70), 0, "the outer space is subtracted");
    }

    /// The defaults are the contract every earlier build shipped: FI-016's
    /// new sign and FI-022's new switch must not move a single pixel until
    /// the owner reaches for them. (`FillOpts` is tool state, never
    /// persisted — no file on disk carries these, so "old files load
    /// pixel-identically" reduces to exactly this.)
    #[test]
    fn the_new_fill_options_default_to_the_old_behaviour() {
        let d = FillOpts::default();
        assert_eq!(d.expand_px, 1, "the 1 px overfill default is unchanged");
        assert!(!d.refer_border, "the page rim is not a wall unless asked");

        // And prove it end to end: the same box fills the same way as
        // before through the defaults.
        let mut doc = Document::new(128, 128);
        draw_box_with_gap(&mut doc, 20, 20, 100, 100, 0);
        assert!(bucket_fill(&mut doc, (60, 60), [1.0, 0.0, 0.0], &d) > 0);
        assert!(px(&doc, 60, 60)[3] > 0, "inside filled");
        assert_eq!(px(&doc, 5, 5)[3], 0, "outside untouched");
    }

    /// FI-016: area scaling is SIGNED. Negative erodes, so the fill pulls
    /// back off the line instead of tucking under it — CSP's underfill.
    #[test]
    fn area_scaling_underfills_when_negative() {
        let mut doc = Document::new(128, 128);
        draw_box_with_gap(&mut doc, 20, 20, 100, 100, 0);
        let opts = |expand_px| FillOpts {
            gap_close_px: 0,
            expand_px,
            ..Default::default()
        };
        let flat = flood_region(&doc, (60, 60), &opts(0)).expect("region");
        let under = flood_region(&doc, (60, 60), &opts(-3)).expect("region");
        let over = flood_region(&doc, (60, 60), &opts(3)).expect("region");
        let n = |m: &[bool]| m.iter().filter(|&&b| b).count();
        assert!(
            n(&under) < n(&flat) && n(&flat) < n(&over),
            "underfill < plain < overfill ({}, {}, {})",
            n(&under),
            n(&flat),
            n(&over)
        );
        // -3 erodes the 3 px band just inside the outline (rows 21..23 of
        // an interior that starts at 21); the middle survives.
        assert!(flat[23 * 128 + 60], "3 px inside the top edge fills plain");
        assert!(!under[23 * 128 + 60], "…and is eroded away by -3");
        assert!(under[60 * 128 + 60], "the middle still fills");

        // The painted result follows: the erode leaves a clean margin.
        assert!(bucket_fill(&mut doc, (60, 60), [1.0, 0.0, 0.0], &opts(-3)) > 0);
        assert_eq!(px(&doc, 60, 23)[3], 0, "eroded margin left unpainted");
        assert!(px(&doc, 60, 60)[3] > 0, "the area itself painted");
    }

    /// FI-022: with "refer to image border" on, the page's outer perimeter
    /// counts as a drawn line. The case it is FOR is the everyday one —
    /// panel walls that stop a few pixels short of the page edge. Nothing
    /// closes that slot, so the fill escapes and floods the page; with the
    /// perimeter drawn in, gap closing has something to seal against.
    #[test]
    fn refer_to_image_border_seals_lineart_that_stops_short_of_the_page() {
        let mut doc = Document::new(128, 128);
        // Two walls and a floor; the walls stop 3 px shy of the page top.
        for y in 3..=100 {
            paint(&mut doc, 20, y);
            paint(&mut doc, 100, y);
        }
        for x in 20..=100 {
            paint(&mut doc, x, 100);
        }
        let base = FillOpts {
            gap_close_px: 2,
            expand_px: 0,
            ..Default::default()
        };
        let leaky = flood_region(&doc, (60, 60), &base).expect("region");
        assert!(
            leaky[5 * 128 + 5] && leaky[60 * 128 + 5],
            "the 3 px slot at the page top lets the fill out over the walls"
        );

        let walled = flood_region(
            &doc,
            (60, 60),
            &FillOpts {
                refer_border: true,
                ..base
            },
        )
        .expect("region");
        assert!(walled[60 * 128 + 60], "the area itself still fills");
        assert!(
            !walled[5 * 128 + 5] && !walled[60 * 128 + 5],
            "the perimeter line closes the slot — no escape"
        );
        assert!(!walled[0], "and the border line itself is not painted over");
    }

    /// FI-003: the fill twin of SE-020. One lasso across a messy region and
    /// every closed pocket inside it takes the colour — in ONE undo step,
    /// with the outer space left alone.
    #[test]
    fn enclose_and_fill_paints_every_pocket_but_not_the_outer_space() {
        let mut doc = Document::new(256, 256);
        draw_box_with_gap(&mut doc, 40, 40, 100, 100, 0);
        draw_box_with_gap(&mut doc, 140, 140, 200, 200, 0);
        let opts = FillOpts {
            gap_close_px: 0,
            expand_px: 0,
            ..Default::default()
        };
        let steps = doc.undo_labels().len();
        let path: Vec<(i32, i32)> = (0..=20).map(|i| (40 + i * 8, 40 + i * 8)).collect();
        let (wrote, floods) = enclose_and_fill(&mut doc, &path, [1.0, 0.0, 0.0], &opts);
        assert!(wrote > 0 && floods >= 2, "{wrote} px over {floods} pockets");
        assert_eq!(px(&doc, 70, 70)[0], FIX15_ONE as u16, "pocket A painted");
        assert_eq!(px(&doc, 170, 170)[0], FIX15_ONE as u16, "pocket B painted");
        assert_eq!(px(&doc, 10, 10)[3], 0, "the outer space untouched");
        assert_eq!(
            doc.undo_labels().len(),
            steps + 1,
            "both pockets are ONE undo step"
        );
        assert_eq!(doc.undo_labels()[steps], "Enclose and fill", "named for it");
        assert!(doc.undo());
        assert_eq!(px(&doc, 70, 70)[3], 0);
        assert_eq!(px(&doc, 170, 170)[3], 0);

        // A path that enclosed nothing writes nothing and leaves no undo
        // entry to trip over.
        let oob = [(-5, -5), (-20, -20)];
        assert_eq!(
            enclose_and_fill(&mut doc, &oob, [0.0, 1.0, 0.0], &opts),
            (0, 0)
        );
        assert_eq!(
            doc.undo_labels().len(),
            steps,
            "an empty enclose is not an undo step"
        );
    }

    /// FI-003 honours the selection the same way the bucket does — the
    /// pocket set is clipped, not the click.
    #[test]
    fn enclose_and_fill_is_clipped_by_the_selection() {
        let mut doc = Document::new(256, 256);
        draw_box_with_gap(&mut doc, 40, 40, 100, 100, 0);
        draw_box_with_gap(&mut doc, 140, 140, 200, 200, 0);
        doc.selection = Some(crate::selection::Selection::from_rect(
            &doc, 0.0, 0.0, 128.0, 128.0,
        ));
        let path: Vec<(i32, i32)> = (0..=20).map(|i| (40 + i * 8, 40 + i * 8)).collect();
        let (wrote, _) = enclose_and_fill(
            &mut doc,
            &path,
            [1.0, 0.0, 0.0],
            &FillOpts {
                gap_close_px: 0,
                expand_px: 0,
                ..Default::default()
            },
        );
        assert!(wrote > 0);
        assert!(px(&doc, 70, 70)[3] > 0, "pocket inside the selection");
        assert_eq!(px(&doc, 170, 170)[3], 0, "pocket outside it stays clean");
    }

    /// ROADMAP auto mode, the whole premise: the two numbers follow from the
    /// stroke width, so the same click on thin lineart and on thick lineart
    /// must NOT resolve to the same options.
    #[test]
    fn auto_measures_thin_and_thick_lineart_apart() {
        let opts = FillOpts {
            auto: true,
            ..Default::default()
        };
        let mut thin = Document::new(160, 160);
        draw_thick_box(&mut thin, 20, 20, 140, 140, 1, 0);
        let mut thick = Document::new(160, 160);
        draw_thick_box(&mut thick, 20, 20, 140, 140, 6, 0);

        let a = measure_auto(&thin, (80, 80), &opts).expect("thin lineart measured");
        let b = measure_auto(&thick, (80, 80), &opts).expect("thick lineart measured");
        assert_eq!(a.line_px, 1.0, "1 px outline reads as 1 px");
        assert_eq!(b.line_px, 6.0, "6 px outline reads as 6 px");
        assert!(
            a.samples > 100 && b.samples > 100,
            "the whole outline sampled"
        );
        assert_eq!((a.gap_close_px, a.expand_px), (1, 1));
        assert_eq!((b.gap_close_px, b.expand_px), (6, 3));
        assert!(
            b.gap_close_px > a.gap_close_px && b.expand_px > a.expand_px,
            "thicker lines must buy a wider gap close and a deeper tuck"
        );
        // And the options a fill actually runs with are those values.
        let (resolved, note) = resolve_auto(&thick, (80, 80), &opts);
        assert!(!resolved.auto, "resolution is one-shot");
        assert_eq!((resolved.gap_close_px, resolved.expand_px), (6, 3));
        assert_eq!(note, Some(b));

        // Blank paper bounds nothing: measured as unmeasurable, never guessed.
        let blank = Document::new(64, 64);
        assert!(measure_auto(&blank, (10, 10), &opts).is_none());
        let (kept, none) = resolve_auto(&blank, (10, 10), &opts);
        assert!(none.is_none());
        assert_eq!(
            (kept.gap_close_px, kept.expand_px),
            (opts.gap_close_px, opts.expand_px),
            "unmeasurable keeps the manual numbers"
        );
    }

    /// The halo case: a stroke with a soft edge leaves a pale rim between
    /// the flood and the black core, and the manual 1 px default only eats
    /// part of it. Auto sizes the tuck to the line it measured.
    #[test]
    fn auto_fringe_covers_the_antialiased_halo() {
        // 4 px black core with a 2 px half-alpha skirt on the inside — the
        // shape a real inked line has at print resolution.
        let mut doc = Document::new(160, 160);
        draw_thick_box(&mut doc, 20, 20, 140, 140, 4, 0);
        let grey = [0, 0, 0, (FIX15_ONE / 2) as u16];
        for y in 20..=140 {
            for x in 20..=140 {
                let d = (x - 20).min(140 - x).min(y - 20).min(140 - y);
                if (4..6).contains(&d) {
                    paint_px(&mut doc, x, y, grey);
                }
            }
        }
        let halo = (80, 24); // the outer of the two skirt rows

        let manual = FillOpts::default();
        assert_eq!(manual.expand_px, 1, "the shipped 1 px tuck");
        assert!(bucket_fill(&mut doc, (80, 80), [1.0, 0.0, 0.0], &manual) > 0);
        assert!(px(&doc, 80, 80)[0] > 0, "the area filled");
        assert_eq!(
            px(&doc, halo.0, halo.1)[0],
            0,
            "1 px of tuck leaves the outer skirt row showing — the halo"
        );
        doc.undo();

        let (wrote, auto) = bucket_fill_measured(
            &mut doc,
            (80, 80),
            [1.0, 0.0, 0.0],
            &FillOpts {
                auto: true,
                ..manual
            },
        );
        let a = auto.expect("the skirted line is measurable");
        assert_eq!(a.line_px, 6.0, "core plus both skirt rows is the barrier");
        assert_eq!(a.expand_px, 3, "half the line width, floored");
        assert!(wrote > 0);
        assert_eq!(
            px(&doc, halo.0, halo.1)[0],
            FIX15_ONE as u16,
            "auto tucks the fill under the whole skirt"
        );
        assert_eq!(px(&doc, 5, 5)[3], 0, "and does not walk through the line");
    }

    /// The gap half, end to end: an 8 px break needs 4 px of closing, which
    /// the shipped default (2) does not have and a 5 px line measures its
    /// way to.
    #[test]
    fn auto_seals_a_gap_the_default_leaks_through() {
        let mut doc = Document::new(200, 200);
        draw_thick_box(&mut doc, 20, 20, 180, 180, 5, 8);
        let manual = FillOpts::default();
        assert_eq!(manual.gap_close_px, 2, "the shipped default");
        let leaky = flood_region(&doc, (100, 100), &manual).expect("region");
        assert!(
            leaky[5 * 200 + 5],
            "2 px of closing leaks through an 8 px gap"
        );

        let sealed = flood_region(
            &doc,
            (100, 100),
            &FillOpts {
                auto: true,
                ..manual
            },
        )
        .expect("region");
        assert!(sealed[100 * 200 + 100], "the area itself still fills");
        assert!(!sealed[5 * 200 + 5], "the measured gap close seals it");
    }

    /// Auto is a switch, not a rewrite: with it off every number is honoured
    /// verbatim, so the behaviour `gap_closing_seals_a_leak` pinned before
    /// auto existed must still hold pixel for pixel.
    #[test]
    fn manual_fill_is_untouched_by_the_auto_switch() {
        assert!(!FillOpts::default().auto, "opt-in");
        let mut doc = Document::new(256, 256);
        draw_box_with_gap(&mut doc, 40, 40, 200, 200, 3);
        let manual = FillOpts {
            gap_close_px: 2,
            expand_px: 0,
            ..Default::default()
        };
        let sealed = flood_region(&doc, (120, 120), &manual).expect("region");
        assert!(sealed[120 * 256 + 120], "inside filled");
        assert!(!sealed[10 * 256 + 10], "gap sealed, no leak");

        // Resolution with auto off is the identity, and measures nothing.
        let (same, note) = resolve_auto(&doc, (120, 120), &manual);
        assert!(note.is_none(), "auto off reports no measurement");
        assert_eq!(same.gap_close_px, 2);
        assert_eq!(same.expand_px, 0);
        assert_eq!(
            flood_region(&doc, (120, 120), &same).expect("region"),
            sealed,
            "the resolved options fill identically"
        );

        // A click ON the lineart still fills the lineart (the fill family's
        // seed honesty) — auto must not turn that into a refusal.
        let on_the_line = FillOpts {
            auto: true,
            ..manual
        };
        assert!(bucket_fill(&mut doc, (40, 120), [1.0, 0.0, 0.0], &on_the_line) > 0);
    }

    #[test]
    fn selection_clips_the_fill() {
        let mut doc = Document::new(128, 128);
        doc.selection = Some(crate::selection::Selection::from_rect(
            &doc, 0.0, 0.0, 64.0, 128.0,
        ));
        bucket_fill(&mut doc, (32, 64), [0.0, 0.0, 1.0], &FillOpts::default());
        assert!(px(&doc, 32, 64)[3] > 0, "inside selection filled");
        assert_eq!(px(&doc, 100, 64)[3], 0, "outside selection untouched");
    }

    #[test]
    fn fill_refers_to_a_hidden_reference_layer() {
        // Roughs on a hidden reference layer still guide the fill.
        let mut doc = Document::new(128, 128);
        draw_box_with_gap(&mut doc, 40, 40, 88, 88, 0);
        doc.layers[0].name = "Rough".to_string();
        assert!(doc.set_layer_visible(0, false));
        assert!(doc.set_layer_reference(0, true));
        doc.add_layer("Ink"); // becomes the active fill target

        let wrote = bucket_fill(
            &mut doc,
            (64, 64),
            [1.0, 0.0, 0.0],
            &FillOpts {
                refer: FillRefer::Reference,
                gap_close_px: 0,
                expand_px: 0,
                ..Default::default()
            },
        );
        assert!(wrote > 0);
        assert!(px(&doc, 64, 64)[3] > 0, "inside the hidden rough's box");
        assert_eq!(px(&doc, 10, 10)[3], 0, "outside stays clean");
    }

    #[test]
    fn fill_skips_draft_layers_when_opted_out() {
        let mut doc = Document::new(128, 128);
        draw_box_with_gap(&mut doc, 40, 40, 88, 88, 0);
        doc.layers[0].name = "Draft".to_string();
        assert!(doc.set_layer_draft(0, true));
        doc.add_layer("Ink"); // active fill target

        // Drafts ignored: the box is invisible to the sampler, fill spreads.
        bucket_fill(
            &mut doc,
            (64, 64),
            [0.0, 1.0, 0.0],
            &FillOpts {
                refer_drafts: false,
                gap_close_px: 0,
                expand_px: 0,
                ..Default::default()
            },
        );
        assert!(
            px(&doc, 10, 10)[3] > 0,
            "draft box does not contain the fill"
        );

        doc.undo();
        // Drafts sampled: the box contains it again.
        bucket_fill(
            &mut doc,
            (64, 64),
            [0.0, 1.0, 0.0],
            &FillOpts {
                refer_drafts: true,
                gap_close_px: 0,
                expand_px: 0,
                ..Default::default()
            },
        );
        assert_eq!(px(&doc, 10, 10)[3], 0, "draft box contains the fill");
    }

    #[test]
    fn reference_flags_form_a_set_with_solo_and_clear() {
        // RF-001 (owner spec 2026-08-17): marking is INDEPENDENT — the
        // owner rejected CSP's exclusivity ("five marked, marking a sixth
        // clears the other five" is the complaint). Alt+click = solo;
        // clear-all drops the set.
        let mut doc = Document::new(64, 64);
        doc.add_layer("B");
        doc.add_layer("C");
        assert!(doc.set_layer_reference(0, true));
        assert!(doc.set_layer_reference(1, true));
        assert!(
            doc.layers[0].reference && doc.layers[1].reference,
            "marking the second must not clear the first"
        );
        assert_eq!(doc.reference_layers(), vec![0, 1]);
        assert_eq!(doc.reference_layer_index(), Some(1), "topmost for compat");
        // Solo clears the others.
        assert!(doc.set_layer_reference_solo(2));
        assert_eq!(doc.reference_layers(), vec![2]);
        // Clear-all empties the set.
        doc.clear_references();
        assert!(doc.reference_layers().is_empty());
        assert!(!doc.set_layer_reference(9, true), "bad index refused");
    }

    #[test]
    fn fill_refer_samples_the_reference_set_composited() {
        // Two reference layers stack: their MERGED image is what the fill
        // samples — a barrier only the composite shows must hold the fill.
        let mut doc = Document::new(64, 64);
        let a = doc.add_layer("under");
        doc.layers[a]
            .tile_mut(TileIdx::new(0, 0))
            .set_pixel(5, 5, [0, 0, 0, 32768]);
        let b = doc.add_layer("over");
        doc.layers[b]
            .tile_mut(TileIdx::new(0, 0))
            .set_pixel(5, 6, [0, 0, 0, 32768]);
        doc.set_layer_reference(a, true);
        doc.set_layer_reference(b, true);
        let opts = FillOpts {
            refer: FillRefer::Reference,
            // Zero the under-lineart expansion + gap closing so the
            // BARRIER itself is what the assert measures (step 5 grows
            // the region 1px under lines BY DESIGN — adjacent seeds
            // legitimately cross a 1px barrier).
            expand_px: 0,
            gap_close_px: 0,
            ..FillOpts::default()
        };
        // Seed at (5,5): the under-layer's pixel is a barrier in the SET's
        // composite — the fill must not leak into it.
        let filled = flood_region(&doc, (5, 4), &opts).expect("region");
        assert!(!filled[5 * 64 + 5], "the composite barrier must hold");
        // And the sample source ignores eye state (references are sampled
        // hidden too): hide both, same result.
        doc.set_layer_visible(a, false);
        doc.set_layer_visible(b, false);
        let filled2 = flood_region(&doc, (5, 4), &opts).expect("region 2");
        assert_eq!(filled, filled2, "reference sampling ignores eye state");
    }
}
