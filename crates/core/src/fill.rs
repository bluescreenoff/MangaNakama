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
    /// P0-4 (CSP 拡縮方法): the SHAPE `expand_px` scales by. Defaults to
    /// [`ExpandMode::Rect`], which is the square dilation every earlier
    /// build ran — so existing fills stay byte-identical.
    pub expand_mode: ExpandMode,
    /// Measure `gap_close_px` and `expand_px` from the lineart around the
    /// click instead of taking them from the fields ([`measure_auto`]).
    /// Defaults OFF: with it off every field above is honoured verbatim,
    /// so a build that never touches the switch fills pixel-identically.
    pub auto: bool,
    /// Row 40/120 (CSP 半透明を透明にする, "treat semi-transparent as
    /// transparent"): a source pixel whose OPACITY is below the midpoint
    /// counts as FILLABLE — the antialiased skirt of a line is
    /// semi-transparent ink, so the fill runs under the fringe to the dark
    /// core and the flat shows no light halo against the lineart. Tests
    /// opacity, not brightness (owner verdict 2026-08-27, matching CSP):
    /// identical to the old luma rule on black lineart over white paper,
    /// and correct on colour work, where the luma rule let a pale-but-
    /// OPAQUE tone read as paper and the fill leak across it. Defaults
    /// OFF — the wall every earlier build built, bit for bit.
    pub semi_transparent_paper: bool,
    /// C-005 (CSP 対象色, the Target-colour dropdown, FI-030..039): which
    /// pixel classes are FILLABLE — everything else walls. `AllColours`
    /// (the default) is every build before this field: walls come from
    /// the seed-colour tolerance alone, byte for byte. Class thresholds
    /// are ours (CSP publishes none): transparent = alpha below half;
    /// black = inked and composite luma under 64; white = inked and luma
    /// at or past 192. The "area surrounded by X" trio of the CSP list is
    /// the same wall classes through the close-area (Enclose) machinery,
    /// and the two whole-motif "all enclosed areas" repaint modes are
    /// deliberately not built (note on the row).
    pub close: FillClose,
}

/// The Target-colour classes (see [`FillOpts::close`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FillClose {
    /// FI-030 対象色: すべての色 — tolerance walls, as ever.
    #[default]
    AllColours,
    /// 透明のみ — only transparent pixels fill; drawn ink walls.
    OnlyTransparent,
    /// 透明以外 — drawn pixels fill; transparency walls.
    NotTransparent,
    /// 黒のみ — black fills.
    OnlyBlack,
    /// 黒以外 — everything but black fills.
    NotBlack,
    /// 白と透明 — white and transparent fill.
    WhiteAndTransparent,
    /// 白と透明以外 — everything but white/transparent fills.
    NotWhiteAndTransparent,
}

