//! Correction LAYERS (row 105, CSP 色調補正レイヤー): a live layer whose
//! content is an [`Adjust`] — the corrected page is DERIVED, the pixels
//! below are never touched, and the parameters stay editable forever.
//!
//! # How it composites without a Replace blend mode
//!
//! The derived raster is the correction applied to the composite of every
//! layer BELOW this one **over the paper colour** — which makes every
//! derived pixel OPAQUE, and an opaque tile under `Blend::Normal` *is* a
//! replace. Neither compositor changes: the CPU walk and the GPU shader
//! both just see an ordinary layer whose tiles happen to cover the page.
//! Two consequences, both deliberate:
//!
//! * Layer opacity is the correction's STRENGTH for free: at 0.5 the
//!   compositor shows half corrected-page, half the page itself — exactly
//!   CSP's adjustment-layer fade.
//! * A transparent-background export of a page with a correction layer is
//!   opaque where the correction reaches. The paper is part of what a
//!   correction corrects; a page exported for print never notices.
//!
//! # The window
//!
//! The layer mask is the window, cut from the selection at creation like a
//! live fill (`fill_layer::mask_from_selection`; no mask = the whole
//! canvas). Coverage is applied at DERIVATION through [`correct_tile`]'s
//! own blend — a masked-out pixel derives as the below-composite verbatim
//! — and the compositor then applies LM-005 mask scaling on top like any
//! layer. A soft window therefore feathers twice (coverage²), the same
//! convention live fills already have. Tiles the mask does not reach are
//! not derived at all: the compositor draws the real layers there.
//!
//! # Derivation source
//!
//! The below-composite walks the REAL compositor (`export::composite_size`
//! via a truncated document clone — Arc-shared tiles, no pixel copies), so
//! folders, clips, blend modes and live layers below all read exactly as
//! they render. `CompOpts::Export`: drafts below a correction do not leak
//! into it — the derived tiles are what PRINTS, and since they cover the
//! page they are also what the screen shows. (A 下書き under a correction
//! layer is therefore invisible on screen too; that is the honest half of
//! never printing it by accident.)
//!
//! The composite is read back at 8-bit — `composite_size`'s output — before
//! the correction runs. That is the displayed page, and it is what CSP
//! corrects too; the fix15 headroom is spent where it matters, in
//! `correct_tile`'s own arithmetic.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::adjust::{Adjust, correct_tile};
use crate::doc::{Document, Layer, LayerKind};
use crate::fill_layer::mask_from_selection;
use crate::tile::{TILE_PIXELS, TILE_SIZE, Tile, TileIdx};

/// The derived state riding a correction layer. Never serialized — ORA
/// stores only the `mnc-correction` params and the mask; everything here
/// rebuilds on load.
#[derive(Clone, Debug, Default)]
pub struct CorrDerived {
    /// The corrected page, tile by tile. What both compositors display.
    pub(crate) tiles: HashMap<TileIdx, Arc<Tile>>,
    /// (params, window-mask revision, dpi, canvas size, below props key)
    /// — a mismatch on any of these rebuilds EVERY tile.
    stamp: Option<(Adjust, Option<u64>, u32, (u32, u32), u64)>,
    /// Per-tile (max revision, entry count) of the below layers' tile maps
    /// — a mismatch rebuilds THAT tile. This is what keeps a brush stroke
    /// under a correction layer from re-compositing the whole page.
    tile_keys: HashMap<TileIdx, (u64, u32)>,
}

/// Everything about the layers below `i` that changes how they composite
/// WITHOUT moving a tile revision: eyes, opacity, blend, tints, structure.
/// Tile contents are deliberately absent — they have their own per-tile
/// keys. Misses here are silent stale-correction bugs; when a new
/// presentation field lands on `Layer`, it belongs in this hash.
fn below_props_key(doc: &Document, i: usize) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    i.hash(&mut h);
    // The derivation background: a paper retint must re-derive.
    if let crate::export::Background::Solid(c) = doc.paper_export_background() {
        c.hash(&mut h);
    }
    for l in &doc.layers[..i] {
        l.visible.hash(&mut h);
        l.opacity.to_bits().hash(&mut h);
        l.blend.hash(&mut h);
        l.label.hash(&mut h); // never composited, but hashing it is harmless
        l.layer_colour.hash(&mut h);
        l.layer_sub_colour.hash(&mut h);
        l.depth.hash(&mut h);
        l.folder.hash(&mut h);
        l.through.hash(&mut h);
        l.clip.hash(&mut h);
        l.escape_frame.hash(&mut h);
        l.draft.hash(&mut h);
        if let Some(m) = &l.mask {
            m.enabled.hash(&mut h);
            m.revision.hash(&mut h);
        } else {
            u64::MAX.hash(&mut h);
        }
    }
    h.finish()
}

