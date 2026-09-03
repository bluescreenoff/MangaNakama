//! Print preflight (TRIAGE 132, CSP `PM-060`'s binding-list Confirm column):
//! pure predicates over the work's metadata, page geometry, and page
//! content. No I/O, no app state — the Preflight palette (part 2) consumes
//! `run_work` + `run_page`; tests drive both from plain values.
//!
//! Severity model: **Error** = the page cannot go to a printer as-is
//! (no trim, text outside the trim); **Warn** = it can, but a printer
//! will complain or the result will be wrong (no bleed, colour on a mono
//! work, non-standard finish). CSP's list is a flat list of sentences —
//! we keep their shape, plus a stable check id per finding so the UI (and
//! tests) can key off rows without parsing prose.
//!
//! Scope honesty (2026-08-17, part 1): the predicates that map onto data
//! this tree actually holds. CSP's fifteen also include per-page setup
//! divergence and mixed per-layer resolutions — our model has ONE shared
//! `PageSetup` and one work DPI, so those are structurally impossible and
//! not checked; folio checks wait on a folio concept (our margin print is
//! outside the trim BY DESIGN).

use crate::doc::{Document, LayerKind};
use crate::fill_layer::FillKind;
use crate::page::PageSetup;
use crate::project::ProjectMeta;
use crate::text::TextItem;
use crate::tile::{FIX15_ONE, TILE_SIZE, Tile, TileIdx};
use crate::tone::TonePattern;
use std::collections::HashSet;

/// A chromatic pixel's channel spread above this (fix15 units, ~1.5% of
/// full scale) reads as colour on a mono work — quantization-safe.
const CHROMA_ULP: u16 = 491;

/// CSP's text-vs-trim rule is a fixed 5 mm, independent of the work's own
/// safety margins.
const TEXT_SAFE_MM: f32 = 5.0;

/// Two overlapping screens count as the SAME screen when their numbers
/// agree this closely. lpi and angle are typed by hand into three
/// different panels, so exact float equality would report every
/// re-entered 60 as a clash, and half a line per inch (or half a degree)
/// is not a difference a printer can resolve.
const TONE_LPI_EPS: f32 = 0.5;
const TONE_ANGLE_EPS: f32 = 0.5;

/// Alpha this far from 0 / full is a fractional (anti-aliased) edge pixel.
/// Same margin as [`CHROMA_ULP`] and for the same reason: a source that
/// round-tripped through 8-bit lands a unit or two off the rails.
const ALPHA_ULP: u16 = 491;

/// Fewer fractional-alpha pixels than this is stray quantization, not a
/// soft edge — a genuinely anti-aliased shape carries them along its whole
/// outline, so the threshold costs no real detections and kills the
/// one-stray-pixel false positive.
const GREY_EDGE_MIN_PX: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreflightLevel {
    /// The page cannot go to a printer as-is.
    Error,
    /// It can, but a printer will complain or the result will be wrong.
    Warn,
}

#[derive(Clone, Debug)]
pub struct PreflightFinding {
    /// Stable id (`"bleed.unset"`) — the UI and tests key off this, not
    /// the prose.
    pub check: &'static str,
    pub level: PreflightLevel,
    pub message: String,
}

fn warn(check: &'static str, message: String) -> PreflightFinding {
    PreflightFinding {
        check,
        level: PreflightLevel::Warn,
        message,
    }
}

fn error(check: &'static str, message: String) -> PreflightFinding {
    PreflightFinding {
        check,
        level: PreflightLevel::Error,
        message,
    }
}

/// Work-level checks: metadata and geometry only, no page decoding. Run
/// once for the whole work; `page_count` is the reading-order page count.
pub fn run_work(meta: &ProjectMeta, page_count: usize) -> Vec<PreflightFinding> {
    let mut out = Vec::new();
    let Some(setup) = &meta.setup else {
        out.push(error(
            "setup.absent",
            "no page setup — the work is a pixel canvas with no trim/bleed guidance".into(),
        ));
        return out;
    };
    if setup.trim_mm == setup.paper_mm {
        out.push(error(
            "trim.unset",
            "trim border not set — the trim equals the paper, so nothing marks the cut".into(),
        ));
    }
    if setup.bleed_mm <= 0.0 {
        out.push(warn(
            "bleed.unset",
            "bleed not set — art touching the trim will show paper slivers after the cut".into(),
        ));
    } else if !(3.0..=5.0).contains(&setup.bleed_mm) {
        out.push(warn(
            "bleed.range",
            format!(
                "bleed {} mm is outside the usual 3–5 mm — confirm with the printer",
                setup.bleed_mm
            ),
        ));
    }
    let standard = PageSetup::presets()
        .iter()
        .any(|p| p.trim_mm == setup.trim_mm);
    if !standard {
        out.push(warn(
            "finish.nonstandard",
            format!(
                "finish size {}×{} mm is not a standard preset — confirm the trim",
                setup.trim_mm.0, setup.trim_mm.1
            ),
        ));
    }
    if meta.spine_mm <= 0.0 {
        out.push(warn(
            "spine.unset",
            "spine width not set — perfect binding needs one (0 until the printer says)".into(),
        ));
    }
    match meta.cover {
        None if page_count > 1 => out.push(warn(
            "cover.missing",
            "no cover page set for a multi-page work".into(),
        )),
        Some(i) if i >= page_count => out.push(error(
            "cover.out_of_range",
            format!("cover points at page {} of {}", i + 1, page_count),
        )),
        _ => {}
    }
    // Publisher profile (M2): the picked target's norms become checks —
    // offset printing's 台 rule, and paper geometry drifting away from
    // what the profile restated at pick time.
    if let Some(p) = &meta.profile {
        if let Some(m) = p.page_count_multiple
            && m > 0
            && page_count % m as usize != 0
        {
            out.push(warn(
                "profile.page_count",
                format!(
                    "{page_count} pages is not a multiple of {m} — \"{}\" binds in sheets of {m}",
                    p.name
                ),
            ));
        }
        // The work's RESOLUTION against the target's (audit finding 10,
        // honest v1). `profile.setup_drift` below compares paper and trim
        // only, and dpi is the half that cannot be fixed by re-applying
        // the profile: there is no resample command yet, so this row
        // exists to say the number out loud rather than to offer a fix.
        // A pixel canvas (dpi 0) mismatches EVERY print target, and
        // saying "this work is 0 dpi" would be a lie about what it is.
        if p.setup.dpi > 0 && setup.dpi != p.setup.dpi {
            let mine = if setup.dpi == 0 {
                "this work is a pixel canvas with no dpi".to_string()
            } else {
                format!("this work is {} dpi", setup.dpi)
            };
            out.push(warn(
                "profile.dpi",
                format!(
                    "{mine}; \"{}\" expects {} — there is no resample yet, so the \
                     art has to be redrawn or re-exported at the target's resolution",
                    p.name, p.setup.dpi
                ),
            ));
        }
        if p.setup.paper_mm != setup.paper_mm || p.setup.trim_mm != setup.trim_mm {
            out.push(warn(
                "profile.setup_drift",
                format!(
                    "the work's paper/trim no longer matches \"{}\" — re-apply the profile \
                     in Work Settings or confirm the change is deliberate",
                    p.name
                ),
            ));
        }
    }
    out
}

