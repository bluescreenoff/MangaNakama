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
//! (Inside a sealed folder the paper is not there to derive over — see
//! "Scope" below — and the trick then holds where the group's art is
//! opaque, which is where art is.) Two consequences, both deliberate:
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
//! canvas) — or ARMED all-visible by the first brush stroke on a maskless
//! correction ([`Document::arm_full_window`] +
//! [`crate::doc::LayerMask::full`]: an empty tile map, so "the window is the
//! whole page" costs no pixels, and the stroke carves the correction back
//! off only what it touches). Coverage is applied at DERIVATION through
//! [`correct_tile`]'s
//! own blend — a masked-out pixel derives as the below-composite verbatim
//! — and the compositor then applies LM-005 mask scaling on top like any
//! layer. A soft window therefore feathers twice (coverage²), the same
//! convention live fills already have. Tiles the mask does not reach are
//! not derived at all: the compositor draws the real layers there. A FULL
//! window is the other way round — the tiles it holds are where the artist
//! carved the correction away, and the derive set is the whole canvas.
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
//!
//! # Scope: the page, or the group
//!
//! WHICH layers are "below" is [`Document::below_scope`]. A correction at
//! the top level derives from the whole page beneath it, over paper. One
//! INSIDE a sealed folder derives from that folder's own children beneath
//! it, over nothing — the group is isolated, so the page under it is not
//! visible to a child, which is the ruling Blend If already takes for the
//! same reason. A Through folder has no seal, so a correction in one still
//! sees the page. See [`Document::below_scope`] and
//! [`Document::derive_background`] for the whole rule and its one soft
//! spot (semi-transparent group content).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::adjust::{Adjust, correct_tile};
use crate::doc::{Document, Layer, LayerKind};
use crate::fill_layer::mask_from_selection;
use crate::tile::{TILE_PIXELS, TILE_SIZE, Tile, TileIdx};

/// How many tiles one derive pass hands the kernel at a time.
///
/// The batch exists so the fix15 source buffers are transient: a full-page
/// B4/600 correction is ~12 800 tiles and materialising every source at once
/// would be ~410 MB of scratch. 256 tiles is 8 MB, and it is deliberately the
/// same number as the compositor's `UPLOAD_BATCH` — one staging buffer's
/// worth on the GPU side of the seam.
const DERIVE_BATCH: usize = 256;

/// Ceiling on the below-composite source cache, in TILES.
///
/// The cache had no bound at all: `wanted` is the whole canvas for a
/// maskless correction and every entry is 16 KB, so the map is canvas-sized
/// and a big enough canvas is a big enough map. 16 384 tiles is 256 MB, and
/// the number is picked so the case the cache exists for still fits whole —
/// a maskless correction on a B4 page at 600 dpi is 6070 × 8598 px, i.e.
/// 95 × 135 = 12 825 tiles, 205 MB. Above the cap (a 600-dpi double-width
/// spread is 25 650 tiles; a resampled-up work more) the derive keeps the
/// sources it already has and stops caching new ones, so a drag degrades to
/// "the first 16 384 tiles are free" rather than growing until the machine
/// swaps.
///
/// **Keep-what-you-have, not LRU, and the scan order is why.** A derive pass
/// walks `wanted` once and touches every entry exactly once — the cyclic
/// scan LRU is famously worst at: with a working set larger than the cache,
/// each miss evicts precisely the entry the next tick asks for first and the
/// hit rate is ZERO. A fixed retained set hits `cap / wanted` of the time on
/// every tick instead, and costs no recency bookkeeping. Entries for tiles
/// that stopped being wanted are already dropped, by falling out of the
/// rebuilt map; this cap is only about the size of one pass's own set.
const SRC_CACHE_TILES: usize = 16_384;

/// The cap in force. Production always answers [`SRC_CACHE_TILES`]; the test
/// hook is what lets a suite-sized canvas cross a cap without building the
/// 64 Mpx document a real 16 384-tile crossing would need. Thread-local
/// because cargo runs a binary's tests in parallel threads, and a global
/// would leak one test's cap into another test's derive.
fn src_cache_cap() -> usize {
    #[cfg(test)]
    {
        let n = SRC_CAP_OVERRIDE.with(|c| c.get());
        if n > 0 {
            return n;
        }
    }
    SRC_CACHE_TILES
}

#[cfg(test)]
thread_local! {
    static SRC_CAP_OVERRIDE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// One tile of correction work, as a kernel host sees it.
///
/// `src` is the below-composite (premultiplied fix15 RGBA, `TILE_LEN` long)
/// — over paper at page scope, so opaque; over nothing inside a sealed
/// folder, so carrying the group's own alpha. `cov` is the window's
/// per-pixel coverage
/// (`TILE_PIXELS` bytes), `None` meaning "no window, correct everything".
/// Exactly the two arguments [`correct_tile`] takes — the CPU function IS
/// the specification of what a kernel must reproduce.
pub struct CorrTile<'a> {
    pub src: &'a [u16],
    pub cov: Option<&'a [u8]>,
}

/// A kernel the caller lends the correction derive: given the parameters and
/// a batch of tiles, return one `TILE_LEN` buffer per tile — or `None` to
/// decline the whole batch, in which case [`correct_tile`] runs on the CPU.
///
/// Declining is always legal and always correct. That is the contract that
/// lets `mn-gpu` hand over a compute path whose adapter, size floor or
/// dispatch canary can veto at any moment without the document ever seeing a
/// half-derived page.
pub type CorrKernel<'a> = dyn FnMut(&Adjust, &[CorrTile<'_>]) -> Option<Vec<Box<[u16]>>> + 'a;