/// Per-canvas-tile (max revision, count) over every tile map the compositor
/// might read on the layers below `i`: painted tiles, the derived display
/// raster when it differs, folder coverage masks, border-effect mats.
/// Double counting a map that aliases another is fine — the key only has to
/// be deterministic and move when content moves.
fn below_tile_keys(doc: &Document, i: usize) -> HashMap<TileIdx, (u64, u32)> {
    let (tw, th) = canvas_tiles(doc.size);
    let in_canvas = |idx: &TileIdx| idx.x >= 0 && idx.y >= 0 && idx.x < tw && idx.y < th;
    let mut keys: HashMap<TileIdx, (u64, u32)> = HashMap::new();
    let mut fold = |idx: TileIdx, rev: u64| {
        let e = keys.entry(idx).or_insert((0, 0));
        e.0 = e.0.max(rev);
        e.1 += 1;
    };
    for l in &doc.layers[..i] {
        for (idx, t) in l.tiles() {
            if in_canvas(&idx) {
                fold(idx, t.revision());
            }
        }
        for (idx, t) in l.display_tiles() {
            if in_canvas(idx) {
                fold(*idx, t.revision());
            }
        }
        for m in [l.mask_tiles(), l.edge_tiles()].into_iter().flatten() {
            for (idx, t) in m {
                if in_canvas(idx) {
                    fold(*idx, t.revision());
                }
            }
        }
    }
    keys
}

fn canvas_tiles(size: (u32, u32)) -> (i32, i32) {
    let t = TILE_SIZE as u32;
    (
        (size.0.max(1).div_ceil(t)) as i32,
        (size.1.max(1).div_ceil(t)) as i32,
    )
}

impl Document {
    /// New correction layer above the active one. The window mask comes
    /// from the current selection when one exists, live-fill style; none =
    /// the whole canvas. Structural like every layer add. Returns the index.
    pub fn add_correction_layer(&mut self, adj: Adjust, from_selection: bool) -> usize {
        let i = self.add_layer_above(self.active, adj.label());
        let mask = if from_selection {
            self.selection
                .as_ref()
                .and_then(|s| mask_from_selection(self, s))
        } else {
            None
        };
        let l = &mut self.layers[i];
        l.kind = LayerKind::Correction(adj);
        l.mask = mask;
        l.corr = None; // force a full derive on the next refresh
        self.touch();
        i
    }

    /// Re-derive every correction layer that is stale. Bottom-up, one at a
    /// time, so a correction above another sees the lower one's fresh
    /// raster. Cheap when nothing moved: the props hash plus one pass over
    /// the below layers' tile maps, no pixels.
    ///
    /// Runs from `refresh_derived` AFTER tone/fill/edge refreshes — the
    /// below-composite must read what those layers actually display.
    pub fn refresh_corrections(&mut self, dpi: u32) {
        let idxs: Vec<usize> = (0..self.layers.len())
            .filter(|&i| matches!(self.layers[i].kind, LayerKind::Correction(_)))
            .collect();
        for i in idxs {
            self.refresh_correction_at(i, dpi);
        }
        // A layer that stopped being a correction sheds its derived state.
        for l in &mut self.layers {
            if !matches!(l.kind, LayerKind::Correction(_)) && l.corr.is_some() {
                l.corr = None;
            }
        }
    }