/// Axis-aligned bounds of a text item's layout box under its rotation.
fn text_aabb(t: &TextItem) -> [f32; 4] {
    let (s, c) = t.rotation.sin_cos();
    let (s, c) = (s.abs(), c.abs());
    let hx = (t.size[0] * c + t.size[1] * s) * 0.5;
    let hy = (t.size[0] * s + t.size[1] * c) * 0.5;
    let cx = t.pos[0] + t.size[0] * 0.5;
    let cy = t.pos[1] + t.size[1] * 0.5;
    [cx - hx, cy - hy, cx + hx, cy + hy]
}

/// One tone-bearing thing on a page, reduced to what moiré cares about.
///
/// The three carriers print the SAME screen through different plumbing —
/// `Layer::tone` (painted pixels are the ink source), a `FillKind::Tone`
/// layer (the layer mask is the window), and a balloon's `fill_tone`
/// (pure geometry, and its cell is stored in px, not lpi). Comparing them
/// in one flat list is the whole point: the pairs that actually ring in
/// print are usually a tone layer under a toned balloon, and a check that
/// only knew about `Layer::tone` would never see them.
struct ToneCarrier<'a> {
    /// Names the offender in the finding: `Layer "sky"`, `Balloon 2 on …`.
    label: String,
    lpi: f32,
    angle_deg: f32,
    pattern: TonePattern,
    /// Coverage, coarse: the 64 px tiles this carrier can print in.
    tiles: HashSet<TileIdx>,
    /// The pixels the screen reads its ink from, when there are any — a
    /// balloon's interior is geometry, so it has none. `tone.grey_edge`
    /// samples these.
    source: Vec<(TileIdx, &'a Tile)>,
}

/// Round for display: a balloon's lpi is a division, so it arrives as
/// 46.153847 and prints as 46.2.
fn round1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

impl ToneCarrier<'_> {
    /// `Layer "sky" 60 lpi/45° Dots` — the numbers the user re-entered by
    /// hand are exactly the ones he needs to compare.
    fn describe(&self) -> String {
        format!(
            "{} {} lpi/{}° {}",
            self.label,
            round1(self.lpi),
            round1(self.angle_deg),
            self.pattern.label()
        )
    }

    fn overlaps(&self, other: &ToneCarrier) -> bool {
        let (a, b) = if self.tiles.len() <= other.tiles.len() {
            (&self.tiles, &other.tiles)
        } else {
            (&other.tiles, &self.tiles)
        };
        a.iter().any(|t| b.contains(t))
    }

    fn clashes_with(&self, other: &ToneCarrier) -> bool {
        if self.pattern != other.pattern || (self.lpi - other.lpi).abs() > TONE_LPI_EPS {
            return true;
        }
        // Screen angle is periodic: 179° and 1° are 2° apart, not 178°.
        let d = (self.angle_deg - other.angle_deg).rem_euclid(180.0);
        d.min(180.0 - d) > TONE_ANGLE_EPS
    }
}

/// The tiles a canvas-px rect touches, clipped to the canvas. An empty
/// polygon's infinite bbox saturates the casts and yields an empty range,
/// which is the right answer for a balloon with no points.
fn rect_tiles(r: [f32; 4], size: (u32, u32)) -> HashSet<TileIdx> {
    let t = TILE_SIZE as i32;
    let last = |n: u32| ((n as usize).div_ceil(TILE_SIZE) as i32 - 1).max(0);
    let (lx, ly) = (last(size.0), last(size.1));
    let x0 = (r[0].floor() as i32).div_euclid(t).clamp(0, lx);
    let y0 = (r[1].floor() as i32).div_euclid(t).clamp(0, ly);
    let x1 = (r[2].ceil() as i32).div_euclid(t).clamp(0, lx);
    let y1 = (r[3].ceil() as i32).div_euclid(t).clamp(0, ly);
    let mut out = HashSet::new();
    for y in y0..=y1 {
        for x in x0..=x1 {
            out.insert(TileIdx::new(x, y));
        }
    }
    out
}

