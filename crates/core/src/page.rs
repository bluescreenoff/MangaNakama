//! Manga page geometry — the full CSP comic-settings model: paper, trim
//! (finish) with bleed, inner border (default frame) with binding offset, and
//! publisher safety margins. All in mm at a dpi; pixel presets use `dpi == 0`.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageSetup {
    pub name: String,
    pub dpi: u32,
    /// The canvas (paper) size.
    pub paper_mm: (f32, f32),
    /// Trim / finish size — where the page is cut.
    pub trim_mm: (f32, f32),
    /// Default/inner border — the reference frame panels are drawn against.
    pub inner_mm: (f32, f32),
    /// Inner-border offset from centred, mm (CSP X/Y offset; X is applied
    /// towards the outside edge, mirrored on even pages when bound).
    pub inner_offset_mm: (f32, f32),
    /// Bleed width around the trim, mm.
    pub bleed_mm: f32,
    /// Publisher safety margins [top, bottom, inside, outside] from the trim,
    /// mm. `None` = the preset defines none.
    pub safety_mm: Option<[f32; 4]>,
}

impl PageSetup {
    fn mm_to_px(&self, mm: f32) -> f32 {
        mm / 25.4 * self.dpi as f32
    }

    pub fn paper_px(&self) -> (u32, u32) {
        if self.dpi == 0 {
            return (
                self.paper_mm.0.max(1.0) as u32,
                self.paper_mm.1.max(1.0) as u32,
            );
        }
        (
            self.mm_to_px(self.paper_mm.0).round().max(1.0) as u32,
            self.mm_to_px(self.paper_mm.1).round().max(1.0) as u32,
        )
    }

    /// Pixel presets carry no mm geometry, so no guides.
    pub fn has_guides(&self) -> bool {
        self.dpi != 0
    }

    /// A rectangle of `mm` size centred on the paper, offset by `off_mm`,
    /// canvas px: [x0, y0, x1, y1].
    pub fn rect_px(&self, mm: (f32, f32), off_mm: (f32, f32)) -> [f32; 4] {
        let (pw, ph) = self.paper_px();
        let (w, h) = (self.mm_to_px(mm.0), self.mm_to_px(mm.1));
        let (dx, dy) = (self.mm_to_px(off_mm.0), self.mm_to_px(off_mm.1));
        let x0 = (pw as f32 - w) * 0.5 + dx;
        let y0 = (ph as f32 - h) * 0.5 + dy;
        [x0, y0, x0 + w, y0 + h]
    }

    pub fn trim_rect_px(&self) -> [f32; 4] {
        self.rect_px(self.trim_mm, (0.0, 0.0))
    }

    pub fn bleed_rect_px(&self) -> [f32; 4] {
        let b = self.bleed_mm * 2.0;
        self.rect_px((self.trim_mm.0 + b, self.trim_mm.1 + b), (0.0, 0.0))
    }

    pub fn inner_rect_px(&self) -> [f32; 4] {
        self.rect_px(self.inner_mm, self.inner_offset_mm)
    }

    /// Safety-margin rectangle (inside the trim), if the preset defines one.
    /// [top, bottom, inside, outside] with "inside" placed on the left —
    /// callers mirror per page for right-bound books.
    pub fn safety_rect_px(&self) -> Option<[f32; 4]> {
        let [t, b, inside, outside] = self.safety_mm?;
        let [x0, y0, x1, y1] = self.trim_rect_px();
        Some([
            x0 + self.mm_to_px(inside),
            y0 + self.mm_to_px(t),
            x1 - self.mm_to_px(outside),
            y1 - self.mm_to_px(b),
        ])
    }