    fn refresh_correction_at(&mut self, i: usize, dpi: u32) {
        let LayerKind::Correction(adj) = self.layers[i].kind else {
            return;
        };
        let mask_rev = self.layers[i]
            .mask
            .as_ref()
            .filter(|m| m.enabled)
            .map(|m| m.revision);
        let props = below_props_key(self, i);
        let stamp = (adj, mask_rev, dpi, self.size, props);
        let tile_keys = below_tile_keys(self, i);
        let force = self.layers[i].corr.as_ref().and_then(|c| c.stamp) != Some(stamp);
        if !force
            && self.layers[i]
                .corr
                .as_ref()
                .is_some_and(|c| c.tile_keys == tile_keys)
        {
            return;
        }

        // The tiles this correction derives: the window's tiles when a mask
        // is on, the whole canvas otherwise. Everywhere else the compositor
        // draws the real layers and the correction has nothing to say.
        let wanted: Vec<TileIdx> = match self.layers[i].mask.as_ref().filter(|m| m.enabled) {
            Some(m) => {
                let (tw, th) = canvas_tiles(self.size);
                m.tiles
                    .keys()
                    .copied()
                    .filter(|idx| idx.x >= 0 && idx.y >= 0 && idx.x < tw && idx.y < th)
                    .collect()
            }
            None => {
                let (tw, th) = canvas_tiles(self.size);
                (0..th)
                    .flat_map(|y| (0..tw).map(move |x| TileIdx::new(x, y)))
                    .collect()
            }
        };

        // One truncated clone for the whole rebuild — Arc-shared tiles, so
        // this is pointer traffic, not pixels.
        let below = self.below_doc(i);
        let bg = self.paper_export_background();
        let mask_tiles = self.layers[i]
            .mask
            .as_ref()
            .filter(|m| m.enabled)
            .map(|m| m.tiles.clone());

        let mut old = self.layers[i].corr.take().unwrap_or_default();
        let mut out: HashMap<TileIdx, Arc<Tile>> = HashMap::new();
        for idx in wanted {
            let key = tile_keys.get(&idx).copied().unwrap_or((0, 0));
            // Both maps are sparse — a tile with no below content is absent
            // from both, and absent must compare EQUAL to absent or every
            // empty tile re-derives on every pass.
            let old_key = old.tile_keys.get(&idx).copied().unwrap_or((0, 0));
            if !force
                && old_key == key
                && let Some(t) = old.tiles.remove(&idx)
            {
                out.insert(idx, t);
                continue;
            }
            let (ox, oy) = idx.origin();
            let img = crate::export::composite_rect_export(
                &below,
                bg,
                TILE_SIZE as u32,
                TILE_SIZE as u32,
                ox,
                oy,
            );
            // The below-composite as an opaque fix15 tile. Off-canvas pixels
            // stay transparent (the image is zero there) and `correct_tile`
            // passes them through; the compositor clips them anyway.
            let mut src = vec![0u16; TILE_PIXELS * 4];
            for p in 0..TILE_PIXELS {
                let (x, y) = (p % TILE_SIZE, p / TILE_SIZE);
                let px = img.get_pixel(x as u32, y as u32).0;
                if px[3] == 0 {
                    continue;
                }
                let o = p * 4;
                for c in 0..3 {
                    src[o + c] = ((px[c] as u32 * 32768 + 127) / 255) as u16;
                }
                src[o + 3] = 32768;
            }
            let cov: Option<Box<[u8; TILE_PIXELS]>> = mask_tiles.as_ref().map(|mt| {
                let mut cov = Box::new([0u8; TILE_PIXELS]);
                if let Some(m) = mt.get(&idx) {
                    let d = m.data();
                    for (p, c) in cov.iter_mut().enumerate() {
                        *c = ((d[p * 4 + 3] as u32 * 255 + 16384) / 32768).min(255) as u8;
                    }
                }
                cov
            });
            let mut tile = Tile::new_transparent();
            correct_tile(tile.data_mut(), &src, &adj, cov.as_deref());
            out.insert(idx, Arc::new(tile));
        }
        self.layers[i].corr = Some(CorrDerived {
            tiles: out,
            stamp: Some(stamp),
            tile_keys,
        });
    }

    /// The layers below `i`, as their own document — the derivation source.
    /// Everything the composite walk reads is carried; undo history and
    /// selection are not (and must not be — the composite never looks).
    fn below_doc(&self, i: usize) -> Document {
        let mut d = Document::new(self.size.0.max(1), self.size.1.max(1));
        d.layers = self.layers[..i].to_vec();
        d.paper = self.paper.clone();
        d
    }
}