/// Every visible tone carrier on the page, bottom of the stack first.
///
/// `Noise` drops out here: it is an FM screen with no lattice, so it has
/// nothing to interfere with and nothing a grey edge can degrade. Layers that
/// do not print drop out for the reason given on [`run_page`]'s `printed`.
fn tone_carriers<'a>(setup: &PageSetup, doc: &'a Document, printed: &[bool]) -> Vec<ToneCarrier<'a>> {
    let mut out: Vec<ToneCarrier<'a>> = Vec::new();
    for (i, layer) in doc.layers.iter().enumerate() {
        if !printed[i] {
            continue;
        }
        if let Some(t) = &layer.tone {
            let source: Vec<_> = layer.tiles().map(|(i, t)| (i, &**t)).collect();
            out.push(ToneCarrier {
                label: format!("Layer {:?}", layer.name),
                lpi: t.lpi,
                angle_deg: t.angle_deg,
                pattern: t.pattern,
                tiles: source.iter().map(|(i, _)| *i).collect(),
                source,
            });
        }
        match &layer.kind {
            LayerKind::Fill(FillKind::Tone { tone, .. }) => {
                // The window rule copied from `Layer::refresh_fill`: the
                // mask is the coverage, and NO mask windows the whole
                // canvas (the adjustment-layer convention). `enabled` is
                // not consulted there either.
                let source: Vec<_> = layer
                    .mask
                    .iter()
                    .flat_map(|m| m.tiles.iter().map(|(i, t)| (*i, &**t)))
                    .collect();
                let tiles = if layer.mask.is_some() {
                    source.iter().map(|(i, _)| *i).collect()
                } else {
                    rect_tiles([0.0, 0.0, doc.size.0 as f32, doc.size.1 as f32], doc.size)
                };
                out.push(ToneCarrier {
                    label: format!("Layer {:?}", layer.name),
                    lpi: tone.lpi,
                    angle_deg: tone.angle_deg,
                    pattern: tone.pattern,
                    tiles,
                    source,
                });
            }
            LayerKind::Balloon(bs) => {
                for (n, b) in bs.balloons.iter().enumerate() {
                    let Some(bt) = b.fill_tone else { continue };
                    out.push(ToneCarrier {
                        label: format!("Balloon {} on {:?}", n + 1, layer.name),
                        // `BalloonTone` stores its cell in canvas px by
                        // design (it rasterizes from paths that carry no
                        // dpi) — the page dpi is the only thing that turns
                        // it back into a number comparable with a tone
                        // layer's lpi.
                        lpi: setup.dpi as f32 / bt.cell_px.max(1.0),
                        angle_deg: bt.angle_deg,
                        pattern: bt.pattern,
                        tiles: rect_tiles(b.bbox(), doc.size),
                        source: Vec::new(),
                    });
                }
            }
            _ => {}
        }
    }
    out.retain(|c| c.pattern != TonePattern::Noise);
    out
}

/// Fractional alpha along the ink's outline.
///
/// Only the tiles at the EDGE of the painted set are sampled: a soft edge
/// lives where the coverage ends, and walking a filled region's interior
/// at 600 dpi is millions of pixels for a boolean.
fn has_grey_edges(source: &[(TileIdx, &Tile)]) -> bool {
    let filled: HashSet<TileIdx> = source.iter().map(|(i, _)| *i).collect();
    let hi = FIX15_ONE as u16 - ALPHA_ULP;
    let mut n = 0usize;
    for (idx, tile) in source {
        let interior = [(1, 0), (-1, 0), (0, 1), (0, -1)]
            .iter()
            .all(|(dx, dy)| filled.contains(&TileIdx::new(idx.x + dx, idx.y + dy)));
        if interior {
            continue;
        }
        for px in tile.data().chunks_exact(4) {
            if px[3] > ALPHA_ULP && px[3] < hi {
                n += 1;
                if n >= GREY_EDGE_MIN_PX {
                    return true;
                }
            }
        }
    }
    false
}

