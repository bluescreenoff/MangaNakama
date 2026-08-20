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

use crate::doc::Document;
use crate::page::PageSetup;
use crate::project::ProjectMeta;
use crate::text::TextItem;

/// A chromatic pixel's channel spread above this (fix15 units, ~1.5% of
/// full scale) reads as colour on a mono work — quantization-safe.
const CHROMA_ULP: u16 = 491;

/// CSP's text-vs-trim rule is a fixed 5 mm, independent of the work's own
/// safety margins.
const TEXT_SAFE_MM: f32 = 5.0;

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

/// Page-level checks over the page's CONTENT: lettering vs the trim, and
/// colour on a mono work. `page_index` names the page in findings.
pub fn run_page(
    setup: &PageSetup,
    meta: &ProjectMeta,
    page_index: usize,
    doc: &Document,
) -> Vec<PreflightFinding> {
    let mut out = Vec::new();
    let trim = setup.trim_rect_px();
    let safe_px = TEXT_SAFE_MM / 25.4 * setup.dpi as f32;
    // The trim rect can sit outside the canvas or touch it; text boxes
    // compare in the same px space `TextItem.pos` uses (canvas px).
    for layer in &doc.layers {
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
        for layer in &doc.layers {
            if !layer.visible || layer.is_vector() {
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
}