    /// The standard starting points. The Shueisha entry is copied verbatim
    /// from CSP's own new-comic preset (owner screenshot, 2026-08-13).
    pub fn presets() -> Vec<PageSetup> {
        let plain =
            |name: &str, dpi: u32, paper: (f32, f32), trim: (f32, f32), inner: (f32, f32)| {
                PageSetup {
                    name: name.to_owned(),
                    dpi,
                    paper_mm: paper,
                    trim_mm: trim,
                    inner_mm: inner,
                    inner_offset_mm: (0.0, 0.0),
                    bleed_mm: 3.0,
                    safety_mm: None,
                }
            };
        vec![
            PageSetup {
                name: "Shueisha manga A (Shonen Jump, Margaret)".to_owned(),
                dpi: 600,
                paper_mm: (257.0, 364.0),
                trim_mm: (211.2, 323.4),
                inner_mm: (180.0, 270.0),
                inner_offset_mm: (3.6, 2.7),
                bleed_mm: 6.0,
                safety_mm: Some([13.8, 9.6, 5.5, 11.8]),
            },
            plain(
                "Manuscript B4 600dpi (投稿用)",
                600,
                (257.0, 364.0),
                (220.0, 310.0),
                (180.0, 270.0),
            ),
            plain(
                "Doujinshi B5 600dpi (同人誌)",
                600,
                (210.0, 297.0),
                (182.0, 257.0),
                (150.0, 220.0),
            ),
            plain(
                "B5 Monochrome 600dpi",
                600,
                (188.0, 263.0),
                (182.0, 257.0),
                (150.0, 220.0),
            ),
            plain(
                "A4 Color 350dpi",
                350,
                (216.0, 303.0),
                (210.0, 297.0),
                (190.0, 277.0),
            ),
            PageSetup {
                name: "Square 2048 px".to_owned(),
                dpi: 0,
                paper_mm: (2048.0, 2048.0),
                trim_mm: (0.0, 0.0),
                inner_mm: (0.0, 0.0),
                inner_offset_mm: (0.0, 0.0),
                bleed_mm: 0.0,
                safety_mm: None,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shueisha_a_is_the_screenshot_numbers() {
        let p = &PageSetup::presets()[0];
        assert_eq!(p.paper_px(), (6071, 8598)); // 257x364mm @600
        let trim = p.trim_rect_px();
        // 211.2mm @600 = 4989.9px wide, centred on 6071.
        assert!((trim[2] - trim[0] - 4989.9).abs() < 1.0);
        assert!(p.safety_rect_px().is_some());
        let inner = p.inner_rect_px();
        let centred = p.rect_px(p.inner_mm, (0.0, 0.0));
        assert!(inner[0] > centred[0], "x offset shifts the frame");
    }

    #[test]
    fn pixel_preset_has_no_guides() {
        let p = PageSetup::presets().pop().unwrap();
        assert_eq!(p.paper_px(), (2048, 2048));
        assert!(!p.has_guides());
    }
}

// --- spreads (TRIAGE 143, PM-030..033) ------------------------------------

use crate::doc::{Document, Layer, LayerKind};
#[cfg(test)]
use crate::tile::TileIdx;

/// Copy a layer into a canvas-sized frame, offset by `dx` px — EVERY plane
/// (ink, mask, derived tone/fill/edge rasters) through
/// `Layer::remap_planes_x`, which is where the "derived planes must travel
/// with the ink" rule lives.
fn shifted(l: &Layer, dx: i64, w: u32, h: u32) -> Layer {
    let mut out = l.clone();
    out.remap_planes_x((i64::MIN / 4, i64::MAX / 4), dx, w, h);
    out
}

/// Copy the horizontal slice `[x0, x0+cw)` of a layer (canvas-height),
/// re-based to x = 0 — same every-plane rule as `shifted`.
fn cropped(l: &Layer, x0: i64, cw: u32, h: u32) -> Layer {
    let mut out = l.clone();
    out.remap_planes_x((x0, x0 + cw as i64), -x0, cw, h);
    out
}

/// PM-030: two page documents side-by-side as one wide spread with a
/// `gap`-px gutter between them. Layer STATE travels whole (blend, depth,
/// folders, kinds...); B's stack lands above A's in the layer list and its
/// ink offsets by (A.width + gap, 0). The height is the max — a shorter
/// page leaves its corner transparent.
pub fn combine_spread(a: &Document, b: &Document, gap: u32) -> Document {
    let h = a.size.1.max(b.size.1);
    let mut d = Document::new(a.size.0 + gap + b.size.0, h);
    d.layers.clear();
    let w = d.size.0;
    d.layers = a
        .layers
        .iter()
        .map(|l| shifted(l, 0, w, h))
        .chain(
            b.layers
                .iter()
                .map(|l| shifted(l, a.size.0 as i64 + gap as i64, w, h)),
        )
        .collect();
    d
}

/// PM-033: split a spread back into two page documents. The gutter pixels
/// (`gap` wide, at the centre) are DISCARDED — that is PM-031's swallow;
/// art meant to survive must cross the gap-less boundary (gap 0 round
/// trips exactly). `None` for a canvas too narrow to halve.
///
/// Audit L1 (rounds 50-68), recorded limitation: the seam assumes both
/// halves came from SAME-WIDTH pages — `left_w` is half the canvas no
/// matter what `combine_spread` was given (and the combine takes
/// `max(a.h, b.h)`). Manga spreads are uniform in practice; splitting a
/// spread built from differently-sized pages lands the seam in the wrong
/// place. If odd spreads are ever needed, record each page's width at
/// combine time and split by that — don't guess here.
pub fn split_spread(d: &Document, gap: u32) -> Option<(Document, Document)> {
    let (w, h) = (d.size.0, d.size.1);
    if w <= gap || w - gap < 2 {
        return None;
    }
    let left_w = (w - gap) / 2;
    let right_w = w - gap - left_w;
    let cut = left_w as i64 + gap as i64;
    let mut left = Document::new(left_w, h);
    left.layers = d.layers.iter().map(|l| cropped(l, 0, left_w, h)).collect();
    let mut right = Document::new(right_w, h);
    right.layers = d
        .layers
        .iter()
        .map(|l| cropped(l, cut, right_w, h))
        .collect();
    Some((left, right))
}

/// PM-032: drop raster layers that lost all their tiles (combine/split
/// housekeeping). Folder headers and vector layers (Frame/Balloon/Text
/// carry their geometry, not tiles) always stay.
pub fn drop_empty_raster_layers(d: &mut Document) -> usize {
    let before = d.layers.len();
    d.layers
        .retain(|l| l.folder || !matches!(l.kind, LayerKind::Raster) || l.tiles().count() > 0);
    before - d.layers.len()
}

#[cfg(test)]
mod spread_tests {
    use super::*;
    use crate::doc::Document;

    fn ink(doc: &mut Document, x: i32, y: i32) {
        let idx = TileIdx::of_pixel(x, y);
        doc.active_layer_mut().tile_mut(idx).set_pixel(
            (x - idx.origin().0) as usize,
            (y - idx.origin().1) as usize,
            [1, 2, 3, 4],
        );
    }

    fn has_ink(doc: &Document, x: i32, y: i32) -> bool {
        let idx = TileIdx::of_pixel(x, y);
        let (lx, ly) = ((x - idx.origin().0) as usize, (y - idx.origin().1) as usize);
        doc.layers
            .iter()
            .any(|l| l.tile(idx).is_some_and(|t| t.pixel(lx, ly)[3] > 0))
    }

    /// Combine offsets B by A.width + gap; split is its exact inverse at
    /// gap 0, and the gutter swallows at gap > 0.
    #[test]
    fn combine_split_round_trip_and_gutter() {
        let mut a = Document::new(128, 64);
        ink(&mut a, 10, 20);
        let mut b = Document::new(128, 64);
        ink(&mut b, 100, 30);
        // Gutter art: in A at x=127 (inside A, survives) and in B at x=0
        // (lands in the gutter when gap > 0).
        ink(&mut a, 127, 5);
        ink(&mut b, 0, 6);

        // gap 0: exact inverse.
        let s0 = combine_spread(&a, &b, 0);
        assert_eq!(s0.size, (256, 64));
        assert!(has_ink(&s0, 10, 20) && has_ink(&s0, 228, 30));
        let (l, r) = split_spread(&s0, 0).unwrap();
        assert!(has_ink(&l, 10, 20) && !has_ink(&l, 228, 30));
        assert!(has_ink(&r, 100, 30) && !has_ink(&r, 10, 20));

        // gap 16: B shifts by 144; the halves' edges survive the round
        // trip and the gutter [128, 144) is discarded on split.
        let mut s1 = combine_spread(&a, &b, 16);
        assert_eq!(s1.size, (272, 64));
        assert!(has_ink(&s1, 10, 20) && has_ink(&s1, 244, 30));
        assert!(has_ink(&s1, 127, 5), "A-side edge ink survives the combine");
        assert!(has_ink(&s1, 144, 6), "B-side edge ink lands at the cut");
        // Draw INTO the gutter — PM-031's swallow case.
        ink(&mut s1, 135, 40);
        let (l1, r1) = split_spread(&s1, 16).unwrap();
        assert_eq!(l1.size, (128, 64));
        assert!(has_ink(&l1, 10, 20) && has_ink(&l1, 127, 5));
        assert!(has_ink(&r1, 100, 30), "B-side ink re-bases to its page");
        assert!(
            has_ink(&r1, 0, 6),
            "B's edge pixel is the right half's first"
        );
        // The gutter pixel belongs to NEITHER half: 135 ≥ 128 (left's cw)
        // and < 144 (right's x0). It is simply gone.
        assert!(!has_ink(&l1, 135, 40), "left keeps no gutter ink");
        assert!(!has_ink(&r1, 135 - 144, 40), "right keeps no gutter ink");
    }

    /// The r69–r115 audit's print-corruption finding, pinned: DERIVED
    /// rasters (tone dots here) travel with the crop. The page export
    /// derives BEFORE splitting (the dot phase is canvas-continuous) and
    /// does not re-derive after — so each half must carry its own slice of
    /// the derived map. The old crop cloned the derived map at SPREAD
    /// coordinates: the right page printed the left page's dots.
    #[test]
    fn split_carries_derived_tone_with_each_half() {
        let mut d = Document::new(256, 64);
        // Solid black ink ONLY in the right half (x 200..232).
        for x in 200..232 {
            for y in 10..40 {
                let idx = TileIdx::of_pixel(x, y);
                d.active_layer_mut().tile_mut(idx).set_pixel(
                    (x - idx.origin().0) as usize,
                    (y - idx.origin().1) as usize,
                    [0, 0, 0, 32768],
                );
            }
        }
        assert!(d.set_tone(
            0,
            Some(crate::tone::ToneParams {
                lpi: 20.0,
                ..Default::default()
            })
        ));
        d.refresh_derived(600);
        let (l, r) = split_spread(&d, 0).unwrap();

        let ink_count = |doc: &Document| -> usize {
            let img =
                crate::export::composite_for_export(doc, crate::export::Background::White);
            img.pixels().filter(|p| p.0[0] < 128).count()
        };
        assert_eq!(
            ink_count(&l),
            0,
            "the left half was blank on the spread — no dots may appear"
        );
        let right_ink = ink_count(&r);
        assert!(
            right_ink > 0,
            "the right half's ink block must keep its tone dots"
        );
        // And the dots sit where the ink was (x re-based to 200-128=72).
        let img = crate::export::composite_for_export(&r, crate::export::Background::White);
        let in_block = (72..104)
            .flat_map(|x| (10..40).map(move |y| (x, y)))
            .filter(|&(x, y)| img.get_pixel(x, y).0[0] < 128)
            .count();
        assert_eq!(
            in_block, right_ink,
            "every dark pixel lies inside the re-based ink block"
        );
    }

    /// PM-032: split empties a spanning layer's halves; only the empty
    /// RASTER layers go.
    #[test]
    fn split_drops_only_empty_raster_layers() {
        let mut a = Document::new(64, 64);
        ink(&mut a, 10, 10);
        let mut b = Document::new(64, 64);
        ink(&mut b, 10, 10);
        let s = combine_spread(&a, &b, 0);
        let (mut l, mut r) = split_spread(&s, 0).unwrap();
        assert!(has_ink(&l, 10, 10) && has_ink(&r, 10, 10));
        // Now a layer that lives ONLY on B: splitting empties its left copy.
        assert_eq!(
            drop_empty_raster_layers(&mut l),
            1,
            "B's layer vanishes from A's half"
        );
        assert_eq!(drop_empty_raster_layers(&mut r), 1, "and A's from B's");
    }
}