/// Page-level checks over the page's CONTENT: lettering vs the trim,
/// colour on a mono work, and the moiré pair (TOP-15 #1 — CSP's own tips
/// article 9181 calls getting moiré "a coin flip", because nothing in CSP
/// ever tells the user which two tones are fighting).
/// `page_index` names the page in findings.
pub fn run_page(
    setup: &PageSetup,
    meta: &ProjectMeta,
    page_index: usize,
    doc: &Document,
) -> Vec<PreflightFinding> {
    let mut out = Vec::new();
    let trim = setup.trim_rect_px();
    let safe_px = TEXT_SAFE_MM / 25.4 * setup.dpi as f32;
    // Which layers are actually ON the printed page. Folded exactly the
    // way the export composite folds them (`composite_size` in `export.rs`:
    // `effective_visibility`, then drafts knocked out of it), because
    // preflight is a report about the print and a layer that will not be
    // printed cannot be a finding. Both cascade through folders: hiding or
    // drafting a folder takes everything inside it with it, and a rough
    // normally lives in one folder rather than one layer.
    let mut printed = doc.effective_visibility();
    let drafts = doc.effective_drafts();
    for (p, d) in printed.iter_mut().zip(&drafts) {
        if *d {
            *p = false;
        }
    }
    // The trim rect can sit outside the canvas or touch it; text boxes
    // compare in the same px space `TextItem.pos` uses (canvas px).
    for (i, layer) in doc.layers.iter().enumerate() {
        if !printed[i] {
            continue;
        }
        let Some(ts) = layer.texts() else { continue };
        for t in &ts.texts {
            let b = text_aabb(t);
            let label: String = t.text.chars().take(12).collect();
            if b[0] < trim[0] || b[1] < trim[1] || b[2] > trim[2] || b[3] > trim[3] {
                out.push(error(
                    "text.outside_trim",
                    format!(
                        "page {}: text {:?} sticks out of the trim",
                        page_index + 1,
                        label
                    ),
                ));
            } else if b[0] < trim[0] + safe_px
                || b[1] < trim[1] + safe_px
                || b[2] > trim[2] - safe_px
                || b[3] > trim[3] - safe_px
            {
                out.push(warn(
                    "text.margin",
                    format!(
                        "page {}: text {:?} sits within {} mm of the trim",
                        page_index + 1,
                        label,
                        TEXT_SAFE_MM
                    ),
                ));
            }
        }
    }
    if meta.expression == crate::project::Expression::Mono {
        for (i, layer) in doc.layers.iter().enumerate() {
            // A 下書き layer never reaches the printer, so its colour
            // cannot land on a mono page. Roughing in blue is universal,
            // so without this the check fires on every page of every
            // chapter and teaches the artist to ignore preflight.
            if !printed[i] || layer.is_vector() {
                continue;
            }
            'tiles: for (_, tile) in layer.tiles() {
                for px in tile.data().chunks_exact(4) {
                    let (r, g, b) = (px[0], px[1], px[2]);
                    let spread = r.max(g).max(b).saturating_sub(r.min(g).min(b));
                    if spread > CHROMA_ULP {
                        out.push(warn(
                            "expression.colour_on_mono",
                            format!(
                                "page {}: layer {:?} holds colour a mono print cannot reproduce",
                                page_index + 1,
                                layer.name
                            ),
                        ));
                        break 'tiles;
                    }
                }
            }
        }
    }
    let carriers = tone_carriers(setup, doc, &printed);
    for (i, upper) in carriers.iter().enumerate() {
        // Bottom-first collection, so everything before `i` is underneath.
        for lower in &carriers[..i] {
            if !upper.clashes_with(lower) || !upper.overlaps(lower) {
                continue;
            }
            out.push(warn(
                "tone.clash",
                format!(
                    "page {}: {} over {} — two screens on the same area, \
                     unaligned dots: expect interference rings in print",
                    page_index + 1,
                    upper.describe(),
                    lower.describe()
                ),
            ));
        }
    }
    // Publisher profile (M2): the target's screen ruling is a norm the
    // page's tones can violate. One finding per offending ruling, not per
    // layer — a page toned entirely at 55 lpi is one decision to revisit.
    if let Some(p) = &meta.profile
        && p.lpi > 0.0
    {
        let mut flagged: Vec<f32> = Vec::new();
        for c in &carriers {
            if (c.lpi - p.lpi).abs() <= 0.5 || flagged.iter().any(|f| (f - c.lpi).abs() <= 0.5) {
                continue;
            }
            flagged.push(c.lpi);
            out.push(warn(
                "profile.lpi",
                format!(
                    "page {}: tone at {} lpi — \"{}\" prints {} lpi screens",
                    page_index + 1,
                    round1(c.lpi),
                    p.name,
                    round1(p.lpi),
                ),
            ));
        }
    }
    if meta.expression == crate::project::Expression::Mono {
        for c in &carriers {
            if !has_grey_edges(&c.source) {
                continue;
            }
            out.push(warn(
                "tone.grey_edge",
                format!(
                    "page {}: {} screens a source with anti-aliased grey edges — \
                     grey under a halftone is the classic moiré: threshold the \
                     source to pure black and white",
                    page_index + 1,
                    c.label
                ),
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Expression;

    fn meta(setup: Option<PageSetup>) -> ProjectMeta {
        let mut p = crate::project::Project::new("test".into(), setup, true);
        p.meta.expression = Expression::Mono;
        p.meta.spine_mm = 6.0;
        p.meta.cover = Some(0);
        p.meta
    }

    /// A sane B5-ish setup: trim smaller than paper, 3–5 mm bleed, standard
    /// enough for the presets check (falls back to Warn-only assertions in
    /// the tests that care).
    fn good_setup() -> PageSetup {
        PageSetup::presets().remove(0)
    }

    #[test]
    fn clean_work_has_no_findings() {
        // The first preset is a real manga preset; adjust to be clean.
        let mut s = good_setup();
        s.bleed_mm = 3.0;
        let m = meta(Some(s.clone()));
        let f = run_work(&m, 1);
        let hard: Vec<_> = f
            .iter()
            .filter(|x| x.check != "finish.nonstandard")
            .collect();
        assert!(
            hard.is_empty(),
            "unexpected findings: {:?}",
            hard.iter().map(|x| x.check).collect::<Vec<_>>()
        );
    }

    #[test]
    fn absent_setup_is_the_one_error() {
        let f = run_work(&meta(None), 1);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].check, "setup.absent");
        assert_eq!(f[0].level, PreflightLevel::Error);
    }

    #[test]
    fn geometry_family_fires() {
        let mut s = good_setup();
        s.trim_mm = s.paper_mm; // trim unset
        s.bleed_mm = 0.0;
        let m = meta(Some(s.clone()));
        let f = run_work(&m, 2);
        let ids: Vec<_> = f.iter().map(|x| x.check).collect();
        assert!(ids.contains(&"trim.unset"));
        assert!(ids.contains(&"bleed.unset"));
        assert!(ids.contains(&"spine.unset") == false); // spine set in meta()
        // cover = Some(0) with 2 pages: fine
        assert!(!ids.contains(&"cover.missing"));
        assert_eq!(
            f.iter().find(|x| x.check == "trim.unset").unwrap().level,
            PreflightLevel::Error
        );
    }

    #[test]
    fn bleed_range_and_cover_checks() {
        let mut s = good_setup();
        s.bleed_mm = 8.0;
        let mut m = meta(Some(s.clone()));
        m.cover = None;
        let f = run_work(&m, 3);
        let ids: Vec<_> = f.iter().map(|x| x.check).collect();
        assert!(ids.contains(&"bleed.range"));
        assert!(ids.contains(&"cover.missing"));
        m.cover = Some(3); // == page_count: out of range (0-based index 3 with 3 pages)
        let f = run_work(&m, 3);
        assert!(f.iter().any(|x| x.check == "cover.out_of_range"));
    }

    /// M2: a picked publisher profile turns its norms into checks — the 台
    /// page-count rule, and paper geometry drifting from what the profile
    /// restated. No profile = no findings, byte-for-byte the old preflight.
    #[test]
    fn publisher_profile_norms_fire_and_absence_is_silent() {
        let doujin = crate::profile::PublisherProfile::builtins()
            .into_iter()
            .find(|p| p.page_count_multiple.is_some())
            .expect("a builtin with the 台 rule");
        let mult = doujin.page_count_multiple.unwrap() as usize;
        let mut m = meta(Some(doujin.setup.clone()));
        m.profile = Some(doujin.clone());
        // Wrong count: the 台 rule fires; matching geometry stays silent.
        let f = run_work(&m, mult + 1);
        let ids: Vec<_> = f.iter().map(|x| x.check).collect();
        assert!(ids.contains(&"profile.page_count"), "{ids:?}");
        assert!(!ids.contains(&"profile.setup_drift"), "{ids:?}");
        // Right count: silent.
        let f = run_work(&m, mult * 2);
        assert!(!f.iter().any(|x| x.check == "profile.page_count"));
        // Drift the paper: the drift check fires.
        let mut drifted = m.clone();
        drifted.setup.as_mut().unwrap().paper_mm.0 += 10.0;
        let f = run_work(&drifted, mult);
        assert!(f.iter().any(|x| x.check == "profile.setup_drift"));
        // No profile: neither check can fire.
        m.profile = None;
        let f = run_work(&m, mult + 1);
        assert!(!f.iter().any(|x| x.check.starts_with("profile.")));
    }

    /// Audit finding 10 (honest v1): the work's dpi against the picked
    /// target's, with both numbers and the target NAMED — the geometry
    /// drift check never looked at resolution, so a 350 dpi work aimed at
    /// a 600 dpi publisher passed preflight clean.
    #[test]
    fn profile_dpi_mismatch_names_both_numbers_and_the_target() {
        let target = crate::profile::PublisherProfile::builtins()
            .into_iter()
            .find(|p| p.setup.dpi == 600)
            .expect("a 600 dpi builtin");
        let mut m = meta(Some(target.setup.clone()));
        m.profile = Some(target.clone());
        // Matching resolution: silent.
        assert!(
            !run_work(&m, 4).iter().any(|x| x.check == "profile.dpi"),
            "a work at the target's dpi must not flag"
        );
        // 350 dpi under a 600 dpi target: the audit's own sentence.
        m.setup.as_mut().unwrap().dpi = 350;
        let f = run_work(&m, 4);
        let d = f
            .iter()
            .find(|x| x.check == "profile.dpi")
            .unwrap_or_else(|| panic!("dpi mismatch must flag: {f:?}"));
        assert_eq!(d.level, PreflightLevel::Warn);
        assert!(
            d.message.contains("this work is 350 dpi")
                && d.message.contains("expects 600")
                && d.message.contains(&target.name),
            "the row names the offender: {}",
            d.message
        );
        // Paper/trim are untouched, so this is NOT the drift row wearing
        // a new id — the two checks are independent.
        assert!(
            !f.iter().any(|x| x.check == "profile.setup_drift"),
            "dpi alone must not read as geometry drift: {f:?}"
        );
        // A pixel canvas says what it is instead of claiming 0 dpi.
        m.setup.as_mut().unwrap().dpi = 0;
        let f = run_work(&m, 4);
        let d = f.iter().find(|x| x.check == "profile.dpi").expect("flags");
        assert!(
            d.message.contains("pixel canvas") && !d.message.contains("0 dpi"),
            "a dpi-less canvas is named honestly: {}",
            d.message
        );
        // A target that states no resolution cannot accuse anyone.
        let mut loose = m.clone();
        loose.profile.as_mut().unwrap().setup.dpi = 0;
        assert!(
            !run_work(&loose, 4)
                .iter()
                .any(|x| x.check == "profile.dpi")
        );
    }

    #[test]
    fn text_inside_margin_is_clean_outside_is_error() {
        let s = good_setup();
        let m = meta(Some(s.clone()));
        let mut doc = Document::new(s.paper_px().0, s.paper_px().1);
        let trim = s.trim_rect_px();
        // Centre of the trim: clean.
        let mut t = crate::text::TextItem::new([0.0, 0.0], String::new(), 12.0, [0, 0, 0], false);
        t.pos = [
            (trim[0] + trim[2]) * 0.5 - 20.0,
            (trim[1] + trim[3]) * 0.5 - 5.0,
        ];
        t.size = [40.0, 10.0];
        let li = doc.add_text_layer(
            "text",
            crate::text::TextSet {
                texts: vec![t.clone()],
            },
        );
        assert!(run_page(&s, &m, 0, &doc).is_empty(), "centre text is clean");

        // On the trim edge: outside.
        t.pos = [trim[0] - 5.0, trim[1] + 5.0];
        let ts = crate::text::TextSet { texts: vec![t] };
        doc.set_texts(li, ts);
        let f = run_page(&s, &m, 0, &doc);
        assert!(
            f.iter().any(|x| x.check == "text.outside_trim"),
            "edge text must flag: {f:?}"
        );

        // Just inside the trim but within 5 mm: margin warn.
        let mut t2 = crate::text::TextItem::new([0.0, 0.0], String::new(), 12.0, [0, 0, 0], false);
        t2.pos = [trim[0] + 2.0, (trim[1] + trim[3]) * 0.5];
        t2.size = [30.0, 10.0];
        let ts = crate::text::TextSet { texts: vec![t2] };
        doc.set_texts(li, ts);
        let f = run_page(&s, &m, 0, &doc);
        assert!(f.iter().any(|x| x.check == "text.margin"));
    }

    /// A tone layer with a hard-edged ink source: one opaque pixel per
    /// named tile, which is all the tile-granular coverage check reads.
    fn tone_layer(doc: &mut Document, name: &str, lpi: f32, tiles: &[(i32, i32)]) -> usize {
        let i = doc.add_layer(name);
        for &(x, y) in tiles {
            doc.layers[i]
                .tile_mut(TileIdx::new(x, y))
                .set_pixel(0, 0, [0, 0, 0, FIX15_ONE as u16]);
        }
        doc.layers[i].tone = Some(crate::tone::ToneParams {
            lpi,
            ..Default::default()
        });
        i
    }

    fn clash_of(f: &[PreflightFinding]) -> Option<&PreflightFinding> {
        f.iter().find(|x| x.check == "tone.clash")
    }

    #[test]
    fn tone_clash_names_both_carriers_and_their_numbers() {
        let s = good_setup();
        let m = meta(Some(s.clone()));
        let mut doc = Document::new(256, 256);
        tone_layer(&mut doc, "cloud", 55.0, &[(0, 0), (1, 0)]);
        tone_layer(&mut doc, "sky", 60.0, &[(1, 0)]);
        let f = run_page(&s, &m, 0, &doc);
        let c = clash_of(&f).unwrap_or_else(|| panic!("mismatched overlap must flag: {f:?}"));
        assert_eq!(c.level, PreflightLevel::Warn);
        assert!(
            c.message.contains("Layer \"sky\" 60 lpi/45°")
                && c.message.contains("Layer \"cloud\" 55 lpi/45°"),
            "both carriers and their numbers: {}",
            c.message
        );
    }

    #[test]
    fn tone_clash_ignores_carriers_that_only_touch() {
        let s = good_setup();
        let m = meta(Some(s.clone()));
        let mut doc = Document::new(256, 256);
        tone_layer(&mut doc, "cloud", 55.0, &[(0, 0)]);
        tone_layer(&mut doc, "sky", 60.0, &[(1, 0)]);
        let f = run_page(&s, &m, 0, &doc);
        assert!(
            clash_of(&f).is_none(),
            "adjacent tiles do not interfere: {f:?}"
        );
    }

    #[test]
    fn tone_clash_ignores_matching_screens() {
        let s = good_setup();
        let m = meta(Some(s.clone()));
        let mut doc = Document::new(256, 256);
        tone_layer(&mut doc, "cloud", 60.0, &[(0, 0)]);
        tone_layer(&mut doc, "sky", 60.0, &[(0, 0)]);
        let f = run_page(&s, &m, 0, &doc);
        assert!(
            clash_of(&f).is_none(),
            "same screen is phase 2's job: {f:?}"
        );
    }

    #[test]
    fn tone_clash_normalizes_a_balloons_cell_px() {
        let s = good_setup();
        let m = meta(Some(s.clone()));
        let mut doc = Document::new(256, 256);
        tone_layer(&mut doc, "sky", 55.0, &[(0, 0)]);
        let b = crate::balloon::Balloon {
            shape: crate::balloon::BalloonShape::Ellipse {
                center: [32.0, 32.0],
                radii: [20.0, 20.0],
            },
            tails: Vec::new(),
            // The balloon stores px; 60 lpi at this page's dpi is that cell.
            fill_tone: Some(crate::balloon::BalloonTone {
                cell_px: s.dpi as f32 / 60.0,
                ..Default::default()
            }),
            ..Default::default()
        };
        doc.add_balloon_layer(
            "bubbles",
            crate::balloon::BalloonSet {
                balloons: vec![b],
                border_px: 2.0,
                pressure_width: false,
            },
        );
        let f = run_page(&s, &m, 0, &doc);
        let c = clash_of(&f).unwrap_or_else(|| panic!("balloon over tone must flag: {f:?}"));
        assert!(
            c.message.contains("Balloon 1 on \"bubbles\" 60 lpi/45°")
                && c.message.contains("Layer \"sky\" 55 lpi/45°"),
            "cell px must read back as lpi: {}",
            c.message
        );
    }

    #[test]
    fn tone_grey_edge_fires_on_a_soft_source_only() {
        let s = good_setup();
        let m = meta(Some(s.clone()));
        let mut doc = Document::new(256, 256);
        let i = tone_layer(&mut doc, "sky", 60.0, &[(0, 0)]);
        // An anti-aliased outline: a run of half-covered grey pixels.
        for x in 0..20 {
            doc.layers[i]
                .tile_mut(TileIdx::new(0, 0))
                .set_pixel(x, 3, [8000, 8000, 8000, 16000]);
        }
        let f = run_page(&s, &m, 0, &doc);
        assert!(
            f.iter().any(|x| x.check == "tone.grey_edge"),
            "an AA'd tone source must flag: {f:?}"
        );

        // Hardened to pure ink: nothing to break up.
        for x in 0..20 {
            doc.layers[i]
                .tile_mut(TileIdx::new(0, 0))
                .set_pixel(x, 3, [0, 0, 0, FIX15_ONE as u16]);
        }
        let f = run_page(&s, &m, 0, &doc);
        assert!(
            !f.iter().any(|x| x.check == "tone.grey_edge"),
            "a hard-edged source must not flag: {f:?}"
        );
    }

    #[test]
    fn colour_on_mono_warns_grey_does_not() {
        let s = good_setup();
        let mut m = meta(Some(s.clone()));
        m.expression = Expression::Mono;
        let mut doc = Document::new(256, 256);
        let li = doc.add_layer("art");
        // A reddish pixel (premultiplied fix15 with alpha).
        doc.layers[li]
            .tile_mut(crate::tile::TileIdx::new(0, 0))
            .set_pixel(5, 5, [20000, 491, 491, 32767]);
        let f = run_page(&s, &m, 0, &doc);
        assert!(f.iter().any(|x| x.check == "expression.colour_on_mono"));

        // Grey (equal channels) with the same alpha: clean.
        doc.layers[li]
            .tile_mut(crate::tile::TileIdx::new(0, 0))
            .set_pixel(5, 5, [20000, 20000, 20000, 32767]);
        let f = run_page(&s, &m, 0, &doc);
        assert!(
            !f.iter().any(|x| x.check == "expression.colour_on_mono"),
            "grey must not flag: {f:?}"
        );

        // And colour is fine when the work IS colour.
        doc.layers[li]
            .tile_mut(crate::tile::TileIdx::new(0, 0))
            .set_pixel(5, 5, [20000, 491, 491, 32767]);
        m.expression = Expression::Colour;
        let f = run_page(&s, &m, 0, &doc);
        assert!(f.iter().all(|x| x.check != "expression.colour_on_mono"));
    }

    /// PF-02, the other half of d2b879a: a 下書き layer is not on the
    /// printed page, so NO page check may accuse it. Lettering roughed in
    /// over the trim is how a name is laid out before it is set properly.
    #[test]
    fn draft_text_is_never_measured_against_the_trim() {
        let s = good_setup();
        let m = meta(Some(s.clone()));
        let mut doc = Document::new(s.paper_px().0, s.paper_px().1);
        let trim = s.trim_rect_px();
        let mut t = crate::text::TextItem::new([0.0, 0.0], String::new(), 12.0, [0, 0, 0], false);
        t.pos = [trim[0] - 5.0, trim[1] + 5.0];
        t.size = [40.0, 10.0];
        let li = doc.add_text_layer("rough letters", crate::text::TextSet { texts: vec![t] });
        // Not a draft: it hangs out of the trim and preflight says so.
        assert!(
            run_page(&s, &m, 0, &doc)
                .iter()
                .any(|x| x.check == "text.outside_trim"),
            "the control case must flag, or this test proves nothing"
        );
        doc.set_layer_draft(li, true);
        let f = run_page(&s, &m, 0, &doc);
        assert!(
            f.iter().all(|x| !x.check.starts_with("text.")),
            "a draft never prints, so it cannot leave the trim: {f:?}"
        );
    }

    /// The cascade the export composite already honours
    /// ([`Document::effective_drafts`]): a folder marked draft drafts
    /// everything inside it, and the whole ネーム usually lives in one.
    #[test]
    fn a_draft_folder_drafts_the_page_content_inside_it() {
        let s = good_setup();
        let m = meta(Some(s.clone()));
        let mut doc = Document::new(s.paper_px().0, s.paper_px().1);
        let trim = s.trim_rect_px();
        // A blue rough, and lettering laid out over the trim beside it.
        let blue = doc.add_layer("blue pencil");
        doc.layers[blue]
            .tile_mut(TileIdx::new(0, 0))
            .set_pixel(5, 5, [491, 491, 20000, 32767]);
        let mut t = crate::text::TextItem::new([0.0, 0.0], String::new(), 12.0, [0, 0, 0], false);
        t.pos = [trim[0] - 5.0, trim[1] + 5.0];
        t.size = [40.0, 10.0];
        let li = doc.add_text_layer("rough letters", crate::text::TextSet { texts: vec![t] });
        // [base, children (depth 1)…, folder header] — a folder owns the run
        // of deeper layers directly BELOW it.
        doc.layers[blue].depth = 1;
        doc.layers[li].depth = 1;
        let mut folder = crate::doc::Layer::new("rough");
        folder.folder = true;
        doc.layers.push(folder);
        let fi = doc.layers.len() - 1;
        let ids = |f: &[PreflightFinding]| -> Vec<&'static str> {
            f.iter().map(|x| x.check).collect()
        };
        let before = run_page(&s, &m, 0, &doc);
        assert!(
            ids(&before).contains(&"text.outside_trim")
                && ids(&before).contains(&"expression.colour_on_mono"),
            "the control case must flag both: {:?}",
            ids(&before)
        );
        doc.set_layer_draft(fi, true);
        let f = run_page(&s, &m, 0, &doc);
        assert!(
            f.is_empty(),
            "a draft FOLDER drafts its children, same as export: {f:?}"
        );
    }

    /// A screen on a draft layer prints nothing, so it cannot ring against
    /// anything and its soft edges cannot degrade.
    #[test]
    fn a_draft_tone_layer_is_no_screen_at_all() {
        let s = good_setup();
        let m = meta(Some(s.clone()));
        let mut doc = Document::new(256, 256);
        tone_layer(&mut doc, "cloud", 55.0, &[(0, 0), (1, 0)]);
        let sky = tone_layer(&mut doc, "sky", 60.0, &[(1, 0)]);
        // Give the draft-to-be soft edges as well, so one flip covers both
        // tone checks.
        for x in 0..20 {
            doc.layers[sky]
                .tile_mut(TileIdx::new(1, 0))
                .set_pixel(x, 3, [8000, 8000, 8000, 16000]);
        }
        let before = run_page(&s, &m, 0, &doc);
        assert!(
            clash_of(&before).is_some() && before.iter().any(|x| x.check == "tone.grey_edge"),
            "the control case must flag both: {before:?}"
        );
        doc.set_layer_draft(sky, true);
        let f = run_page(&s, &m, 0, &doc);
        assert!(
            clash_of(&f).is_none(),
            "a draft screen never meets the other one in print: {f:?}"
        );
        assert!(
            !f.iter()
                .any(|x| x.check == "tone.grey_edge" && x.message.contains("sky")),
            "and its grey edges are nobody's problem: {f:?}"
        );
    }

    /// The other two tone carriers: a live `FillKind::Tone` layer and a
    /// toned balloon. Both are set from the same ungated row menu.
    #[test]
    fn a_draft_fill_tone_layer_and_a_draft_balloon_carry_no_screen() {
        let s = good_setup();
        let m = meta(Some(s.clone()));
        let build = |draft: bool| {
            let mut doc = Document::new(256, 256);
            tone_layer(&mut doc, "sky", 55.0, &[(0, 0)]);
            let fi = doc.add_layer("flat tone");
            doc.layers[fi].kind = crate::doc::LayerKind::Fill(FillKind::Tone {
                tone: crate::tone::ToneParams {
                    lpi: 65.0,
                    ..Default::default()
                },
                density: 0.5,
            });
            let bi = doc.add_balloon_layer(
                "bubbles",
                crate::balloon::BalloonSet {
                    balloons: vec![crate::balloon::Balloon {
                        shape: crate::balloon::BalloonShape::Ellipse {
                            center: [32.0, 32.0],
                            radii: [20.0, 20.0],
                        },
                        tails: Vec::new(),
                        fill_tone: Some(crate::balloon::BalloonTone {
                            cell_px: s.dpi as f32 / 70.0,
                            ..Default::default()
                        }),
                        ..Default::default()
                    }],
                    border_px: 2.0,
                    pressure_width: false,
                },
            );
            if draft {
                doc.set_layer_draft(fi, true);
                doc.set_layer_draft(bi, true);
            }
            doc
        };
        let before = run_page(&s, &m, 0, &build(false));
        assert!(
            before.iter().filter(|x| x.check == "tone.clash").count() >= 2,
            "the control case must flag both carriers: {before:?}"
        );
        let f = run_page(&s, &m, 0, &build(true));
        assert!(
            clash_of(&f).is_none(),
            "neither draft carrier prints, so neither clashes: {f:?}"
        );
    }

    /// PF-02's other half: a hidden FOLDER takes its children off the
    /// printed page, and the export composite already reads it that way
    /// (`effective_visibility`). Preflight reads the raw per-layer flag in
    /// two places and not at all in the text check, so lettering and a
    /// screen inside a folder the artist switched off still reported.
    #[test]
    fn a_hidden_folder_takes_its_content_off_the_page() {
        let s = good_setup();
        let m = meta(Some(s.clone()));
        let mut doc = Document::new(s.paper_px().0, s.paper_px().1);
        let trim = s.trim_rect_px();
        // The partner screen, OUTSIDE the folder: it stays on the page and
        // is what the folder's screen clashes with.
        tone_layer(&mut doc, "cloud", 55.0, &[(0, 0), (1, 0)]);
        // Two children: a clashing screen and lettering over the trim.
        let sky = tone_layer(&mut doc, "sky", 60.0, &[(1, 0)]);
        let mut t = crate::text::TextItem::new([0.0, 0.0], String::new(), 12.0, [0, 0, 0], false);
        t.pos = [trim[0] - 5.0, trim[1] + 5.0];
        t.size = [40.0, 10.0];
        let li = doc.add_text_layer("rough letters", crate::text::TextSet { texts: vec![t] });
        // [base, cloud, children (depth 1)…, folder header].
        doc.layers[sky].depth = 1;
        doc.layers[li].depth = 1;
        let mut folder = crate::doc::Layer::new("inserts");
        folder.folder = true;
        doc.layers.push(folder);
        let fi = doc.layers.len() - 1;

        let before = run_page(&s, &m, 0, &doc);
        let ids: Vec<_> = before.iter().map(|x| x.check).collect();
        assert!(
            ids.contains(&"text.outside_trim") && ids.contains(&"tone.clash"),
            "the control case must flag both: {ids:?}"
        );
        // Hide the FOLDER, not the layers: the children keep `visible`.
        doc.set_layer_visible(fi, false);
        assert!(
            doc.layers[sky].visible && doc.layers[li].visible,
            "the test must exercise the cascade, not a hidden layer"
        );
        let f = run_page(&s, &m, 0, &doc);
        assert!(
            f.is_empty(),
            "nothing inside a hidden folder reaches the printer: {f:?}"
        );
    }
}