/// The derived state riding a correction layer. Never serialized — ORA
/// stores only the `mnc-correction` params and the mask; everything here
/// rebuilds on load.
#[derive(Clone, Debug, Default)]
pub struct CorrDerived {
    /// The corrected page, tile by tile. What both compositors display.
    pub(crate) tiles: HashMap<TileIdx, Arc<Tile>>,
    /// (params, window-mask key, dpi, canvas size, below props key)
    /// — a mismatch on any of these rebuilds EVERY tile. The window key is
    /// `(revision, full)`: the flag decides what the tiles the mask does
    /// NOT hold mean, so flipping it changes every derived tile without
    /// moving a revision.
    stamp: Option<(Adjust, Option<(u64, bool)>, u32, (u32, u32), u64)>,
    /// The same stamp WITHOUT the parameters — everything the below-composite
    /// depends on. This is the whole point of the split: a slider drag moves
    /// `stamp` and leaves this alone, so every tile's source survives and the
    /// drag pays for the correction arithmetic only.
    ///
    /// Before the split, a param drag re-walked the real compositor once per
    /// tile. Measured on a 2560² page with three art layers
    /// (`gpu/tests/kernel_bench.rs`, release): an uncached tone-curve derive
    /// is 1642 ms, a cached one 1115 ms — the composite is ~32 % of the CPU
    /// figure. That share is small only because the CPU's own tone-curve
    /// arithmetic is expensive; the GPU kernel cuts that term by ~6×, at
    /// which point the composite would be roughly two thirds of every drag
    /// tick. The two changes only pay off together, which is why they shipped
    /// together.
    src_stamp: Option<(Option<(u64, bool)>, u32, (u32, u32), u64)>,
    /// The below-composite each derived tile was made from, kept 8-bit
    /// exactly as `composite_rect_export` produced it — the derive converts
    /// to fix15 on the way in, so caching the wide form would double the cost
    /// for no information. 16 KB per tile: ~205 MB for a full-page maskless
    /// B4/600 correction, against the ~410 MB the derived tiles themselves
    /// already cost. Windowed corrections (the convention) pay in proportion
    /// to the window.
    src: HashMap<TileIdx, Arc<[u8]>>,
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
///
/// `scope` is [`Document::below_scope`]'s answer and rides the hash itself:
/// it IS the scoping rule, and dragging the correction into a folder or
/// flipping that folder's seal moves it without touching a single layer.
fn below_props_key(doc: &Document, i: usize, scope: Option<usize>) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    i.hash(&mut h);
    scope.hash(&mut h);
    let floor = scope.unwrap_or(0);
    // The derivation background: a paper retint must re-derive.
    if let crate::export::Background::Solid(c) = doc.paper_export_background() {
        c.hash(&mut h);
    }
    for l in &doc.layers[floor..i] {
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
        // Part 2: the breakout's seat and its mask cap both change WHERE the
        // layer lands in the composite without moving a tile.
        l.draws_over.hash(&mut h);
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
fn below_tile_keys(doc: &Document, i: usize, floor: usize) -> HashMap<TileIdx, (u64, u32)> {
    let (tw, th) = canvas_tiles(doc.size);
    let in_canvas = |idx: &TileIdx| idx.x >= 0 && idx.y >= 0 && idx.x < tw && idx.y < th;
    let mut keys: HashMap<TileIdx, (u64, u32)> = HashMap::new();
    let mut fold = |idx: TileIdx, rev: u64| {
        let e = keys.entry(idx).or_insert((0, 0));
        e.0 = e.0.max(rev);
        e.1 += 1;
    };
    for l in &doc.layers[floor..i] {
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
        self.refresh_corrections_with(dpi, &mut |_, _| None);
    }

    /// [`Self::refresh_corrections`] with a kernel lent by the caller — the
    /// GPU seam's door into the derive.
    ///
    /// `run` sees one batch of tiles at a time and may decline any of them
    /// (`None`), which runs the CPU reference for that batch instead. Nothing
    /// else changes: the freshness stamps, the source cache and the derived
    /// tiles are written the same way whoever computed the pixels, so a tile
    /// the GPU produced carries exactly the keys a CPU-produced one would and
    /// the next incremental pass cannot tell them apart. That is deliberate —
    /// a GPU-derived tile that did not carry the same keys would re-derive
    /// forever, or worse, never.
    pub fn refresh_corrections_with(&mut self, dpi: u32, run: &mut CorrKernel<'_>) {
        let idxs: Vec<usize> = (0..self.layers.len())
            .filter(|&i| matches!(self.layers[i].kind, LayerKind::Correction(_)))
            .collect();
        for i in idxs {
            self.refresh_correction_at(i, dpi, run);
        }
        // A layer that stopped being a correction sheds its derived state.
        for l in &mut self.layers {
            if !matches!(l.kind, LayerKind::Correction(_)) && l.corr.is_some() {
                l.corr = None;
            }
        }
    }

    fn refresh_correction_at(&mut self, i: usize, dpi: u32, run: &mut CorrKernel<'_>) {
        let LayerKind::Correction(adj) = self.layers[i].kind else {
            return;
        };
        let mask_rev = self.layers[i]
            .mask
            .as_ref()
            .filter(|m| m.enabled)
            .map(|m| (m.revision, m.full));
        // The scope FIRST: it decides which layers the two freshness keys
        // below enumerate, and it is itself part of the props key.
        let scope = self.below_scope(i);
        let floor = scope.unwrap_or(0);
        let props = below_props_key(self, i, scope);
        let stamp = (adj, mask_rev, dpi, self.size, props);
        // The source half of the stamp — the same fields minus the params.
        let src_stamp = (mask_rev, dpi, self.size, props);
        let tile_keys = below_tile_keys(self, i, floor);
        let force = self.layers[i].corr.as_ref().and_then(|c| c.stamp) != Some(stamp);
        // A slider drag lands here: `force` (params moved) but not
        // `force_src` (nothing under the layer moved), so every tile's cached
        // below-composite stands and only the correction re-runs.
        let force_src = self.layers[i].corr.as_ref().and_then(|c| c.src_stamp) != Some(src_stamp);
        if !force
            && self.layers[i]
                .corr
                .as_ref()
                .is_some_and(|c| c.tile_keys == tile_keys)
        {
            return;
        }

        // The tiles this correction derives: the window's tiles when a
        // CARVED mask is on, the whole canvas otherwise. Everywhere else the
        // compositor draws the real layers and the correction has nothing to
        // say.
        //
        // A FULL window (`LayerMask::full`, what a brush stroke on a
        // maskless correction arms) is the "otherwise": its tiles are the
        // places the artist has carved the correction AWAY, and the tiles it
        // does not hold are still corrected — so the derive set is the whole
        // canvas, exactly as if there were no mask at all, and an
        // untouched arm costs no pixels anywhere.
        let (tw, th) = canvas_tiles(self.size);
        let carved = self.layers[i]
            .mask
            .as_ref()
            .filter(|m| m.enabled && !m.full);
        let wanted: Vec<TileIdx> = match carved {
            Some(m) => m
                .tiles
                .keys()
                .copied()
                .filter(|idx| idx.x >= 0 && idx.y >= 0 && idx.x < tw && idx.y < th)
                .collect(),
            None => (0..th)
                .flat_map(|y| (0..tw).map(move |x| TileIdx::new(x, y)))
                .collect(),
        };

        // One truncated clone for the whole rebuild — Arc-shared tiles, so
        // this is pointer traffic, not pixels.
        let below = self.below_doc(i, floor);
        let bg = self.derive_background(scope);
        // The window's coverage, plus what a tile it does not hold means:
        // 0 for a carved window (outside it), full for a `full` one.
        let mask_tiles = self.layers[i]
            .mask
            .as_ref()
            .filter(|m| m.enabled)
            .map(|m| (m.tiles.clone(), if m.full { 255u8 } else { 0 }));

        let mut old = self.layers[i].corr.take().unwrap_or_default();
        let mut out: HashMap<TileIdx, Arc<Tile>> = HashMap::new();
        let mut out_src: HashMap<TileIdx, Arc<[u8]>> = HashMap::new();

        // Pass 1: keep what is still fresh, and collect the rest. Nothing is
        // composited or corrected here — this is the pointer-traffic pass.
        let cap = src_cache_cap();
        let mut todo: Vec<TileIdx> = Vec::new();
        for idx in wanted {
            let key = tile_keys.get(&idx).copied().unwrap_or((0, 0));
            // Both maps are sparse — a tile with no below content is absent
            // from both, and absent must compare EQUAL to absent or every
            // empty tile re-derives on every pass.
            let old_key = old.tile_keys.get(&idx).copied().unwrap_or((0, 0));
            let src_fresh = !force_src && old_key == key;
            if src_fresh
                && out_src.len() < cap
                && let Some(s) = old.src.remove(&idx)
            {
                out_src.insert(idx, s);
            }
            if !force
                && old_key == key
                && let Some(t) = old.tiles.remove(&idx)
            {
                out.insert(idx, t);
                continue;
            }
            todo.push(idx);
        }

        // Pass 2: derive the stale ones, `DERIVE_BATCH` at a time so the
        // fix15 sources stay transient (see the constant).
        let mut src_px = vec![0u16; DERIVE_BATCH * TILE_PIXELS * 4];
        for chunk in todo.chunks(DERIVE_BATCH) {
            let mut covs: Vec<Option<Box<[u8; TILE_PIXELS]>>> = Vec::with_capacity(chunk.len());
            for (n, &idx) in chunk.iter().enumerate() {
                // The source: the cache when the pass-1 compare kept it,
                // otherwise a fresh walk of the real compositor.
                let cached = out_src.get(&idx).map(Arc::clone);
                let bytes = match cached {
                    Some(s) => s,
                    None => {
                        let (ox, oy) = idx.origin();
                        let img = crate::export::composite_rect_export(
                            &below,
                            bg,
                            TILE_SIZE as u32,
                            TILE_SIZE as u32,
                            ox,
                            oy,
                        );
                        let s: Arc<[u8]> = Arc::from(img.into_raw().into_boxed_slice());
                        // Past the cap the source is used for this tile and
                        // then dropped: correctness never depended on the
                        // cache, only the drag's cost does.
                        if out_src.len() < cap {
                            out_src.insert(idx, Arc::clone(&s));
                        }
                        s
                    }
                };
                // The below-composite as a premultiplied fix15 tile. Under
                // page scope the source is opaque by construction (the
                // paper) and this is the old byte-for-byte conversion; under
                // group scope the alpha is real and rides along, so
                // `correct_tile` corrects the straight colour and hands the
                // group's own coverage back. Off-canvas pixels stay
                // transparent (the image is zero there) and `correct_tile`
                // passes them through; the compositor clips them anyway.
                let dst = &mut src_px[n * TILE_PIXELS * 4..(n + 1) * TILE_PIXELS * 4];
                dst.fill(0);
                for p in 0..TILE_PIXELS {
                    let o = p * 4;
                    let a = bytes[o + 3] as u32;
                    if a == 0 {
                        continue;
                    }
                    // 255 → exactly 32768, so the opaque path is unchanged.
                    let a15 = (a * 32768 + 127) / 255;
                    for c in 0..3 {
                        dst[o + c] = ((bytes[o + c] as u32 * a15 + 127) / 255).min(a15) as u16;
                    }
                    dst[o + 3] = a15 as u16;
                }
                covs.push(mask_tiles.as_ref().and_then(|(mt, absent)| {
                    match mt.get(&idx) {
                        Some(m) => {
                            let mut cov = Box::new([0u8; TILE_PIXELS]);
                            let d = m.data();
                            for (p, c) in cov.iter_mut().enumerate() {
                                *c = ((d[p * 4 + 3] as u32 * 255 + 16384) / 32768).min(255) as u8;
                            }
                            Some(cov)
                        }
                        // Full coverage IS "no window" as far as
                        // `correct_tile` is concerned — handing `None`
                        // instead of 256 opaque bytes keeps the kernel on
                        // its cheap path for the tiles a full window has
                        // never been carved out of, which is most of them.
                        None if *absent == 255 => None,
                        None => Some(Box::new([0u8; TILE_PIXELS])),
                    }
                }));
            }

            let job: Vec<CorrTile<'_>> = chunk
                .iter()
                .enumerate()
                .map(|(n, _)| CorrTile {
                    src: &src_px[n * TILE_PIXELS * 4..(n + 1) * TILE_PIXELS * 4],
                    cov: covs[n].as_deref().map(|c| &c[..]),
                })
                .collect();
            let lent = run(&adj, &job);
            drop(job);

            let usable = lent.filter(|t| {
                t.len() == chunk.len() && t.iter().all(|px| px.len() == TILE_PIXELS * 4)
            });
            match usable {
                Some(tiles) => {
                    for (&idx, px) in chunk.iter().zip(tiles) {
                        let mut tile = Tile::new_transparent();
                        tile.data_mut().copy_from_slice(&px);
                        out.insert(idx, Arc::new(tile));
                    }
                }
                // Declined, or a host that returned the wrong count / wrong
                // tile length (a bug on its side, never a reason to write
                // short tiles): the CPU reference stands.
                None => {
                    for (n, &idx) in chunk.iter().enumerate() {
                        let mut tile = Tile::new_transparent();
                        correct_tile(
                            tile.data_mut(),
                            &src_px[n * TILE_PIXELS * 4..(n + 1) * TILE_PIXELS * 4],
                            &adj,
                            covs[n].as_deref(),
                        );
                        out.insert(idx, Arc::new(tile));
                    }
                }
            }
        }

        self.layers[i].corr = Some(CorrDerived {
            tiles: out,
            stamp: Some(stamp),
            src_stamp: Some(src_stamp),
            src: out_src,
            tile_keys,
        });
    }

    /// What a correction at `i` derives FROM: `Some(start)` — the first
    /// index of the sealed group it lives in — or `None` for the page.
    ///
    /// The convention was top-level, so the answer was always the page. A
    /// correction dragged INSIDE a folder kept deriving from the global
    /// below-set, which is not what a group means and (worse) is not even
    /// what it got — a truncated stack that starts mid-folder has orphan
    /// children at depth 1, they composite into an accumulator nothing ever
    /// blends down, and the folder's own art silently vanished from the
    /// derivation while the derived page flooded the group.
    ///
    /// **The rule, mirroring the compositor rather than re-deciding it.**
    /// `export::composite_size` gives each layer the accumulator
    /// `collapse[depth]`: a SEALED folder opens a new one for its children,
    /// a THROUGH folder collapses onto its parent's. So the scope is the
    /// nearest SEALED enclosing folder — walk out through Through folders,
    /// and the first sealed one's children are the below-set; none at all
    /// (or Through all the way out) means the page. That is the same
    /// sealed-is-the-group / Through-is-the-page ruling Blend If takes, and
    /// for the same reason: both questions are "what has this layer's
    /// accumulator got in it so far".
    ///
    /// The `Some`/`None` distinction is not just the start index — see
    /// [`Self::below_doc`]'s background. A sealed group whose children
    /// happen to start at index 0 still scopes as a GROUP.
    pub(crate) fn below_scope(&self, i: usize) -> Option<usize> {
        let mut cur = i;
        while let Some(f) = self.enclosing_folder(cur) {
            if !self.layers[f].through {
                return Some(self.children_range(f).start);
            }
            cur = f;
        }
        None
    }

    /// The below-set of a correction at `i`, as its own document — the
    /// derivation source. Everything the composite walk reads is carried;
    /// undo history and selection are not (and must not be — the composite
    /// never looks).
    ///
    /// `normalize_depths` is load-bearing, not tidiness: a folder-local
    /// slice starts BELOW its folder header, so its children are orphans at
    /// depth 1. `composite_size` initialises its depth→accumulator collapse
    /// to the identity and only rewrites it at a header, so an orphan lands
    /// in accumulator 1 — which nothing blends onto accumulator 0, which is
    /// the only one the page is blitted from. Un-normalised, the derivation
    /// comes back blank. Re-basing to 0 is exactly what the missing header
    /// would have done for a sealed group, and exactly what a Through
    /// folder does for real.
    fn below_doc(&self, i: usize, floor: usize) -> Document {
        let mut d = Document::new(self.size.0.max(1), self.size.1.max(1));
        d.layers = self.layers[floor..i].to_vec();
        d.paper = self.paper.clone();
        d.normalize_depths();
        d
    }

    /// The background a correction at `i` derives over.
    ///
    /// Page scope keeps the paper: that is what makes every derived pixel
    /// OPAQUE, which is what makes `Blend::Normal` a replace, which is the
    /// whole trick (see the module doc). GROUP scope must not — a sealed
    /// folder is isolated, so the page beneath it is not visible to a child
    /// (the same isolation ruling Blend If takes), and deriving over paper
    /// there floods the whole group with opaque corrected paper and covers
    /// the page with it. Transparent instead: the group's own composite,
    /// alpha and all, and the derived tile stays transparent exactly where
    /// the group has nothing to correct.
    ///
    /// The replace trick survives where it matters, because group art is
    /// opaque where it exists. It DEGRADES on semi-transparent group
    /// content: an alpha-`a` pixel comes out `C·a + G·(1−a)` rather than
    /// `C·a`, since Normal-over-itself is only a replace at `a == 1`. That
    /// is the same no-Replace-blend compromise this module already lives
    /// with, now visible at soft edges inside a group.
    fn derive_background(&self, scope: Option<usize>) -> crate::export::Background {
        match scope {
            Some(_) => crate::export::Background::Transparent,
            None => self.paper_export_background(),
        }
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

    /// The parameter-drag win, asserted where it actually lives: a param-only
    /// change must NOT re-walk the compositor. The below-composite sources
    /// are `Arc`s, so pointer identity across the change is proof the cache
    /// served them — and pointer INequality after a stroke below is proof the
    /// cache still invalidates.
    #[test]
    fn a_param_change_reuses_the_cached_below_composite() {
        let mut doc = Document::new(128, 128);
        put(&mut doc, 0, 10, 10, [0.6, 0.6, 0.6, 1.0]);
        let ci = doc.add_correction_layer(Adjust::Invert, false);
        refresh(&mut doc);
        let before: Vec<(TileIdx, Arc<[u8]>)> = doc.layers[ci]
            .corr
            .as_ref()
            .unwrap()
            .src
            .iter()
            .map(|(i, s)| (*i, Arc::clone(s)))
            .collect();
        assert_eq!(before.len(), 4, "a 128² canvas is four tiles of source");

        // Params move, nothing below does.
        doc.layers[ci].kind = LayerKind::Correction(Adjust::Binarize { threshold: 0.7 });
        refresh(&mut doc);
        let after = &doc.layers[ci].corr.as_ref().unwrap().src;
        for (idx, s) in &before {
            assert!(
                Arc::ptr_eq(s, after.get(idx).expect("the source survived")),
                "tile {idx:?} re-composited for a parameter change"
            );
        }
        // …and the pixels really did change, so this is not a stale-derive
        // test passing for the wrong reason.
        assert_eq!(px(&doc, 100, 100), [255, 255, 255], "paper binarizes white");

        // A stroke below must still throw its tile's source away.
        put(&mut doc, 0, 12, 12, [0.2, 0.2, 0.2, 1.0]);
        refresh(&mut doc);
        let after = &doc.layers[ci].corr.as_ref().unwrap().src;
        let near = TileIdx::new(0, 0);
        let far = TileIdx::new(1, 1);
        let old = |i: TileIdx| before.iter().find(|(k, _)| *k == i).map(|(_, s)| s).unwrap();
        assert!(
            !Arc::ptr_eq(old(near), after.get(&near).unwrap()),
            "the stroked tile kept a stale below-composite"
        );
        assert!(
            Arc::ptr_eq(old(far), after.get(&far).unwrap()),
            "an untouched tile re-composited anyway"
        );
    }

    /// The source cache's one real hazard: a correction stacked ON another
    /// correction derives from the lower one's DERIVED raster, so moving the
    /// lower one's parameters has to invalidate the upper one's cached
    /// sources — even though nobody painted anything and the upper layer's
    /// own parameters did not move.
    ///
    /// It holds because a re-derived tile is a fresh `Tile`, which takes a
    /// fresh monotonic revision, and a correction's derived tiles ARE its
    /// `display_tiles()` — so `below_tile_keys` sees the change. That is a
    /// load-bearing coincidence of three separate decisions, which is
    /// exactly the kind of thing that stops being true silently.
    #[test]
    fn a_correction_above_another_sees_the_lower_ones_params_move() {
        let mut doc = Document::new(128, 128);
        put(&mut doc, 0, 10, 10, [0.6, 0.6, 0.6, 1.0]);
        let lower = doc.add_correction_layer(Adjust::Invert, false);
        doc.set_active(lower);
        let upper = doc.add_correction_layer(
            Adjust::BrightnessContrast {
                brightness: 0.0,
                contrast: 0.0,
            },
            false,
        );
        assert!(upper > lower, "the second correction stacks above the first");
        refresh(&mut doc);
        // White paper, inverted by the lower layer, passed through by the
        // upper one's neutral brightness.
        assert!(px(&doc, 100, 100)[0] <= 2, "the stack starts inverted");
        let src_before: Vec<Arc<[u8]>> = doc.layers[upper]
            .corr
            .as_ref()
            .unwrap()
            .src
            .values()
            .cloned()
            .collect();

        // Only the LOWER layer's parameters move.
        doc.layers[lower].kind = LayerKind::Correction(Adjust::Binarize { threshold: 0.9 });
        refresh(&mut doc);
        assert_eq!(
            px(&doc, 100, 100),
            [255, 255, 255],
            "the upper correction is still deriving from a stale source"
        );
        let after = &doc.layers[upper].corr.as_ref().unwrap().src;
        assert!(
            src_before
                .iter()
                .all(|s| !after.values().any(|t| Arc::ptr_eq(s, t))),
            "the upper layer kept a source that predates the lower layer's edit"
        );
    }

    /// The lent kernel replaces `correct_tile` and nothing else: the derived
    /// tiles are whatever it returned, and the freshness bookkeeping around
    /// it is unchanged, so the next pass still rebuilds nothing.
    #[test]
    fn a_lent_kernel_derives_the_tiles_and_keeps_the_freshness_keys() {
        let mut doc = Document::new(128, 128);
        put(&mut doc, 0, 10, 10, [0.6, 0.6, 0.6, 1.0]);
        doc.add_correction_layer(Adjust::Invert, false);
        let mut calls = 0usize;
        doc.refresh_derived_with(600, &mut |_, tiles| {
            calls += 1;
            // A recognisable constant: mid-grey, fully opaque.
            Some(
                (0..tiles.len())
                    .map(|_| {
                        let mut t = vec![0u16; TILE_PIXELS * 4];
                        for p in 0..TILE_PIXELS {
                            t[p * 4] = 16384;
                            t[p * 4 + 1] = 16384;
                            t[p * 4 + 2] = 16384;
                            t[p * 4 + 3] = 32768;
                        }
                        t.into_boxed_slice()
                    })
                    .collect(),
            )
        });
        assert_eq!(calls, 1, "one batch for a four-tile canvas");
        let got = px(&doc, 10, 10);
        assert!(
            (got[0] as i32 - 128).abs() <= 2,
            "the kernel's pixels are what the page shows: {got:?}"
        );

        // Nothing moved, so a second pass must not call the kernel at all.
        let mut again = 0usize;
        doc.refresh_derived_with(600, &mut |_, _| {
            again += 1;
            None
        });
        assert_eq!(
            again, 0,
            "a GPU-derived tile did not carry the freshness keys — it would \
             re-derive on every frame"
        );
    }

    /// A kernel that declines — or hands back the wrong shape, which is the
    /// same thing — must leave the CPU reference in charge, byte for byte.
    #[test]
    fn a_declining_or_malformed_kernel_falls_back_to_the_cpu() {
        let build = |k: &mut CorrKernel<'_>| {
            let mut doc = Document::new(128, 128);
            put(&mut doc, 0, 10, 10, [0.6, 0.6, 0.6, 1.0]);
            doc.add_correction_layer(Adjust::Binarize { threshold: 0.7 }, false);
            doc.refresh_derived_with(600, k);
            crate::export::composite(&doc, crate::export::Background::White)
        };
        let reference = build(&mut |_, _| None);
        // Too few tiles back.
        let short = build(&mut |_, _| Some(Vec::new()));
        // Right count, wrong length — a host bug that must never be written
        // into a tile.
        let ragged = build(&mut |_, tiles| {
            Some((0..tiles.len()).map(|_| vec![0u16; 7].into_boxed_slice()).collect())
        });
        assert!(
            reference.pixels().zip(short.pixels()).all(|(a, b)| a.0 == b.0),
            "a short kernel result changed the page"
        );
        assert!(
            reference.pixels().zip(ragged.pixels()).all(|(a, b)| a.0 == b.0),
            "a ragged kernel result changed the page"
        );
    }

    /// The source cache is bounded. Before the cap it grew to one entry per
    /// derived tile with nothing ever evicting it — canvas-sized, 16 KB
    /// apiece — so this asserts the ceiling holds AND that crossing it does
    /// not change a single derived pixel, because the cache was only ever an
    /// optimisation.
    #[test]
    fn the_source_cache_stops_growing_at_its_cap() {
        let mut doc = Document::new(256, 256); // 4 × 4 = 16 tiles of source
        put(&mut doc, 0, 10, 10, [0.6, 0.6, 0.6, 1.0]);
        let ci = doc.add_correction_layer(Adjust::Invert, false);
        refresh(&mut doc);
        assert_eq!(
            doc.layers[ci].corr.as_ref().unwrap().src.len(),
            16,
            "uncapped, every derived tile caches its source"
        );
        let uncapped = crate::export::composite(&doc, crate::export::Background::White);

        SRC_CAP_OVERRIDE.with(|c| c.set(5));
        let mut capped = Document::new(256, 256);
        put(&mut capped, 0, 10, 10, [0.6, 0.6, 0.6, 1.0]);
        let ci = capped.add_correction_layer(Adjust::Invert, false);
        refresh(&mut capped);
        SRC_CAP_OVERRIDE.with(|c| c.set(0));
        assert_eq!(
            capped.layers[ci].corr.as_ref().unwrap().src.len(),
            5,
            "the cap did not hold"
        );
        let got = crate::export::composite(&capped, crate::export::Background::White);
        assert!(
            uncapped.pixels().zip(got.pixels()).all(|(a, b)| a.0 == b.0),
            "a capped cache changed the derived page"
        );
    }

    /// …and the cap must not cost the drag win. Under a cap the retained
    /// entries are KEPT (rather than cycled the way an LRU would cycle them
    /// on a full-canvas scan), so a parameter drag still serves those tiles
    /// from cache tick after tick.
    #[test]
    fn a_hot_drag_still_hits_the_capped_cache() {
        SRC_CAP_OVERRIDE.with(|c| c.set(5));
        let mut doc = Document::new(256, 256);
        put(&mut doc, 0, 10, 10, [0.6, 0.6, 0.6, 1.0]);
        let ci = doc.add_correction_layer(Adjust::Invert, false);
        refresh(&mut doc);
        let before: Vec<(TileIdx, Arc<[u8]>)> = doc.layers[ci]
            .corr
            .as_ref()
            .unwrap()
            .src
            .iter()
            .map(|(i, s)| (*i, Arc::clone(s)))
            .collect();
        assert_eq!(before.len(), 5);

        // Two ticks of a parameter drag: the cached five must survive both,
        // by pointer, or the cache is cycling instead of holding.
        for t in [0.7f32, 0.75] {
            doc.layers[ci].kind = LayerKind::Correction(Adjust::Binarize { threshold: t });
            refresh(&mut doc);
            let after = &doc.layers[ci].corr.as_ref().unwrap().src;
            assert_eq!(after.len(), 5, "the cap slipped mid-drag");
            for (idx, s) in &before {
                assert!(
                    Arc::ptr_eq(s, after.get(idx).expect("a cached source was evicted")),
                    "tile {idx:?} re-composited during a drag"
                );
            }
        }
        SRC_CAP_OVERRIDE.with(|c| c.set(0));
        assert_eq!(px(&doc, 100, 100), [255, 255, 255], "paper binarizes white");
    }

    // ---- row 105 edge: the armed all-visible window -------------------

    /// The arm costs NO pixels. An all-visible window is an empty tile map
    /// — the point of `LayerMask::full` — and the page it derives is the
    /// same page a maskless correction derives, tile for tile.
    #[test]
    fn an_armed_full_window_corrects_the_whole_page_and_stores_no_tiles() {
        let mut maskless = Document::new(128, 128);
        put(&mut maskless, 0, 10, 10, [0.6, 0.6, 0.6, 1.0]);
        maskless.add_correction_layer(Adjust::Invert, false);
        refresh(&mut maskless);
        let want = crate::export::composite(&maskless, crate::export::Background::White);

        let mut doc = Document::new(128, 128);
        put(&mut doc, 0, 10, 10, [0.6, 0.6, 0.6, 1.0]);
        let ci = doc.add_correction_layer(Adjust::Invert, false);
        doc.set_active(ci);
        assert!(doc.arm_full_window(), "a maskless correction arms");
        assert!(!doc.arm_full_window(), "and only once");
        let m = doc.layers[ci].mask.as_ref().unwrap();
        assert!(m.full && m.enabled);
        assert!(
            m.tiles.is_empty(),
            "an all-visible window allocated tiles — the whole point is that \
             it does not (a B4/600 dense one is ~30 MB)"
        );
        refresh(&mut doc);
        let got = crate::export::composite(&doc, crate::export::Background::White);
        assert!(
            want.pixels().zip(got.pixels()).all(|(a, b)| a.0 == b.0),
            "an armed window changed the derived page"
        );
    }

    /// …and carving it takes the correction off exactly what was carved.
    /// The carve is written the way `mn-brush`'s mask surface writes it: a
    /// tile materialised from `blank_tile` (OPAQUE, because the window is
    /// full) with the eraser's footprint taken down to zero.
    #[test]
    fn carving_a_full_window_only_uncorrects_what_it_touched() {
        let mut doc = Document::new(256, 256);
        for (x, y) in [(10, 10), (100, 100), (200, 200)] {
            put(&mut doc, 0, x, y, [0.6, 0.6, 0.6, 1.0]);
        }
        let ci = doc.add_correction_layer(Adjust::Invert, false);
        doc.set_active(ci);
        doc.arm_full_window();
        {
            let m = doc.layers[ci].mask.as_mut().unwrap();
            let mut t = m.blank_tile();
            assert_eq!(t.data()[3], 32768, "a full window blanks OPAQUE");
            // Erase a 4x4 corner of tile (1,1) — pixels (64..68, 64..68).
            for y in 0..4 {
                for x in 0..4 {
                    t.set_pixel(x, y, [0; 4]);
                }
            }
            m.tiles.insert(TileIdx::new(1, 1), Arc::new(t));
            m.revision = crate::tile::next_revision();
        }
        refresh(&mut doc);
        assert_eq!(
            px(&doc, 65, 65),
            [255, 255, 255],
            "the carved pixels show the page itself again"
        );
        assert!(
            px(&doc, 80, 80)[0] <= 2,
            "the rest of the carved TILE is still corrected — a zero tile \
             must not hide the whole 64x64: {:?}",
            px(&doc, 80, 80)
        );
        assert!(
            (px(&doc, 100, 100)[0] as i32 - 102).abs() <= 2,
            "and a tile the window never held is still corrected: {:?}",
            px(&doc, 100, 100)
        );
    }

    /// The arm and the stroke it was armed for are ONE undo press, and a
    /// stroke that never dabbed takes the window back instead of spending
    /// a step on nothing.
    #[test]
    fn arming_a_window_costs_one_undo_press_and_an_empty_stroke_costs_none() {
        let mut doc = Document::new(128, 128);
        put(&mut doc, 0, 10, 10, [0.6, 0.6, 0.6, 1.0]);
        let ci = doc.add_correction_layer(Adjust::Invert, false);
        doc.set_active(ci);

        // The pen came down and went up again.
        let steps = doc.undo_len();
        doc.mask_op_begin();
        assert!(doc.arm_full_window());
        assert!(!doc.mask_op_end(), "an empty stroke pushed a step");
        assert!(doc.layers[ci].mask.is_none(), "the speculative arm stayed");
        assert_eq!(doc.undo_len(), steps, "…and cost a press");

        // A real stroke: one press takes the window and the carve together.
        doc.mask_op_begin();
        assert!(doc.arm_full_window());
        {
            let m = doc.layers[ci].mask.as_mut().unwrap();
            let t = m.blank_tile();
            m.tiles.insert(TileIdx::new(0, 0), Arc::new(t));
            m.revision = crate::tile::next_revision();
        }
        assert!(doc.mask_op_end(), "a real mask stroke pushed no step");
        assert_eq!(doc.undo_len(), steps + 1, "the arm and the carve are one");
        doc.undo();
        assert!(
            doc.layers[ci].mask.is_none(),
            "one undo left the armed window behind"
        );
    }

    /// Clear Mask on a full window means what it says. It cannot say it by
    /// zeroing coverage — the visible part of a full window is the tiles it
    /// does NOT hold — so it drops the flag and empties the map, which is a
    /// window that reaches nothing.
    #[test]
    fn clearing_a_full_window_takes_the_correction_off_the_whole_page() {
        let mut doc = Document::new(128, 128);
        put(&mut doc, 0, 10, 10, [0.6, 0.6, 0.6, 1.0]);
        let ci = doc.add_correction_layer(Adjust::Invert, false);
        doc.set_active(ci);
        doc.arm_full_window();
        refresh(&mut doc);
        assert!(px(&doc, 100, 100)[0] <= 2, "armed: the page is inverted");
        assert!(doc.mask_clear(ci));
        refresh(&mut doc);
        assert_eq!(
            px(&doc, 100, 100),
            [255, 255, 255],
            "cleared: the correction reaches nothing"
        );
        assert!((px(&doc, 10, 10)[0] as i32 - 153).abs() <= 2, "including the ink");
    }

    /// A carved full window survives ORA — without the flag the reload
    /// would read the carve tiles as the window and correct only there,
    /// which is the exact inverse of what was saved.
    #[test]
    fn a_full_window_round_trips_through_ora() {
        let mut doc = Document::new(128, 128);
        put(&mut doc, 0, 10, 10, [0.6, 0.6, 0.6, 1.0]);
        let ci = doc.add_correction_layer(Adjust::Invert, false);
        doc.set_active(ci);
        doc.arm_full_window();
        {
            let m = doc.layers[ci].mask.as_mut().unwrap();
            let mut t = m.blank_tile();
            for y in 0..8 {
                for x in 0..8 {
                    t.set_pixel(x, y, [0; 4]);
                }
            }
            m.tiles.insert(TileIdx::new(0, 0), Arc::new(t));
            m.revision = crate::tile::next_revision();
        }
        refresh(&mut doc);
        let before = crate::export::composite(&doc, crate::export::Background::White);

        let mut buf = std::io::Cursor::new(Vec::new());
        crate::ora::save_to(&doc, &mut buf).unwrap();
        let mut back =
            crate::ora::load_from(std::io::Cursor::new(buf.into_inner())).unwrap();
        assert!(
            back.layers.iter().any(|l| l.mask.as_ref().is_some_and(|m| m.full)),
            "the full-window flag did not survive the save"
        );
        back.refresh_derived(600);
        let after = crate::export::composite(&back, crate::export::Background::White);
        assert!(
            before.pixels().zip(after.pixels()).all(|(a, b)| a.0 == b.0),
            "the reloaded window derives a different page"
        );
    }

    // ---- the in-folder scope ------------------------------------------

    /// A page layer, a folder holding one art layer, and a correction
    /// stacked on that art INSIDE the folder. Returns (doc, correction).
    fn in_folder(through: bool) -> (Document, usize) {
        let mut doc = Document::new(128, 128);
        // Below the folder, on the page: must never be corrected by a
        // correction sealed inside the group.
        put(&mut doc, 0, 100, 100, [0.6, 0.6, 0.6, 1.0]);
        let f = doc.add_folder_above(0, "group");
        let art = doc.add_layer_in_folder(f, "art").unwrap();
        put(&mut doc, art, 10, 10, [0.6, 0.6, 0.6, 1.0]);
        doc.set_active(art);
        let ci = doc.add_correction_layer(Adjust::Invert, false);
        let header = doc.enclosing_folder(ci).expect("the correction is inside");
        doc.layers[header].through = through;
        assert_eq!(doc.layers[ci].depth, 1, "the correction is a child");
        (doc, ci)
    }

    /// The ruling: a correction INSIDE a sealed folder derives from that
    /// folder's own children below it, not from the page.
    ///
    /// Before the fix it derived from `layers[..i]` — the global set — and
    /// got the worst of both: the folder's own art was an orphan at depth 1
    /// in the truncated clone, composited into an accumulator nothing
    /// blends down, so it VANISHED from the derivation; and the paper came
    /// along, so the derived tiles were opaque everywhere and the group
    /// covered the page with them.
    #[test]
    fn a_correction_inside_a_sealed_folder_derives_from_the_folder() {
        let (mut doc, ci) = in_folder(false);
        assert_eq!(doc.below_scope(ci), Some(1), "sealed = the group");
        refresh(&mut doc);
        assert!(
            (px(&doc, 10, 10)[0] as i32 - 102).abs() <= 2,
            "the group's own art is what got corrected: {:?}",
            px(&doc, 10, 10)
        );
        assert!(
            (px(&doc, 100, 100)[0] as i32 - 153).abs() <= 2,
            "the page BELOW the folder is outside the group and untouched: {:?}",
            px(&doc, 100, 100)
        );
        assert_eq!(
            px(&doc, 60, 60),
            [255, 255, 255],
            "and where the group has nothing the derived tile is transparent \
             — no flood of corrected paper"
        );
    }

    /// The other half of the same ruling: a THROUGH folder has no seal, so
    /// a correction in one still corrects the page, exactly as if it were
    /// loose at the top level.
    #[test]
    fn a_correction_inside_a_through_folder_still_derives_from_the_page() {
        let (mut doc, ci) = in_folder(true);
        assert_eq!(doc.below_scope(ci), None, "Through = the page");
        refresh(&mut doc);
        assert!(
            (px(&doc, 10, 10)[0] as i32 - 102).abs() <= 2,
            "the group's art is corrected: {:?}",
            px(&doc, 10, 10)
        );
        assert!(
            (px(&doc, 100, 100)[0] as i32 - 102).abs() <= 2,
            "and so is the page under the folder — that is what Through \
             means: {:?}",
            px(&doc, 100, 100)
        );
        assert!(
            px(&doc, 60, 60)[0] <= 2,
            "including the bare paper: {:?}",
            px(&doc, 60, 60)
        );
    }

    /// Flipping the seal moves nothing but a bool — no tile revision, no
    /// layer under the correction — so the freshness keys have to catch it
    /// on the SCOPE alone.
    #[test]
    fn flipping_the_folders_seal_re_derives_the_correction() {
        let (mut doc, ci) = in_folder(false);
        refresh(&mut doc);
        assert!((px(&doc, 100, 100)[0] as i32 - 153).abs() <= 2);
        let header = doc.enclosing_folder(ci).unwrap();
        doc.layers[header].through = true;
        refresh(&mut doc);
        assert!(
            (px(&doc, 100, 100)[0] as i32 - 102).abs() <= 2,
            "the correction kept a group-scoped derive after the seal opened: \
             {:?}",
            px(&doc, 100, 100)
        );
    }

    /// A correction nested in a THROUGH folder inside a SEALED one scopes
    /// to the sealed grandparent — the walk mirrors the compositor's
    /// depth→accumulator collapse, which a Through folder does not open.
    #[test]
    fn a_through_folder_inside_a_sealed_one_scopes_to_the_sealed_grandparent() {
        let mut doc = Document::new(128, 128);
        put(&mut doc, 0, 100, 100, [0.6, 0.6, 0.6, 1.0]);
        let outer = doc.add_folder_above(0, "outer");
        let inner = doc.add_folder_above(outer - 1, "inner");
        doc.layers[inner].depth = 1;
        doc.layers[inner].through = true;
        let art = doc.add_layer_in_folder(inner, "art").unwrap();
        doc.set_active(art);
        let ci = doc.add_correction_layer(Adjust::Invert, false);
        let outer = doc
            .layers
            .iter()
            .position(|l| l.name == "outer")
            .expect("the outer folder is still there");
        assert!(!doc.layers[outer].through);
        assert_eq!(
            doc.below_scope(ci),
            Some(doc.children_range(outer).start),
            "the Through folder was walked through to the sealed one"
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