/// Rec.709 luma of a composite-over-white source pixel, rounded — the
/// "mostly paper" test for the BRUSH anti-overflow barrier (which keeps
/// the luma rule deliberately: it asks "how dark is the reference ink",
/// not "how opaque"). The fill's semi-transparent switch left this rule
/// for the opacity test above (owner verdict 2026-08-27).
fn luma_u8(p: [u8; 3]) -> u8 {
    ((p[0] as u16 * 54 + p[1] as u16 * 183 + p[2] as u16 * 19) >> 8) as u8
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

/// P0-4, CSP's 拡縮方法 ("scaling method") — the SHAPE area scaling grows in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ExpandMode {
    /// 矩形: a square (Chebyshev) ball. Corners grow as far as edges, which
    /// is what every build before P0-4 did, so it stays the default.
    #[default]
    Rect,
    /// 円形: a Euclidean disc. Rounds off the corners a square dilation
    /// leaves on a diagonal boundary, so it never covers more than [`Rect`].
    Round,
    /// 最も濃いピクセルまで: walk outward and STOP at the darkest pixel of
    /// the reference image. The JP-standard mode for anti-aliased lineart —
    /// the fill tucks exactly to the core of the line instead of a fixed
    /// distance that overshoots straight past a thin one. Only the POSITIVE
    /// half has a "darkest pixel" to aim at; a negative `expand_px` erodes
    /// with the [`Rect`] shape, as it always did.
    ToDarkest,
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
            expand_mode: ExpandMode::Rect,
            auto: false,
            semi_transparent_paper: false,
            close: FillClose::default(),
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
    // The opacity walk runs only when a rule needs it — the
    // semi-transparent switch or the Target-colour classes — one extra
    // composite pass, opted into, never a tax on the default fill.
    let alpha = if opts.semi_transparent_paper || opts.close != FillClose::AllColours {
        source_alpha(doc, opts)
    } else {
        Vec::new()
    };
    let start = sy as usize * w + sx as usize;
    let (opts, auto) = resolve_with_src(&src, w, h, start, opts);
    Some((region_from_src(&src, &alpha, w, h, start, &opts), auto))
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
fn source_pixels(doc: &Document, opts: &FillOpts) -> Vec<[u8; 3]> {    match opts.refer {
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

/// The opacity canvas for [`FillOpts::semi_transparent_paper`] — the same
/// source walk [`source_pixels`] samples, reduced to one coverage byte per
/// pixel. Over-transparent composites so the art's own alpha survives:
/// a pale-but-opaque tone reads 255 (a wall), the antialiased skirt of any
/// line reads low (fillable), whatever the colour (owner verdict
/// 2026-08-27: CSP's 半透明を透明にする tests opacity, not brightness).
fn source_alpha(doc: &Document, opts: &FillOpts) -> Vec<u8> {
    match opts.refer {
        FillRefer::Active => layer_alpha(doc.active_layer(), doc.size),
        FillRefer::Reference => {
            let refs = doc.reference_layers();
            if refs.is_empty() {
                export::composite_alpha_for_fill(doc, opts.refer_drafts)
            } else {
                layers_alpha(doc, &refs)
            }
        }
        FillRefer::All => export::composite_alpha_for_fill(doc, opts.refer_drafts),
    }
}

/// Row 42 / A-014 (CSP はみ出さない): the BRUSH anti-overflow barrier —
/// `(width, allow)` with one byte per canvas pixel: 255 = paint freely,
/// 0 = the REFERENCE SET's ink. The reference set is the ONLY referent
/// (owner ruling 2026-08-25, overturning the earlier widened one): a
/// frame folder already clips its own children through its panel mask,
/// and a page-level layer below the folder is covered by the border ink
/// at composite — so walling every stroke on every layer behind every
/// panel bought correctness nowhere and cost a border rasterize per
/// stroke. Deliberately still the LUMA rule (composite darkness of the
/// reference ink — "how dark", not "how opaque"); only the FILL's
/// semi-transparent switch moved to the opacity test (owner verdict
/// 2026-08-27). `None` when there is
/// nothing to refer to — the toggle is then honestly a no-op, not an
/// all-paper mask.
pub fn anti_overflow_barrier(
    doc: &Document,
    colour_margin: u8,
    vector_centreline: bool,
) -> Option<(usize, Vec<u8>)> {
    let (w, h) = (doc.size.0 as usize, doc.size.1 as usize);
    let refs = doc.reference_layers();
    if refs.is_empty() {
        return None;
    }
    let mut allow = vec![255u8; w * h];
    // A-015 (ベクトルまで塗り): with centreline mode on, a VECTOR
    // reference layer contributes its strokes' sample points as 1 px
    // walls — the spline itself — and its rendered anti-aliased edge is
    // excluded from the luma wall below, so paint may tuck right up to
    // the middle of the line instead of stopping at its fringe.
    let mut raster_refs = refs.clone();
    if vector_centreline {
        let (vector_refs, kept): (Vec<usize>, Vec<usize>) = refs
            .into_iter()
            .partition(|&li| doc.layers.get(li).is_some_and(|l| l.strokes.is_some()));
        for li in vector_refs {
            let Some(set) = doc.layers.get(li).and_then(|l| l.strokes.as_ref()) else {
                continue;
            };
            for s in &set.strokes {
                // Samples sit a couple of px apart; connect them with a
                // short DDA so the wall has no gaps.
                for pair in s.points.windows(2) {
                    let (x0, y0) = (pair[0].0, pair[0].1);
                    let (x1, y1) = (pair[1].0, pair[1].1);
                    let n = ((x1 - x0).abs().max((y1 - y0).abs()).ceil() as usize).max(1);
                    for i in 0..=n {
                        let t = i as f32 / n as f32;
                        let x = (x0 + (x1 - x0) * t).round() as i64;
                        let y = (y0 + (y1 - y0) * t).round() as i64;
                        if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
                            allow[y as usize * w + x as usize] = 0;
                        }
                    }
                }
            }
        }
        raster_refs = kept;
    }
    // The luma wall from the RASTER references (all of them when
    // centreline mode is off — the behaviour every earlier build had).
    if !raster_refs.is_empty() {
        let src = layers_over_white(doc, &raster_refs);
        // A-016 (色余白): the reference ink's own colour, for the
        // margin — the darkest inked pixel is the line core.
        let mut ink: Option<[u8; 3]> = None;
        let mut ink_luma = 255u8;
        for px in &src {
            let l = luma_u8(*px);
            if l < 128 && l < ink_luma {
                ink_luma = l;
                ink = Some(*px);
            }
        }
        let m = colour_margin as i16;
        for (a, px) in allow.iter_mut().zip(&src) {
            let luma_wall = luma_u8(*px) < 128;
            let margin_wall = m > 0
                && ink.is_some_and(|k| {
                    (px[0] as i16 - k[0] as i16).abs().max((px[1] as i16 - k[1] as i16).abs()).max(
                        (px[2] as i16 - k[2] as i16).abs(),
                    ) <= m
                });
            if luma_wall || margin_wall {
                *a = 0;
            }
        }
    }
    Some((w, allow))
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
/// resolution happens once, above. `alpha` is [`source_alpha`]'s canvas —
/// empty when the semi-transparent switch is off, which is the only rule
/// that reads it.
fn region_from_src(
    src: &[[u8; 3]],
    alpha: &[u8],
    w: usize,
    h: usize,
    start: usize,
    opts: &FillOpts,
) -> Vec<bool> {
    let mut barrier = barrier_from(src, src[start], opts.tolerance);
    let mut barrier_orig = barrier.clone();

    // 2a. Row 40/120 (CSP "treat semi-transparent as transparent"): a
    // source pixel that is mostly TRANSPARENT — composite opacity below
    // the midpoint, which is what the antialiased skirt of any line is —
    // is FILLABLE. The flood then runs under the fringe to the line's
    // dark core, and no light halo survives against the flat. Opacity,
    // not brightness (owner verdict 2026-08-27, matching CSP): on colour
    // work a pale-but-opaque tone keeps its wall, where the old luma
    // rule read it as paper and let the fill leak. Cleared from BOTH
    // barriers: the flood walls at the core, and the gap-recovery step
    // agrees. The page rim (2b) walls after this and stays a wall.
    if opts.semi_transparent_paper {
        for (b, a) in barrier.iter_mut().zip(alpha) {
            if *a < 128 {
                *b = false;
            }
        }
        for (b, a) in barrier_orig.iter_mut().zip(alpha) {
            if *a < 128 {
                *b = false;
            }
        }
    }

    // 2a'. C-005 (対象色, Target colour): the chosen class set REPLACES
    // the tolerance barrier — a pixel is a wall exactly when its class
    // is not the target. After the semi-transparent pass so the two
    // compose deterministically, into BOTH barriers so the gap-recovery
    // step agrees, and before the rim wall so FI-022 still closes the
    // page edge.
    if opts.close != FillClose::AllColours {
        for i in 0..barrier.len() {
            let transparent = alpha.get(i).is_none_or(|a| *a < 128);
            let luma = luma_u8(src[i]);
            let black = !transparent && luma < 64;
            let white = !transparent && luma >= 192;
            let fillable = match opts.close {
                FillClose::AllColours => true,
                FillClose::OnlyTransparent => transparent,
                FillClose::NotTransparent => !transparent,
                FillClose::OnlyBlack => black,
                FillClose::NotBlack => !black,
                FillClose::WhiteAndTransparent => white || transparent,
                FillClose::NotWhiteAndTransparent => !(white || transparent),
            };
            barrier[i] = !fillable;
            barrier_orig[i] = !fillable;
        }
    }

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

    // 3. Fatten the barrier to seal gaps. ONE window pass, whatever the
    // radius: `gap_close_px` rounds of 3×3 dilation IS the Chebyshev ball of
    // that radius, and [`dilate_by`] computes the ball directly (the
    // `Selection::grow` prefix-sum shape). The barrier is not confined to
    // any region yet, so this is the one morphology call that must see the
    // whole canvas.
    dilate_by(
        &mut barrier,
        w,
        &Rect {
            x0: 0,
            y0: 0,
            x1: w,
            y1: h,
        },
        opts.gap_close_px as usize,
        false,
    );

    // Flood (4-connected BFS) over non-barrier. A seed that landed on the
    // FATTENED barrier means gap closing ate it: fall back to the seed's own
    // contiguous same-colour blob, flooded against the real lines only.
    let mut region = if barrier[start] {
        flood(&barrier_orig, w, h, start)
    } else {
        flood(&barrier, w, h, start)
    };

    // 4. Recover the margin the fat barrier stole — but never cross real
    // lines. This one CANNOT collapse into a window pass: every round is
    // re-clipped against `barrier_orig`, so the reachable set depends on the
    // previous round. It is bbox-clipped instead — the region grows by at
    // most 1 px a round, so `bbox + gap_close_px` is all it can ever touch,
    // and a small fill on a B4 page stops paying for the whole page.
    let gap = opts.gap_close_px as usize;
    if gap > 0 {
        let rect = mask_rect(&region, w, h, gap);
        let (rw, rh) = (rect.x1 - rect.x0, rect.y1 - rect.y0);
        let mut grown = vec![false; rw * rh];
        for _ in 0..gap {
            for j in 0..rh {
                for i in 0..rw {
                    let (x, y) = (rect.x0 + i, rect.y0 + j);
                    // 8-connected, clamped at the canvas border exactly as
                    // the old per-pixel dilation was.
                    let x1 = (x + 1).min(w - 1);
                    let y1 = (y + 1).min(h - 1);
                    grown[j * rw + i] = region[y * w + x]
                        || (x.saturating_sub(1)..=x1)
                            .any(|nx| (y.saturating_sub(1)..=y1).any(|ny| region[ny * w + nx]));
                }
            }
            for j in 0..rh {
                for i in 0..rw {
                    let o = (rect.y0 + j) * w + rect.x0 + i;
                    if grown[j * rw + i] && !barrier_orig[o] {
                        region[o] = true;
                    }
                }
            }
        }
    }
    // 5. Signed area scaling (FI-016), in the shape P0-4's `expand_mode`
    // asks for. Positive = overfill, tucking the region under the
    // anti-aliased lineart; negative = underfill, eroding it so a
    // hard-edged fill does not touch the line at all. Erosion is dilation
    // of the complement, the same identity `Selection::shrink` uses — and
    // it inherits that identity's edge rule: the window clamps at the
    // canvas border, so a region running off the page does not pull back
    // from the page edge, only from real boundaries.
    let r = opts.expand_px.unsigned_abs() as usize;
    if r > 0 {
        if opts.expand_px > 0 && opts.expand_mode == ExpandMode::ToDarkest {
            expand_to_darkest(&mut region, src, w, h, r);
        } else {
            // ToDarkest has no darkest pixel to aim at when it is SHRINKING,
            // so a negative scaling erodes with the square ball, as always.
            let rect = mask_rect(&region, w, h, r);
            let round = opts.expand_mode == ExpandMode::Round;
            if opts.expand_px < 0 {
                erode_by(&mut region, w, &rect, r, round);
            } else {
                dilate_by(&mut region, w, &rect, r, round);
            }
        }
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

/// One layer's own coverage, canvas-sized — [`layer_over_white`]'s alpha
/// twin (raw tiles, no folders, no blend, no opacity: the same source the
/// Active-refer RGB walk shows).
fn layer_alpha(layer: &crate::doc::Layer, size: (u32, u32)) -> Vec<u8> {
    let (w, h) = (size.0 as usize, size.1 as usize);
    let mut out = vec![0u8; w * h];
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
                out[y as usize * w + x as usize] = (p[3] as u32 * 255 / 32768) as u8;
            }
        }
    }
    out
}

/// The reference SET's merged coverage — [`layers_over_white`]'s alpha
/// twin: standard over-accumulation in fix15, quantized once at the end.
fn layers_alpha(doc: &Document, indices: &[usize]) -> Vec<u8> {
    let (w, h) = (doc.size.0 as usize, doc.size.1 as usize);
    let mut acc = vec![0u32; w * h];
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
                    let a = tile.pixel(px, py)[3] as u32;
                    let o = &mut acc[y as usize * w + x as usize];
                    *o = a + *o * (32768 - a) / 32768;
                }
            }
        }
    }
    acc.iter().map(|a| ((a * 255 + 16384) / 32768) as u8).collect()
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