impl Layer {
    /// The derived corrected-page raster, if this is a correction layer
    /// whose derive has run. Display routing (`display_tile`) reads this.
    pub(crate) fn corr_tiles(&self) -> Option<&HashMap<TileIdx, Arc<Tile>>> {
        self.corr.as_ref().map(|c| &c.tiles)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::{FIX15_ONE_F, f32_to_fix15};
    use crate::selection::Selection;

    /// Straight-colour pixel write through the undo-recording door (bumps
    /// the tile revision like a real stroke).
    fn put(doc: &mut Document, li: usize, x: i32, y: i32, rgba: [f32; 4]) {
        let idx = TileIdx::of_pixel(x, y);
        let (ox, oy) = idx.origin();
        let p = ((y - oy) as usize * TILE_SIZE + (x - ox) as usize) * 4;
        let a = rgba[3];
        let d = doc.layers[li].tile_mut(idx).data_mut();
        for c in 0..3 {
            d[p + c] = f32_to_fix15(rgba[c] * a);
        }
        d[p + 3] = f32_to_fix15(a);
    }

    fn px(doc: &Document, x: i32, y: i32) -> [u8; 3] {
        crate::export::composite_pixel(doc, x, y).unwrap()
    }

    fn refresh(doc: &mut Document) {
        doc.refresh_derived(600);
    }

    #[test]
    fn a_correction_layer_corrects_the_page_below_including_paper() {
        let mut doc = Document::new(128, 128);
        put(&mut doc, 0, 10, 10, [0.6, 0.6, 0.6, 1.0]);
        let ci = doc.add_correction_layer(Adjust::Invert, false);
        refresh(&mut doc);
        let ink = px(&doc, 10, 10);
        assert!(
            (ink[0] as i32 - 102).abs() <= 2,
            "0.6 grey inverts to ~0.4: {ink:?}"
        );
        let paper = px(&doc, 100, 100);
        assert!(
            paper[0] <= 2 && paper[1] <= 2 && paper[2] <= 2,
            "white paper inverts to black — the correction corrects the PAGE: {paper:?}"
        );
        // The layer below was never touched: its own pixel still holds 0.6.
        let d = doc.layers[0].tile_arc(TileIdx::of_pixel(10, 10)).unwrap();
        let o = (10 * TILE_SIZE + 10) * 4;
        let v = d.data()[o] as f32 / FIX15_ONE_F;
        assert!((v - 0.6).abs() < 0.01, "source pixels untouched: {v}");
        // And the derived tiles are opaque — the no-Replace-blend contract.
        let t = doc.layers[ci].corr_tiles().unwrap().values().next().unwrap();
        assert_eq!(t.data()[3], 32768, "derived tiles are opaque");
    }

    #[test]
    fn layers_above_the_correction_are_not_corrected() {
        let mut doc = Document::new(128, 128);
        put(&mut doc, 0, 10, 10, [0.6, 0.6, 0.6, 1.0]);
        doc.add_correction_layer(Adjust::Invert, false);
        let top = doc.add_layer("above");
        put(&mut doc, top, 20, 10, [1.0, 0.0, 0.0, 1.0]);
        refresh(&mut doc);
        let red = px(&doc, 20, 10);
        assert!(
            red[0] > 200 && red[1] < 60,
            "art above stays uncorrected: {red:?}"
        );
        let ground = px(&doc, 100, 100);
        assert!(ground[0] <= 2, "the corrected ground is under it: {ground:?}");
    }

    #[test]
    fn a_selection_cuts_the_window() {
        let mut doc = Document::new(128, 128);
        put(&mut doc, 0, 10, 10, [0.6, 0.6, 0.6, 1.0]);
        put(&mut doc, 0, 100, 100, [0.6, 0.6, 0.6, 1.0]);
        doc.selection = Some(Selection::from_rect(&doc, 0.0, 0.0, 64.0, 64.0));
        doc.add_correction_layer(Adjust::Invert, true);
        doc.selection = None;
        refresh(&mut doc);
        let inside = px(&doc, 10, 10);
        assert!((inside[0] as i32 - 102).abs() <= 2, "windowed in: {inside:?}");
        let outside = px(&doc, 100, 100);
        assert!(
            (outside[0] as i32 - 153).abs() <= 2,
            "outside the window the page is its old self: {outside:?}"
        );
    }

    #[test]
    fn params_stay_editable_after_creation() {
        let mut doc = Document::new(128, 128);
        put(&mut doc, 0, 10, 10, [0.6, 0.6, 0.6, 1.0]);
        let ci = doc.add_correction_layer(Adjust::Invert, false);
        refresh(&mut doc);
        assert!(px(&doc, 100, 100)[0] <= 2);
        doc.layers[ci].kind = LayerKind::Correction(Adjust::Binarize { threshold: 0.5 });
        refresh(&mut doc);
        assert_eq!(px(&doc, 100, 100), [255, 255, 255], "paper binarizes white");
        let ink = px(&doc, 10, 10);
        assert_eq!(ink, [255, 255, 255], "0.6 grey is over the threshold");
        doc.layers[ci].kind = LayerKind::Correction(Adjust::Binarize { threshold: 0.7 });
        refresh(&mut doc);
        assert_eq!(px(&doc, 10, 10), [0, 0, 0], "…and under the raised one");
    }

    #[test]
    fn refresh_is_incremental_a_stroke_rebuilds_only_its_tile() {
        let mut doc = Document::new(128, 128);
        put(&mut doc, 0, 10, 10, [0.6, 0.6, 0.6, 1.0]);
        let ci = doc.add_correction_layer(Adjust::Invert, false);
        refresh(&mut doc);
        let far = TileIdx::new(1, 1);
        let near = TileIdx::new(0, 0);
        let keep = Arc::clone(doc.layers[ci].corr_tiles().unwrap().get(&far).unwrap());
        let stale = Arc::clone(doc.layers[ci].corr_tiles().unwrap().get(&near).unwrap());
        // A no-op refresh rebuilds nothing at all.
        refresh(&mut doc);
        assert!(Arc::ptr_eq(
            &keep,
            doc.layers[ci].corr_tiles().unwrap().get(&far).unwrap()
        ));
        assert!(Arc::ptr_eq(
            &stale,
            doc.layers[ci].corr_tiles().unwrap().get(&near).unwrap()
        ));
        // A stroke in tile (0,0) rebuilds that tile and leaves (1,1) alone.
        put(&mut doc, 0, 12, 12, [0.2, 0.2, 0.2, 1.0]);
        refresh(&mut doc);
        assert!(
            Arc::ptr_eq(&keep, doc.layers[ci].corr_tiles().unwrap().get(&far).unwrap()),
            "the untouched tile kept its allocation"
        );
        assert!(
            !Arc::ptr_eq(&stale, doc.layers[ci].corr_tiles().unwrap().get(&near).unwrap()),
            "the stroked tile re-derived"
        );
        let ink = px(&doc, 12, 12);
        assert!((ink[0] as i32 - 204).abs() <= 2, "and shows the new stroke inverted: {ink:?}");
    }

    #[test]
    fn a_below_eye_toggle_rederives() {
        let mut doc = Document::new(128, 128);
        put(&mut doc, 0, 10, 10, [0.6, 0.6, 0.6, 1.0]);
        doc.add_correction_layer(Adjust::Invert, false);
        refresh(&mut doc);
        assert!((px(&doc, 10, 10)[0] as i32 - 102).abs() <= 2);
        doc.layers[0].visible = false;
        refresh(&mut doc);
        assert!(
            px(&doc, 10, 10)[0] <= 2,
            "hidden ink leaves inverted paper — the props key caught the eye"
        );
    }

    /// A correction layer round-trips through ORA: params as
    /// `mnc-correction`, window as the persisted mask, and the reload
    /// re-derives the same composite — "change your mind at page 15"
    /// must survive a save.
    #[test]
    fn a_correction_layer_round_trips_through_ora() {
        let mut doc = Document::new(128, 128);
        put(&mut doc, 0, 10, 10, [0.6, 0.6, 0.6, 1.0]);
        doc.selection = Some(Selection::from_rect(&doc, 0.0, 0.0, 64.0, 64.0));
        doc.add_correction_layer(Adjust::Binarize { threshold: 0.7 }, true);
        doc.selection = None;
        refresh(&mut doc);
        let before = crate::export::composite(&doc, crate::export::Background::White);

        let mut buf = std::io::Cursor::new(Vec::new());
        crate::ora::save_to(&doc, &mut buf).unwrap();
        {
            let mut z = zip::ZipArchive::new(std::io::Cursor::new(buf.get_ref().clone())).unwrap();
            let mut s = String::new();
            std::io::Read::read_to_string(&mut z.by_name("stack.xml").unwrap(), &mut s).unwrap();
            assert!(s.contains("mnc-correction="), "params saved: {s}");
            assert!(s.contains("mnc-mask="), "window saved");
        }
        let mut back = crate::ora::load_from(std::io::Cursor::new(buf.into_inner())).unwrap();
        back.refresh_derived(600);
        let found = back
            .layers
            .iter()
            .find_map(|l| match l.kind {
                LayerKind::Correction(a) => Some(a),
                _ => None,
            })
            .expect("the correction layer survived as its kind");
        assert_eq!(
            found,
            Adjust::Binarize { threshold: 0.7 },
            "every parameter came back"
        );
        let after = crate::export::composite(&back, crate::export::Background::White);
        assert!(
            before.pixels().zip(after.pixels()).all(|(a, b)| a.0 == b.0),
            "and it derives the same page"
        );
    }

    #[test]
    fn a_draft_below_never_reaches_the_correction() {
        let mut doc = Document::new(128, 128);
        put(&mut doc, 0, 10, 10, [0.0, 0.0, 1.0, 1.0]);
        doc.layers[0].draft = true;
        doc.add_correction_layer(Adjust::Invert, false);
        refresh(&mut doc);
        assert!(
            px(&doc, 10, 10)[0] <= 2 && px(&doc, 10, 10)[2] <= 2,
            "the draft is not in the derived page — inverted paper only: {:?}",
            px(&doc, 10, 10)
        );
    }
}