// --- morphology ----------------------------------------------------------
//
// GPU audit queue #2, CPU half. The old shape was a naive 3×3 pass RUN ONCE
// PER MORPHOLOGY PIXEL, over the whole canvas, allocating a fresh canvas-sized
// Vec each round: `expand_px = 8` on a B4 page at 600 dpi cost eight full-page
// passes and eight full-page allocations to grow a fill by 8 px. Two changes
// fix it and neither moves a pixel:
//
// * N rounds of 3×3 dilation IS the Chebyshev ball of radius N, and a
//   Chebyshev ball is separable — a horizontal window OR then a vertical one,
//   each answered from a prefix sum, so ANY radius costs one pass. That is
//   `Selection::grow`'s trick, ported here.
// * Nothing but the flood's own bounding box (plus the radius) can change, so
//   the passes run over that sub-rect instead of the page.

/// A half-open pixel rect, `x1`/`y1` exclusive.
struct Rect {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

/// `mask`'s bounding box grown by `margin` and clipped to the canvas — the
/// only ground a `margin`-radius morphology on `mask` can reach. Empty (and
/// so a no-op for every pass below) when the mask is.
fn mask_rect(mask: &[bool], w: usize, h: usize, margin: usize) -> Rect {
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            if mask[y * w + x] {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x1 == 0 {
        return Rect {
            x0: 0,
            y0: 0,
            x1: 0,
            y1: 0,
        };
    }
    Rect {
        x0: x0.saturating_sub(margin),
        y0: y0.saturating_sub(margin),
        x1: (x1 + margin).min(w),
        y1: (y1 + margin).min(h),
    }
}

/// Dilate `mask` in place by radius `r` — a Euclidean disc when `round`, the
/// square (Chebyshev) ball otherwise, the latter being exactly `r` rounds of
/// the 3×3 dilation this replaced. Only `rect` is read or written, so callers
/// MUST pass a rect that already contains the mask's bounding box grown by
/// `r` (that is what [`mask_rect`] is for); pixels outside it cannot be
/// reached by a radius-`r` ball anyway. Windows clip to `rect`, which where
/// `rect` meets the page reproduces the old pass's canvas-border clamping.
fn dilate_by(mask: &mut [bool], w: usize, rect: &Rect, r: usize, round: bool) {
    let (rw, rh) = (rect.x1 - rect.x0, rect.y1 - rect.y0);
    if r == 0 || rw == 0 || rh == 0 {
        return;
    }
    // Row prefix sums: `run[j][a..b]` is set iff prefix[b] > prefix[a].
    let stride = rw + 1;
    let mut prefix = vec![0u32; stride * rh];
    for j in 0..rh {
        let base = (rect.y0 + j) * w + rect.x0;
        for i in 0..rw {
            prefix[j * stride + i + 1] = prefix[j * stride + i] + mask[base + i] as u32;
        }
    }
    let row_hit = |j: usize, i: usize, hw: usize| {
        let lo = i.saturating_sub(hw);
        let hi = (i + hw + 1).min(rw);
        prefix[j * stride + hi] > prefix[j * stride + lo]
    };
    if round {
        // A disc is a stack of rows whose half-width falls off as
        // √(r²−dy²). Not separable, so it costs r row-window lookups per
        // pixel — but each is O(1) off the same prefix sums, and it is
        // still ONE pass over the rect instead of r of them.
        let half: Vec<usize> = (0..=r)
            .map(|dy| ((r * r - dy * dy) as f64).sqrt() as usize)
            .collect();
        for j in 0..rh {
            for i in 0..rw {
                let on = (0..=r).any(|dy| {
                    (j + dy < rh && row_hit(j + dy, i, half[dy]))
                        || (dy <= j && row_hit(j - dy, i, half[dy]))
                });
                mask[(rect.y0 + j) * w + rect.x0 + i] = on;
            }
        }
    } else {
        // Separable: horizontal window OR, then vertical.
        let mut tmp = vec![false; rw * rh];
        for j in 0..rh {
            for i in 0..rw {
                tmp[j * rw + i] = row_hit(j, i, r);
            }
        }
        let mut col = vec![0u32; rh + 1];
        for i in 0..rw {
            for j in 0..rh {
                col[j + 1] = col[j] + tmp[j * rw + i] as u32;
            }
            for j in 0..rh {
                let lo = j.saturating_sub(r);
                let hi = (j + r + 1).min(rh);
                mask[(rect.y0 + j) * w + rect.x0 + i] = col[hi] > col[lo];
            }
        }
    }
}

/// Erode by radius `r` — dilation of the complement, the identity
/// `Selection::shrink` is built on. Same `rect` contract as [`dilate_by`]:
/// outside it the mask is empty, so its complement is solid and erodes to
/// nothing, which is what leaving those pixels alone already says.
fn erode_by(mask: &mut [bool], w: usize, rect: &Rect, r: usize, round: bool) {
    for y in rect.y0..rect.y1 {
        for x in rect.x0..rect.x1 {
            mask[y * w + x] = !mask[y * w + x];
        }
    }
    dilate_by(mask, w, rect, r, round);
    for y in rect.y0..rect.y1 {
        for x in rect.x0..rect.x1 {
            mask[y * w + x] = !mask[y * w + x];
        }
    }
}

/// Rec.601 luma of a straight-RGB source pixel, ×1000 so it stays integer.
fn luma(p: [u8; 3]) -> u32 {
    p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114
}

/// [`ExpandMode::ToDarkest`] (CSP 最も濃いピクセルまで拡張): from every pixel
/// on the region's boundary, walk outward up to `r` px in each of the eight
/// directions and stop at the first LOCAL LUMINANCE MINIMUM of the reference
/// image — the [`barrier_run`] ray walk, reading darkness instead of barrier
/// membership. On an anti-aliased line that lands the fill's edge exactly on
/// the line's darkest pixel: it steps through the pale skirt (each pixel
/// darker than the last), reaches the core, and refuses the first pixel that
/// is lighter again, so a thin line is never stepped over the way a fixed
/// radius steps over it. Where the boundary faces open paper nothing gets
/// darker, so nothing expands — the mode has no target there, and inventing
/// one is what the fixed radius already does.
fn expand_to_darkest(region: &mut [bool], src: &[[u8; 3]], w: usize, h: usize, r: usize) {
    const DIRS: [(i32, i32); 8] = [
        (1, 0),
        (-1, 0),
        (0, 1),
        (0, -1),
        (1, 1),
        (1, -1),
        (-1, 1),
        (-1, -1),
    ];
    // Collected first, applied after: the walk must see the boundary the
    // flood left, not one it grew itself half a scan ago.
    let mut add: Vec<usize> = Vec::new();
    let rect = mask_rect(region, w, h, 0);
    for y in rect.y0..rect.y1 {
        for x in rect.x0..rect.x1 {
            let i = y * w + x;
            if !region[i] {
                continue;
            }
            let edge = (x > 0 && !region[i - 1])
                || (x + 1 < w && !region[i + 1])
                || (y > 0 && !region[i - w])
                || (y + 1 < h && !region[i + w]);
            if !edge {
                continue;
            }
            for (dx, dy) in DIRS {
                let mut prev = luma(src[i]);
                let (mut cx, mut cy) = (x as i32, y as i32);
                for _ in 0..r {
                    cx += dx;
                    cy += dy;
                    if cx < 0 || cy < 0 || cx as usize >= w || cy as usize >= h {
                        break;
                    }
                    let j = cy as usize * w + cx as usize;
                    let l = luma(src[j]);
                    if l >= prev {
                        break; // the darkest pixel on this ray is behind us
                    }
                    add.push(j);
                    prev = l;
                }
            }
        }
    }
    for j in add {
        region[j] = true;
    }
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
        // P0-4: `Rect` IS the square dilation every earlier build ran, so
        // the new 拡縮方法 field costs the default fill nothing.
        assert_eq!(d.expand_mode, ExpandMode::Rect, "the square ball as before");

        // And prove it end to end: the same box fills the same way as
        // before through the defaults.
        let mut doc = Document::new(128, 128);
        draw_box_with_gap(&mut doc, 20, 20, 100, 100, 0);
        assert!(bucket_fill(&mut doc, (60, 60), [1.0, 0.0, 0.0], &d) > 0);
        assert!(px(&doc, 60, 60)[3] > 0, "inside filled");
        assert_eq!(px(&doc, 5, 5)[3], 0, "outside untouched");
        // Row 40/120 joins the defaults contract: OFF, exactly as before.
        assert!(!d.semi_transparent_paper);
    }

    /// Row 40/120 (CSP 半透明を透明にする): the antialiased skirt of a
    /// line is PAPER to the flood when the switch is on — the fill runs
    /// under the fringe to the dark core and the flat shows no halo; OFF,
    /// skirt walls the fill exactly as every earlier build did. The fixture
    /// is TRUE alpha AA (black ink, alpha ramp) — what a rendered stroke
    /// actually is; over white it composites to the same greys the old
    /// opaque-skirt fixture drew, so the assertions carry over verbatim.
    #[test]
    fn semi_transparent_paper_runs_the_fill_under_the_skirt() {
        fn vline(doc: &mut Document, li: usize, x: i32, a: u8) {
            for y in 0..128i32 {
                let idx = TileIdx::of_pixel(x, y);
                let (ox, oy) = idx.origin();
                let t = doc.layers[li].tile_mut(idx);
                let d = t.data_mut();
                let o = ((y - oy) as usize * crate::tile::TILE_SIZE + (x - ox) as usize) * 4;
                d[o] = 0;
                d[o + 1] = 0;
                d[o + 2] = 0;
                d[o + 3] = f32_to_fix15(a as f32 / 255.0);
            }
        }
        let fill = |on: bool| {
            let mut doc = Document::new(128, 128);
            let li = doc.add_layer("line");
            // Hand-made AA, as opacity: light skirt → mid skirt → dark core.
            for (x, a) in [(62, 45u8), (63, 95), (64, 215), (65, 95), (66, 45)] {
                vline(&mut doc, li, x, a);
            }
            let opts = FillOpts {
                tolerance: 0.05,
                gap_close_px: 0,
                expand_px: 0,
                semi_transparent_paper: on,
                ..FillOpts::default()
            };
            assert!(bucket_fill(&mut doc, (10, 64), [1.0, 0.0, 0.0], &opts) > 0);
            doc
        };
        let is_red = |doc: &Document, x: i32, y: i32| {
            let p = px(doc, x, y);
            p[0] > 30_000 && p[1] < 1_000
        };
        let off = fill(false);
        assert!(is_red(&off, 61, 64), "up to the skirt, as ever");
        assert!(!is_red(&off, 62, 64), "OFF: the light skirt walls the fill");
        assert_eq!(
            px(&off, 64, 64)[3],
            f32_to_fix15(215.0 / 255.0),
            "core untouched"
        );
        let on = fill(true);
        assert!(is_red(&on, 61, 64));
        assert!(is_red(&on, 62, 64), "the skirt is paper now");
        assert!(is_red(&on, 63, 64), "all the way to the core");
        assert!(!is_red(&on, 64, 64), "the dark core stays a wall");
        assert_eq!(px(&on, 64, 64)[3], f32_to_fix15(215.0 / 255.0));
        assert!(!is_red(&on, 66, 64), "the far side is a separate region");
    }

    /// C-005 (対象色): the Target-colour classes. Fixture: three bands —
    /// opaque black, opaque white, untouched (transparent) — and one
    /// assertion per mode about which bands a flood can cross. The
    /// transparent band reads as WHITE in the composite-over-white RGB
    /// (that is the paper), so only the alpha canvas separates it —
    /// exactly what the class test uses.
    #[test]
    fn target_colour_decides_what_walls_and_what_fills() {
        fn band(doc: &mut Document, li: usize, x0: i32, x1: i32, v: u8) {
            for y in 0..128i32 {
                for x in x0..x1 {
                    let idx = TileIdx::of_pixel(x, y);
                    let (ox, oy) = idx.origin();
                    let d = doc.layers[li].tile_mut(idx).data_mut();
                    let o = ((y - oy) as usize * TILE_SIZE + (x - ox) as usize) * 4;
                    let f = f32_to_fix15(v as f32 / 255.0);
                    d[o] = f;
                    d[o + 1] = f;
                    d[o + 2] = f;
                    d[o + 3] = f32_to_fix15(1.0);
                }
            }
        }
        let build = || {
            let mut doc = Document::new(128, 128);
            let li = doc.add_layer("art");
            band(&mut doc, li, 10, 42, 0); // black
            band(&mut doc, li, 52, 84, 255); // white
            doc
        };
        let opts = |close: FillClose| FillOpts {
            tolerance: 0.05,
            gap_close_px: 0,
            expand_px: 0,
            close,
            ..FillOpts::default()
        };
        let covers = |doc: &Document, close: FillClose, seed: (i32, i32), x: i32| {
            let r = flood_region(doc, seed, &opts(close)).expect("region");
            r[64 * 128 + x as usize]
        };
        let doc = build();
        // Only transparent: the untouched band fills, both inks wall.
        assert!(covers(&doc, FillClose::OnlyTransparent, (100, 64), 90));
        assert!(!covers(&doc, FillClose::OnlyTransparent, (100, 64), 20));
        assert!(!covers(&doc, FillClose::OnlyTransparent, (100, 64), 60));
        // Other than transparent: the ink band fills and the transparent
        // gap (42..52) walls the crossing — drawn regions are separate.
        assert!(covers(&doc, FillClose::NotTransparent, (20, 64), 15));
        assert!(!covers(&doc, FillClose::NotTransparent, (20, 64), 60));
        assert!(!covers(&doc, FillClose::NotTransparent, (20, 64), 100));
        // Only black: the black band fills, nothing else.
        assert!(covers(&doc, FillClose::OnlyBlack, (20, 64), 15));
        assert!(!covers(&doc, FillClose::OnlyBlack, (20, 64), 60));
        // Other than black: white and transparent fill.
        assert!(covers(&doc, FillClose::NotBlack, (60, 64), 100));
        assert!(!covers(&doc, FillClose::NotBlack, (60, 64), 20));
        // White and transparent: everything but the black band.
        assert!(covers(&doc, FillClose::WhiteAndTransparent, (60, 64), 100));
        assert!(!covers(&doc, FillClose::WhiteAndTransparent, (60, 64), 20));
        // Other than white and transparent: only the black band.
        assert!(covers(&doc, FillClose::NotWhiteAndTransparent, (20, 64), 15));
        assert!(!covers(
            &doc,
            FillClose::NotWhiteAndTransparent,
            (20, 64),
            60
        ));
        assert!(!covers(
            &doc,
            FillClose::NotWhiteAndTransparent,
            (20, 64),
            100
        ));
    }

    /// The verdict's own divergence case (owner, 2026-08-27): a PALE but
    /// OPAQUE tone is a WALL under the switch — opacity is the test, not
    /// brightness. The old luma rule read this band as paper and the fill
    /// leaked straight through it.
    #[test]
    fn semi_transparency_tests_opacity_not_luma() {
        let mut doc = Document::new(128, 128);
        let li = doc.add_layer("tone");
        // Pale yellow, fully opaque — bright to look at, a real wall.
        for y in 0..128i32 {
            let idx = TileIdx::of_pixel(62, y);
            let (ox, oy) = idx.origin();
            let t = doc.layers[li].tile_mut(idx);
            let d = t.data_mut();
            let o = ((y - oy) as usize * crate::tile::TILE_SIZE + (62 - ox) as usize) * 4;
            d[o] = f32_to_fix15(250.0 / 255.0);
            d[o + 1] = f32_to_fix15(240.0 / 255.0);
            d[o + 2] = f32_to_fix15(180.0 / 255.0);
            d[o + 3] = f32_to_fix15(1.0);
        }
        let opts = FillOpts {
            tolerance: 0.05,
            gap_close_px: 0,
            expand_px: 0,
            semi_transparent_paper: true,
            ..FillOpts::default()
        };
        assert!(bucket_fill(&mut doc, (10, 62), [1.0, 0.0, 0.0], &opts) > 0);
        let p = px(&doc, 63, 62);
        assert_eq!(p[3], 0, "the pale opaque band walls the fill (opacity, not luma)");
        assert!(
            px(&doc, 61, 62)[0] > 30_000 && px(&doc, 61, 62)[1] < 1_000,
            "the near side did fill"
        );
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

    // --- GPU audit queue #2: the morphology rewrite ----------------------

    /// The pass the rewrite replaced, kept here as the ORACLE: one 3×3
    /// 8-connected dilation over the whole canvas, clamped at the border.
    fn dilate_once(mask: &[bool], w: usize, h: usize) -> Vec<bool> {
        let mut out = mask.to_vec();
        for y in 0..h {
            for x in 0..w {
                if mask[y * w + x] {
                    continue;
                }
                let x1 = (x + 1).min(w - 1);
                let y1 = (y + 1).min(h - 1);
                'n: for ny in y.saturating_sub(1)..=y1 {
                    for nx in x.saturating_sub(1)..=x1 {
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

    /// An L: a shape with no symmetry in either axis and two concave
    /// corners, so a window that got its bounds off by one anywhere shows
    /// up. Touches the canvas edge too, to pin the border clamping.
    fn l_shape(w: usize, h: usize) -> Vec<bool> {
        let mut m = vec![false; w * h];
        for y in 6..26 {
            for x in 0..7 {
                m[y * w + x] = true; // the tall stroke, flush to x = 0
            }
        }
        for y in 21..26 {
            for x in 0..19 {
                m[y * w + x] = true; // the foot
            }
        }
        m
    }

    /// The equivalence the whole rewrite rests on: N rounds of the old 3×3
    /// dilation ARE the Chebyshev ball of radius N, so one window pass may
    /// stand in for N passes. Checked pixel for pixel on the L, at every
    /// radius the UI and auto mode can produce, and with the bbox clip on —
    /// which is the "bbox-clip changes nothing" identity as well, since a
    /// full-canvas rect and the mask's own bbox+r must agree exactly.
    #[test]
    fn window_dilate_equals_the_iterated_3x3_pass() {
        let (w, h) = (32usize, 32usize);
        let base = l_shape(w, h);
        for r in 0..=8usize {
            let mut oracle = base.clone();
            for _ in 0..r {
                oracle = dilate_once(&oracle, w, h);
            }

            let mut windowed = base.clone();
            dilate_by(&mut windowed, w, &mask_rect(&base, w, h, r), r, false);
            assert_eq!(windowed, oracle, "bbox-clipped window disagrees at r={r}");

            let mut full = base.clone();
            let whole = Rect {
                x0: 0,
                y0: 0,
                x1: w,
                y1: h,
            };
            dilate_by(&mut full, w, &whole, r, false);
            assert_eq!(full, oracle, "full-canvas window disagrees at r={r}");
            assert_eq!(full, windowed, "the bbox clip moved a pixel at r={r}");

            // And the erosion identity, against the same oracle applied to
            // the complement.
            let inv: Vec<bool> = base.iter().map(|&b| !b).collect();
            let mut eroded_oracle = inv.clone();
            for _ in 0..r {
                eroded_oracle = dilate_once(&eroded_oracle, w, h);
            }
            let eroded_oracle: Vec<bool> = eroded_oracle.iter().map(|&b| !b).collect();
            let mut eroded = base.clone();
            erode_by(&mut eroded, w, &mask_rect(&base, w, h, r), r, false);
            assert_eq!(eroded, eroded_oracle, "erosion disagrees at r={r}");
        }
    }

    /// P0-4 Round: a disc is inside the square it fits in, so it can only
    /// ever cover LESS — and strictly less as soon as a corner is involved.
    /// A diagonal stroke is corners all the way down.
    #[test]
    fn round_area_scaling_covers_less_than_square_on_a_diagonal() {
        let (w, h) = (64usize, 64usize);
        let mut diag = vec![false; w * h];
        for i in 8..56 {
            diag[i * w + i] = true;
        }
        let rect_r = mask_rect(&diag, w, h, 4);
        let mut square = diag.clone();
        dilate_by(&mut square, w, &rect_r, 4, false);
        let mut round = diag.clone();
        dilate_by(&mut round, w, &rect_r, 4, true);
        let n = |m: &[bool]| m.iter().filter(|&&b| b).count();
        assert!(
            n(&round) < n(&square),
            "round must be the smaller ball ({} vs {})",
            n(&round),
            n(&square)
        );
        // Subset, not merely smaller.
        assert!(
            round.iter().zip(&square).all(|(r, s)| !r || *s),
            "the disc must sit inside the square"
        );
        // The corner of the square 4 px out on BOTH axes is outside a
        // radius-4 disc (4²+4² > 4²); the straight 4 px out is inside it.
        let mid = 32usize;
        assert!(square[(mid - 4) * w + mid + 4], "square reaches the corner");
        assert!(!round[(mid - 4) * w + mid + 4], "the disc does not");
        assert!(round[mid * w + mid + 4], "but it does reach straight out");
    }

    /// P0-4 ToDarkest (最も濃いピクセルまで), the mode this exists for: a
    /// 1 px black line with a soft anti-aliased edge on either side. A
    /// 4 px SQUARE expansion walks straight over the whole line and out the
    /// far side — the overshoot that ruins a fill against thin lineart.
    /// ToDarkest steps through the near skirt, stops ON the black column,
    /// and refuses the lighter pixel past it.
    #[test]
    fn to_darkest_stops_on_the_line_instead_of_stepping_over_it() {
        let (w, h) = (128usize, 128i32);
        let mut doc = Document::new(w as u32, h as u32);
        // Columns 59 | 60 | 61 | 62 | 63 = paper | skirt | CORE | skirt | paper
        let skirt = [0, 0, 0, (FIX15_ONE / 4) as u16];
        for y in 0..h {
            paint_px(&mut doc, 60, y, skirt);
            paint(&mut doc, 61, y);
            paint_px(&mut doc, 62, y, skirt);
        }
        let base = FillOpts {
            gap_close_px: 0,
            expand_px: 4,
            ..Default::default()
        };
        let row = 64usize * w;

        let square = flood_region(&doc, (30, 64), &base).expect("region");
        assert!(square[row + 59], "the flood stops at the skirt");
        assert!(
            square[row + 62] && square[row + 63],
            "a 4 px square expansion walks over the line and out the far side"
        );

        let darkest = flood_region(
            &doc,
            (30, 64),
            &FillOpts {
                expand_mode: ExpandMode::ToDarkest,
                ..base
            },
        )
        .expect("region");
        assert!(darkest[row + 60], "it tucks under the pale skirt");
        assert!(darkest[row + 61], "and lands ON the darkest column");
        assert!(
            !darkest[row + 62],
            "and refuses the lighter pixel past the core — no overshoot"
        );
        assert!(!darkest[row + 63], "so the far side stays clean");
        // The left-hand boundary is the page edge, not a line: nothing to
        // aim at, so nothing is invented there.
        assert!(darkest[row], "the area itself is intact");
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


    /// A-016 (色余白, colour margin): a pale-but-opaque tone beside the
    /// line folds INTO the wall once the margin covers the colour
    /// distance — at margin 0 it stays paintable.
    #[test]
    fn colour_margin_folds_near_miss_ink_into_the_wall() {
        let mut doc = Document::new(64, 64);
        let li = doc.add_layer("ref");
        let t = doc.layers[li].tile_mut(TileIdx::new(0, 0));
        // A dark line core and a pale opaque tone 200 away per channel
        // (below the luma midpoint, so ONLY the margin can wall it).
        t.set_pixel(10, 10, [0, 0, 0, 32768]);
        let grey = f32_to_fix15(200.0 / 255.0);
        t.set_pixel(30, 10, [grey, grey, grey, 32768]);
        doc.set_layer_reference(li, true);
        let (_, allow0) = anti_overflow_barrier(&doc, 0, false).expect("refs");
        assert_eq!(allow0[10 * 64 + 10], 0, "the dark core walls, as ever");
        assert_eq!(allow0[10 * 64 + 30], 255, "margin 0: the pale tone paints");
        let (_, allow_m) = anti_overflow_barrier(&doc, 220, false).expect("refs");
        assert_eq!(allow_m[10 * 64 + 30], 0, "the margin folded the tone in");
    }

    /// A-015 (ベクトルまで塗り): a VECTOR reference layer walls at its
    /// strokes' centrelines — the 1 px spine — and nothing else of it.
    #[test]
    fn vector_reference_walls_at_the_centreline_when_asked() {
        let mut doc = Document::new(64, 64);
        let li = doc.add_layer("vector");
        doc.layers[li].strokes = Some(crate::stroke_set::StrokeSet {
            strokes: vec![crate::stroke_set::VectorStroke {
                points: (0..=32)
                    .map(|i| (32.0 + i as f32, 32.0, 0.9, 0.0, 0.0, i as f64 * 8.0))
                    .collect(),
                preset: "pen".into(),
                size_px: 20.0,
                color: [0, 0, 0],
                eraser: false,
                stabilizer: 0.0,
                width_scale: 1.0,
            }],
        });
        doc.set_layer_reference(li, true);
        // Centreline OFF (the old behaviour): the layer has no rendered
        // tiles here, so nothing walls.
        let (_, allow_off) = anti_overflow_barrier(&doc, 0, false).expect("refs");
        assert_eq!(allow_off[32 * 64 + 40], 255, "off: no wall without ink");
        // Centreline ON: the spine walls, 2 px off it does not.
        let (_, allow_on) = anti_overflow_barrier(&doc, 0, true).expect("refs");
        assert_eq!(allow_on[32 * 64 + 40], 0, "on: the spine walls");
        assert_eq!(allow_on[34 * 64 + 40], 255, "2 px above the spine paints");
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

    /// Row 42: the brush anti-overflow barrier — the REFERENCE SET is the
    /// only referent (owner ruling 2026-08-25, overturning the widened
    /// one): reference ink blocks, frame-border ink does NOT (a frame
    /// folder clips its own children itself; a lower page layer is
    /// covered at composite), and a document with nothing to refer to
    /// yields None (the toggle is an honest no-op) — even when frame
    /// folders exist.
    #[test]
    fn anti_overflow_barrier_blocks_reference_ink_only() {
        let mut doc = Document::new(128, 128);
        assert!(
            anti_overflow_barrier(&doc, 0, false).is_none(),
            "nothing to refer to — no mask"
        );

        // A frame folder alone is still nothing to refer to.
        let fs = crate::frame::FrameSet::single_rect([16.0, 16.0, 112.0, 112.0], 4.0);
        doc.add_frame_folder("panel", fs);
        assert!(
            anti_overflow_barrier(&doc, 0, false).is_none(),
            "frame folders are not referents — no mask from them alone"
        );

        // A reference layer with a vertical black line at x=10.
        let r = doc.add_layer("ref");
        doc.layers[r].reference = true;
        for y in 0..128i32 {
            let idx = TileIdx::of_pixel(10, y);
            let (ox, oy) = idx.origin();
            let t = doc.layers[r].tile_mut(idx);
            let d = t.data_mut();
            let o = ((y - oy) as usize * TILE_SIZE + (10 - ox) as usize) * 4;
            let f = f32_to_fix15(0.0);
            d[o] = f;
            d[o + 1] = f;
            d[o + 2] = f;
            d[o + 3] = f32_to_fix15(1.0);
        }

        let (w, allow) = anti_overflow_barrier(&doc, 0, false).expect("references exist");
        assert_eq!(w, 128);
        let at = |x: usize, y: usize| allow[y * 128 + x];
        assert_eq!(at(64, 64), 255, "panel paper is paintable");
        assert_eq!(at(0, 0), 255, "gutter paper is paintable");
        assert_eq!(at(10, 64), 0, "the reference line blocks");
        assert_eq!(
            at(16, 64),
            255,
            "the frame border no longer walls strokes globally"
        );
        assert_eq!(at(20, 64), 255, "just inside the border is paintable");
    }
}
