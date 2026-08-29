//! Row 160 / `RD-001`–`RD-009` — **Remove dust** (CSP ゴミ取り): the scan
//! cleanup pass, plus the three other things CSP calls "dust".
//!
//! # The four definitions (RD-003)
//!
//! CSP's Mode row is a 4-way, and the four are two detections crossed with
//! what you do about them ([`DustMode`]):
//!
//! 1. **on transparency** — an isolated blob of INK floating in emptiness.
//!    The scanner speck. Cleared.
//! 2. **on white background** — an isolated blob that is merely DARKER than
//!    the paper around it. Repainted white, because on a white-background
//!    scan "clean" means paper, not a hole.
//! 3. **transparent gaps, surrounding colour** — the inverse: a small
//!    TRANSPARENT pocket enclosed by ink. These are the anti-aliased
//!    pinholes a bucket fill leaves along a line, and the reason this tool
//!    ships with the fill subsystem rather than as a scan filter. Painted
//!    with the average colour of the ink that rings the hole.
//! 4. **transparent gaps, drawing colour** — same holes, current colour.
//!
//! RD-009's select-side Mode row is a 3-way rather than a 4-way for exactly
//! one reason: modes 3 and 4 DETECT the same pixels, so selecting collapses
//! them ([`DustMode::detects_gaps`]).
//!
//! # What is NOT built
//!
//! * `RD-004`/`RD-006` — the shared "advanced categories" block (Reference,
//!   Figure, Shape operation). Both the scrub and the find read the ACTIVE
//!   layer's own pixels, full stop; there is no 参照 axis here the way the
//!   fill family has one. A dust pass is a repair of the layer you are
//!   looking at, and a reference-layer variant of "is this pixel a speck"
//!   has no answer that is not just the active layer's answer.
//! * `RD-008` — the New/Add/Subtract/Intersect row on the select half. That
//!   is `S-002`, absent house-wide (we have no boolean selection modes in
//!   any tool), so the find replaces, like every other selection producer.
//! * `RD-005` — CSP's 塗り残し部分に塗る brush already ships, as the Fill
//!   tool's Leftover pen (row 119 / FI-005). What it does NOT do is pull
//!   colour from the surroundings; it paints the drawing colour. That half
//!   of the row is what [`DustMode::GapsSurrounding`] answers, as a region
//!   op rather than a brush.
//!
//! # The unit is AREA
//!
//! `max_px` is the count of pixels in a blob, never its width — the same
//! unit (and the same 1..=256 clamp, [`crate::filter::dust_max`]) LC-001's
//! menu filter already uses, so the two rows cannot mean different things.
//!
//! # The component pass, and why it is not a flood per speck
//!
//! One linear scan over the lifted buffer. Every target pixel is visited
//! once and lands in one of three states — unseen, kept (a component that
//! came in under the threshold), or rejected. A walk that runs past
//! `max_px` pixels, touches the buffer rim, or meets an already-rejected
//! pixel stops right there and marks everything it saw REJECTED, so the
//! rest of a big drawing is never explored: the next walk that reaches it
//! meets a rejected neighbour and stops again. Rejection therefore spreads
//! through a big blob a bounded bite at a time, and no walk ever costs more
//! than `max_px` + one neighbourhood.
//!
//! Cost is **O(A)** time and O(A) bytes of scratch, where A is the LIFTED
//! buffer's area — the drag window (or the selection) grown by the halo,
//! not the page. A 40 px scrub on a B4/600dpi page touches ~40² px, not
//! 24 M. Both callers get that windowing for free: the scrub goes through
//! [`crate::filter::Filter::Dust`] and inherits `apply_filter`'s tile
//! gather, and [`Document::dust_selection`] does the same arithmetic itself.
//!
//! ## Why the rim rule is safe
//!
//! `Filter::reach` for a dust op is `max_px`, so the lifted buffer extends
//! that far past every pixel that can be WRITTEN. A component holding both
//! a writable pixel and a rim pixel therefore spans at least `max_px`
//! pixels of distance, which means at least `max_px + 1` pixels of area —
//! already too big to be dust. Rejecting rim-touchers can only ever throw
//! away components that were going to be rejected anyway, and it is what
//! keeps the enormous OUTER transparency of a page from ever reading as a
//! "gap" on a canvas small enough for the size test alone to miss it.
//!
//! ## Connectivity
//!
//! Specks are 8-connected and gaps are 4-connected — the standard pairing.
//! 8 for the ink half is LC-001's own argument (a scanner speck is as often
//! a diagonal pair as a square one); 4 for the hole half is the complement,
//! and it stops a pinhole from leaking diagonally through a 1 px line into
//! the open air, which would make every hole along an anti-aliased edge
//! read as one enormous non-dust region.

use crate::doc::Document;
use crate::filter::{Raster, dust_max, gather};
use crate::selection::{Selection, selected};
use crate::tile::TILE_CHANNELS;

/// Fix15 one.
const ONE: u32 = 32768;
/// Half coverage — the line between "a hole" and "ink" for the gap modes.
const HALF: u16 = 16384;
/// The class line for "white background", in fix15 luma: the same 192/255
/// `FillClose` calls white, so the fill family and the dust family agree
/// about what paper is.
const WHITE_LUMA: u32 = 192 * ONE / 255;

/// RD-003 — the four definitions of "dust".
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DustMode {
    /// 透明部分のゴミを消す: clear small blobs of ink floating in emptiness.
    #[default]
    OnTransparency,
    /// 白地のゴミを消す: repaint small blobs darker than the paper WHITE.
    OnWhite,
    /// 透明な隙間を周囲の色で塗る: fill small enclosed transparent holes
    /// with the average colour of the ink around them.
    GapsSurrounding,
    /// 透明な隙間を描画色で塗る: the same holes, in the current colour.
    GapsForeground,
}

impl DustMode {
    pub const ALL: [DustMode; 4] = [
        DustMode::OnTransparency,
        DustMode::OnWhite,
        DustMode::GapsSurrounding,
        DustMode::GapsForeground,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DustMode::OnTransparency => "Remove dust on transparency",
            DustMode::OnWhite => "Remove dust on white background",
            DustMode::GapsSurrounding => "Fill gaps with surrounding colour",
            DustMode::GapsForeground => "Fill gaps with drawing colour",
        }
    }

    /// RD-009: what the SELECT half of the tool calls this detection. The
    /// two gap fills answer the same thing — CSP's select-side row is a
    /// 3-way for that reason and so is ours.
    pub fn select_label(self) -> &'static str {
        match self {
            DustMode::OnTransparency => "Select dust on transparency",
            DustMode::OnWhite => "Select dust on white background",
            DustMode::GapsSurrounding | DustMode::GapsForeground => "Select transparent gaps",
        }
    }

    /// True for the two modes that hunt HOLES rather than specks.
    pub fn detects_gaps(self) -> bool {
        matches!(self, DustMode::GapsSurrounding | DustMode::GapsForeground)
    }

    /// Is this pixel a candidate — a speck pixel, or a hole pixel?
    fn targets(self, px: [u16; 4]) -> bool {
        match self {
            // Any ink at all, at any alpha: dust is usually the faint grey
            // the scanner invented, and thresholding it away would keep
            // exactly the specks worth removing (LC-001's rule, verbatim).
            DustMode::OnTransparency => px[3] != 0,
            // Darker than paper. Measured OVER WHITE, so a transparent
            // pixel reads as the paper it shows and never counts.
            DustMode::OnWhite => luma_over_white(px) < WHITE_LUMA,
            DustMode::GapsSurrounding | DustMode::GapsForeground => px[3] < HALF,
        }
    }
}

/// Rec.709 luma of a premultiplied fix15 pixel composited over white —
/// the same 54/183/19 integer weights the fill family's `luma_u8` uses.
fn luma_over_white(px: [u16; 4]) -> u32 {
    let clear = ONE - px[3] as u32;
    let c = |i: usize| px[i] as u32 + clear;
    (c(0) * 54 + c(1) * 183 + c(2) * 19) >> 8
}

/// Every component of `mode`'s target pixels that comes in at `max_px`
/// pixels of AREA or fewer, as a flat list of buffer indices plus one END
/// offset per component (so callers that need per-component grouping —
/// the surrounding-colour fill does — get it without a Vec of Vecs).
///
/// See the module docs for the state machine and its cost.
fn components(buf: &Raster, mode: DustMode, max_px: u32) -> (Vec<usize>, Vec<usize>) {
    let max = dust_max(max_px) as usize;
    let (w, h) = (buf.w, buf.h);
    let mut flat: Vec<usize> = Vec::new();
    let mut ends: Vec<usize> = Vec::new();
    if w == 0 || h == 0 {
        return (flat, ends);
    }
    let target: Vec<bool> = (0..w * h)
        .map(|p| {
            let o = p * TILE_CHANNELS;
            mode.targets([
                buf.px[o],
                buf.px[o + 1],
                buf.px[o + 2],
                buf.px[o + 3],
            ])
        })
        .collect();

    const UNSEEN: u8 = 0;
    const KEPT: u8 = 1;
    const REJECTED: u8 = 2;
    let diagonal = !mode.detects_gaps();
    let mut state = vec![UNSEEN; w * h];
    let mut comp: Vec<usize> = Vec::new();
    let mut queue: Vec<usize> = Vec::new();

    for start in 0..w * h {
        if !target[start] || state[start] != UNSEEN {
            continue;
        }
        comp.clear();
        queue.clear();
        state[start] = KEPT;
        comp.push(start);
        queue.push(start);
        let mut rejected = false;
        while let Some(p) = queue.pop() {
            let (x, y) = ((p % w) as i32, (p / w) as i32);
            // The rim rule (module docs): a component that reaches the edge
            // of the lifted buffer is bigger than the threshold by
            // construction, so it is not dust.
            if x == 0 || y == 0 || x as usize == w - 1 || y as usize == h - 1 {
                rejected = true;
                break;
            }
            for ny in y - 1..=y + 1 {
                for nx in x - 1..=x + 1 {
                    if (nx != x && ny != y && !diagonal) || (nx == x && ny == y) {
                        continue;
                    }
                    let q = ny as usize * w + nx as usize;
                    if !target[q] {
                        continue;
                    }
                    match state[q] {
                        // Meeting a rejected pixel settles the whole
                        // component: it is part of something already
                        // known to be too big.
                        REJECTED => rejected = true,
                        UNSEEN => {
                            state[q] = KEPT;
                            comp.push(q);
                            queue.push(q);
                        }
                        _ => {}
                    }
                }
            }
            if rejected || comp.len() > max {
                rejected = true;
                break;
            }
        }
        if rejected {
            for &p in &comp {
                state[p] = REJECTED;
            }
        } else {
            flat.extend_from_slice(&comp);
            ends.push(flat.len());
        }
    }
    (flat, ends)
}

/// The detection alone, as a buffer-sized bool mask — RD-007's "look before
/// you delete" half, and what the tests pin.
pub fn dust_mask(buf: &Raster, mode: DustMode, max_px: u32) -> Vec<bool> {
    let mut mask = vec![false; buf.w * buf.h];
    let (flat, _) = components(buf, mode, max_px);
    for p in flat {
        mask[p] = true;
    }
    mask
}

/// RD-001/RD-003 — do it: clear the specks, or plug the holes. Runs in
/// place on a lifted buffer; `color` is the current drawing colour, used by
/// [`DustMode::GapsForeground`] (and as the fallback for a hole with no ink
/// at all around it, which the rim rule makes unreachable in practice).
pub fn scrub(buf: &mut Raster, mode: DustMode, max_px: u32, color: [f32; 3]) {
    let (flat, ends) = components(buf, mode, max_px);
    let fg = premul_opaque(color);
    let (w, h) = (buf.w, buf.h);
    let mut start = 0usize;
    for end in ends {
        let comp = &flat[start..end];
        start = end;
        match mode {
            DustMode::OnTransparency => {
                for &p in comp {
                    buf.px[p * TILE_CHANNELS..(p + 1) * TILE_CHANNELS].fill(0);
                }
            }
            DustMode::OnWhite => {
                for &p in comp {
                    buf.px[p * TILE_CHANNELS..(p + 1) * TILE_CHANNELS]
                        .copy_from_slice(&[ONE as u16; 4]);
                }
            }
            DustMode::GapsForeground => {
                for &p in comp {
                    buf.px[p * TILE_CHANNELS..(p + 1) * TILE_CHANNELS].copy_from_slice(&fg);
                }
            }
            // The hole takes the average STRAIGHT colour of the ink that
            // rings it — weighted by that ink's own alpha, so a half-
            // covered anti-aliased neighbour votes half as loudly as the
            // solid line behind it and the plug matches the fill, not the
            // fringe.
            DustMode::GapsSurrounding => {
                let mut sum = [0u64; 3];
                let mut wsum = 0u64;
                for &p in comp {
                    let (x, y) = ((p % w) as i32, (p / w) as i32);
                    for ny in (y - 1).max(0)..=(y + 1).min(h as i32 - 1) {
                        for nx in (x - 1).max(0)..=(x + 1).min(w as i32 - 1) {
                            let q = ny as usize * w + nx as usize;
                            let o = q * TILE_CHANNELS;
                            let a = buf.px[o + 3] as u64;
                            if a < HALF as u64 {
                                continue; // another hole pixel, or fringe
                            }
                            for c in 0..3 {
                                // Straight colour · alpha == the
                                // premultiplied value, so the alpha weight
                                // is already in the numerator.
                                sum[c] += buf.px[o + c] as u64;
                            }
                            wsum += a;
                        }
                    }
                }
                let plug = if wsum == 0 {
                    fg
                } else {
                    let mut v = [ONE as u16; 4];
                    for c in 0..3 {
                        v[c] = ((sum[c] * ONE as u64 + wsum / 2) / wsum).min(ONE as u64) as u16;
                    }
                    v
                };
                for &p in comp {
                    buf.px[p * TILE_CHANNELS..(p + 1) * TILE_CHANNELS].copy_from_slice(&plug);
                }
            }
        }
    }
}

/// A straight RGB colour as an opaque premultiplied fix15 pixel.
fn premul_opaque(color: [f32; 3]) -> [u16; 4] {
    [
        crate::blend::f32_to_fix15(color[0]),
        crate::blend::f32_to_fix15(color[1]),
        crate::blend::f32_to_fix15(color[2]),
        ONE as u16,
    ]
}

impl Document {
    /// RD-007/RD-009 **Select dust**: the same detection, handed back as a
    /// [`Selection`] so you can look at what the tool found before deleting
    /// it. Reads the ACTIVE layer, inside the current selection when there
    /// is one (that is the tool's drag window — the caller installs it).
    ///
    /// `None` when there is nothing to look at: no layer footprint, a
    /// window that misses it, or no component under the threshold. Never
    /// touches a pixel and never records undo — this is a query.
    pub fn dust_selection(&self, mode: DustMode, max_px: u32) -> Option<Selection> {
        let layer = self.layers.get(self.active)?;
        let (bx, by, bw, bh) = layer.tile_bounds()?;
        let (cw, ch) = (self.size.0 as i32, self.size.1 as i32);
        // The write rect: the layer's own footprint, clipped to the canvas
        // and to the window. Dust lives IN the ink, so unlike a blur this
        // needs no outward growth to find it.
        let (mut x0, mut y0) = (bx.max(0), by.max(0));
        let (mut x1, mut y1) = ((bx + bw as i32).min(cw), (by + bh as i32).min(ch));
        if let Some(s) = &self.selection {
            let [sx0, sy0, sx1, sy1] = s.bounds()?;
            x0 = x0.max(sx0);
            y0 = y0.max(sy0);
            x1 = x1.min(sx1);
            y1 = y1.min(sy1);
        }
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        // The halo, for the same reason `apply_filter` gathers one: a
        // component that runs off the write rect must still be COUNTED
        // whole, or the tool eats the end of a line it clipped.
        let reach = dust_max(max_px) as i32;
        let (gx, gy) = (x0 - reach, y0 - reach);
        let gw = (x1 - x0 + 2 * reach) as usize;
        let gh = (y1 - y0 + 2 * reach) as usize;
        let buf = gather(layer, gx, gy, gw, gh);
        let found = dust_mask(&buf, mode, max_px);

        let w = self.size.0 as usize;
        let mut region = vec![false; w * self.size.1 as usize];
        let mut any = false;
        for y in y0..y1 {
            for x in x0..x1 {
                if !found[(y - gy) as usize * gw + (x - gx) as usize] {
                    continue;
                }
                if self
                    .selection
                    .as_ref()
                    .is_some_and(|s| !selected(s.coverage(x, y)))
                {
                    continue;
                }
                region[y as usize * w + x as usize] = true;
                any = true;
            }
        }
        any.then(|| Selection::from_mask(self, &region, w))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::Filter;
    use crate::tile::{TILE_SIZE, TileIdx};

    /// Paint one opaque pixel on a layer (straight colour in, premultiplied
    /// out — the pixels are all opaque here).
    fn dot(doc: &mut Document, li: usize, x: i32, y: i32, rgb: [f32; 3]) {
        let idx = TileIdx::of_pixel(x, y);
        let (ox, oy) = idx.origin();
        let o = ((y - oy) as usize * TILE_SIZE + (x - ox) as usize) * 4;
        let d = doc.layers[li].tile_mut(idx).data_mut();
        for c in 0..3 {
            d[o + c] = crate::blend::f32_to_fix15(rgb[c]);
        }
        d[o + 3] = ONE as u16;
    }

    fn rect(doc: &mut Document, li: usize, x0: i32, y0: i32, x1: i32, y1: i32, rgb: [f32; 3]) {
        for y in y0..y1 {
            for x in x0..x1 {
                dot(doc, li, x, y, rgb);
            }
        }
    }

    fn clear(doc: &mut Document, li: usize, x: i32, y: i32) {
        let idx = TileIdx::of_pixel(x, y);
        let (ox, oy) = idx.origin();
        let o = ((y - oy) as usize * TILE_SIZE + (x - ox) as usize) * 4;
        doc.layers[li].tile_mut(idx).data_mut()[o..o + 4].fill(0);
    }

    fn px(doc: &Document, li: usize, x: i32, y: i32) -> [u16; 4] {
        let idx = TileIdx::of_pixel(x, y);
        let (ox, oy) = idx.origin();
        doc.layers[li]
            .tile_arc(idx)
            .map(|t| {
                let o = ((y - oy) as usize * TILE_SIZE + (x - ox) as usize) * 4;
                let d = t.data();
                [d[o], d[o + 1], d[o + 2], d[o + 3]]
            })
            .unwrap_or([0; 4])
    }

    fn dust(mode: DustMode, max_px: u32) -> Filter {
        Filter::Dust {
            max_px,
            mode,
            color: [1.0, 0.0, 0.0],
        }
    }

    /// A page with three specks of 1, 4 and 9 px and one 20×20 drawing.
    /// The threshold decides EXACTLY which of them survive — the pinned
    /// counts, not "something changed".
    fn speck_page() -> Document {
        let mut doc = Document::new(256, 256);
        dot(&mut doc, 0, 20, 20, [0.0, 0.0, 0.0]); // 1 px
        rect(&mut doc, 0, 40, 40, 42, 42, [0.0, 0.0, 0.0]); // 4 px
        rect(&mut doc, 0, 60, 60, 63, 63, [0.0, 0.0, 0.0]); // 9 px
        rect(&mut doc, 0, 100, 100, 120, 120, [0.0, 0.0, 0.0]); // 400 px
        doc
    }

    fn inked(doc: &Document, x: i32, y: i32) -> bool {
        px(doc, 0, x, y)[3] != 0
    }

    #[test]
    fn the_threshold_decides_exactly_which_specks_survive() {
        // 1 px threshold: only the single pixel goes.
        let mut doc = speck_page();
        assert!(doc.apply_filter(dust(DustMode::OnTransparency, 1)));
        assert!(!inked(&doc, 20, 20), "the 1 px speck is dust at 1");
        assert!(inked(&doc, 40, 40), "the 4 px speck is not");
        assert!(inked(&doc, 60, 60));
        assert!(inked(&doc, 110, 110));

        // 4 px: the pair goes too, the 9 px block stays.
        let mut doc = speck_page();
        assert!(doc.apply_filter(dust(DustMode::OnTransparency, 4)));
        assert!(!inked(&doc, 20, 20));
        assert!(!inked(&doc, 40, 40), "4 px is dust at a threshold of 4");
        assert!(!inked(&doc, 41, 41));
        assert!(inked(&doc, 60, 60), "9 px is not");
        assert!(inked(&doc, 110, 110));

        // 9 px: everything but the drawing.
        let mut doc = speck_page();
        assert!(doc.apply_filter(dust(DustMode::OnTransparency, 9)));
        assert!(!inked(&doc, 20, 20));
        assert!(!inked(&doc, 40, 40));
        assert!(!inked(&doc, 62, 62), "9 px is dust at a threshold of 9");
        assert!(inked(&doc, 100, 100), "the drawing is never dust");
        assert!(inked(&doc, 119, 119));
    }

    /// The unit is AREA, not width: a 1 px wide, 30 px long hair is 30 px of
    /// dust and survives a threshold of 9 — while a 3×3 block of the same
    /// 3 px width does not.
    #[test]
    fn the_threshold_is_area_not_width() {
        let mut doc = Document::new(128, 128);
        rect(&mut doc, 0, 10, 10, 11, 40, [0.0, 0.0, 0.0]); // 1×30 hair
        rect(&mut doc, 0, 60, 60, 63, 63, [0.0, 0.0, 0.0]); // 3×3 block
        assert!(doc.apply_filter(dust(DustMode::OnTransparency, 9)));
        assert!(inked(&doc, 10, 25), "30 px of area survives a 9 px threshold");
        assert!(!inked(&doc, 61, 61), "9 px of area does not");
    }

    /// A diagonal chain of four pixels is ONE speck (8-connected), not four.
    #[test]
    fn specks_are_eight_connected() {
        let mut doc = Document::new(128, 128);
        for i in 0..4 {
            dot(&mut doc, 0, 20 + i, 20 + i, [0.0, 0.0, 0.0]);
        }
        assert!(doc.apply_filter(dust(DustMode::OnTransparency, 3)));
        assert!(
            (0..4).all(|i| inked(&doc, 20 + i, 20 + i)),
            "the chain is one 4 px mark, over a 3 px threshold"
        );
        let mut doc = Document::new(128, 128);
        for i in 0..4 {
            dot(&mut doc, 0, 20 + i, 20 + i, [0.0, 0.0, 0.0]);
        }
        assert!(doc.apply_filter(dust(DustMode::OnTransparency, 4)));
        assert!(
            (0..4).all(|i| !inked(&doc, 20 + i, 20 + i)),
            "and it all goes at 4"
        );
    }

    /// RD-003 mode 3: the pinholes a bucket fill leaves inside a flat get
    /// plugged with the flat's own colour — and the big transparent hole
    /// next to them does not.
    #[test]
    fn transparent_pinholes_take_the_surrounding_colour() {
        let mut doc = Document::new(256, 256);
        rect(&mut doc, 0, 40, 40, 100, 100, [0.2, 0.4, 0.8]);
        clear(&mut doc, 0, 50, 50); // 1 px pinhole
        clear(&mut doc, 0, 60, 60); // 4 px pinhole
        clear(&mut doc, 0, 61, 60);
        clear(&mut doc, 0, 60, 61);
        clear(&mut doc, 0, 61, 61);
        for y in 70..80 {
            for x in 70..80 {
                clear(&mut doc, 0, x, y); // 100 px window, not dust
            }
        }
        assert!(doc.apply_filter(dust(DustMode::GapsSurrounding, 4)));
        let plug = px(&doc, 0, 50, 50);
        let flat = px(&doc, 0, 45, 45);
        assert_eq!(plug[3], ONE as u16, "the pinhole is opaque now");
        for c in 0..3 {
            assert!(
                (plug[c] as i32 - flat[c] as i32).abs() <= 2,
                "and it took the flat's own colour: {plug:?} vs {flat:?}"
            );
        }
        assert_eq!(px(&doc, 0, 61, 61)[3], ONE as u16, "the 4 px hole too");
        assert_eq!(
            px(&doc, 0, 75, 75)[3],
            0,
            "the 100 px window is a window, not dust"
        );
        assert_eq!(
            px(&doc, 0, 10, 10)[3],
            0,
            "and the page around the flat is still empty"
        );
    }

    /// RD-003 mode 4: same holes, the drawing colour instead.
    #[test]
    fn gaps_can_take_the_drawing_colour_instead() {
        let mut doc = Document::new(128, 128);
        rect(&mut doc, 0, 20, 20, 60, 60, [0.2, 0.4, 0.8]);
        clear(&mut doc, 0, 30, 30);
        assert!(doc.apply_filter(dust(DustMode::GapsForeground, 4)));
        let plug = px(&doc, 0, 30, 30);
        assert_eq!(plug[3], ONE as u16);
        assert!(
            plug[0] > 32000 && plug[1] < 100 && plug[2] < 100,
            "the red the tool was given, not the blue around it: {plug:?}"
        );
    }

    /// RD-003 mode 2: on a white background a speck is anything DARKER than
    /// the paper, and cleaning it means repainting paper — not punching a
    /// transparent hole in the scan.
    #[test]
    fn white_background_dust_is_repainted_white() {
        let mut doc = Document::new(128, 128);
        rect(&mut doc, 0, 0, 0, 128, 128, [1.0, 1.0, 1.0]);
        dot(&mut doc, 0, 30, 30, [0.1, 0.1, 0.1]); // a speck
        rect(&mut doc, 0, 60, 60, 70, 70, [0.0, 0.0, 0.0]); // real ink
        assert!(doc.apply_filter(dust(DustMode::OnWhite, 4)));
        assert_eq!(
            px(&doc, 0, 30, 30),
            [ONE as u16; 4],
            "the speck is paper again — opaque white, not a hole"
        );
        assert_eq!(px(&doc, 0, 65, 65)[3], ONE as u16);
        assert!(px(&doc, 0, 65, 65)[0] < 100, "the drawing is untouched");
        // The white-on-transparency case: the same speck on an EMPTY page
        // is invisible to this mode, because transparency reads as paper.
        let mut doc = Document::new(128, 128);
        dot(&mut doc, 0, 30, 30, [1.0, 1.0, 1.0]);
        doc.apply_filter(dust(DustMode::OnWhite, 4));
        assert_eq!(
            px(&doc, 0, 30, 30),
            [ONE as u16; 4],
            "a white speck on nothing is not darker than the paper"
        );
    }

    /// The selection is the tool's window: dust outside it is left alone,
    /// which is what makes RD-001 a DRAG rather than a page-wide filter.
    #[test]
    fn the_window_limits_the_bite() {
        let mut doc = speck_page();
        // The window is smaller than one tile, so the 4 px speck at (40,40)
        // sits in a tile the filter WRITES and outside the coverage that
        // survives it — the `mask_op_to_selection` restore is the thing
        // under test as much as the detection is.
        doc.selection = Some(Selection::from_rect(&doc, 0.0, 0.0, 30.0, 30.0));
        assert!(doc.apply_filter(dust(DustMode::OnTransparency, 9)));
        assert!(!inked(&doc, 20, 20), "inside the window the speck goes");
        assert!(inked(&doc, 40, 40), "the same tile, outside the window: kept");
        assert!(inked(&doc, 60, 60), "another tile entirely: untouched");
    }

    /// One invocation, ONE undo press — and the page comes back exactly.
    #[test]
    fn one_scrub_is_one_undo_press() {
        let mut doc = speck_page();
        let before = crate::export::composite(&doc, crate::export::Background::White);
        assert!(doc.apply_filter(dust(DustMode::OnTransparency, 9)));
        assert!(!inked(&doc, 20, 20) && !inked(&doc, 40, 40) && !inked(&doc, 62, 62));
        assert!(doc.undo(), "one press");
        let after = crate::export::composite(&doc, crate::export::Background::White);
        assert!(
            before.pixels().zip(after.pixels()).all(|(a, b)| a.0 == b.0),
            "all three specks are back after a single undo"
        );
        assert!(!doc.undo(), "and there was only ever the one op");
    }

    /// Guards: the fill family's `paintable()` rule, and an empty layer that
    /// records nothing at all.
    #[test]
    fn guards_refuse_without_recording() {
        let mut doc = Document::new(128, 128);
        assert!(
            !doc.apply_filter(dust(DustMode::OnTransparency, 4)),
            "an empty layer is a no-op"
        );
        assert!(!doc.undo(), "and left nothing on the undo stack");

        let mut doc = speck_page();
        doc.layers[0].lock = true;
        assert!(!doc.apply_filter(dust(DustMode::OnTransparency, 9)));
        assert!(inked(&doc, 20, 20), "a locked layer keeps its dust");
        doc.layers[0].lock = false;
        doc.layers[0].folder = true;
        assert!(
            !doc.apply_filter(dust(DustMode::OnTransparency, 9)),
            "a folder is not paintable"
        );
        doc.layers[0].folder = false;
        doc.layers[0].kind =
            crate::doc::LayerKind::Fill(crate::fill_layer::FillKind::Flat { color: [0.0; 4] });
        assert!(
            !doc.apply_filter(dust(DustMode::OnTransparency, 9)),
            "a LIVE layer's raster is derived — it refuses like a fill does"
        );
    }

    /// RD-007: the SELECT half finds the same pixels and paints none of
    /// them. The count is pinned — 1 + 4 + 9 px of speck, nothing else.
    #[test]
    fn select_dust_finds_the_specks_and_touches_nothing() {
        let doc = speck_page();
        let sel = doc
            .dust_selection(DustMode::OnTransparency, 9)
            .expect("three specks");
        let mut n = 0;
        for y in 0..256 {
            for x in 0..256 {
                if selected(sel.coverage(x, y)) {
                    n += 1;
                }
            }
        }
        assert_eq!(n, 1 + 4 + 9, "exactly the three specks, not the drawing");
        assert!(selected(sel.coverage(20, 20)));
        assert!(!selected(sel.coverage(110, 110)));
        assert!(inked(&doc, 20, 20), "and nothing was erased");
    }

    /// RD-009 select-side, gaps variant — and the window applies here too.
    #[test]
    fn select_dust_windows_and_finds_gaps() {
        let mut doc = Document::new(256, 256);
        rect(&mut doc, 0, 40, 40, 100, 100, [0.0, 0.0, 0.0]);
        clear(&mut doc, 0, 50, 50);
        clear(&mut doc, 0, 90, 90);
        let sel = doc
            .dust_selection(DustMode::GapsSurrounding, 4)
            .expect("two pinholes");
        assert!(selected(sel.coverage(50, 50)) && selected(sel.coverage(90, 90)));
        assert!(!selected(sel.coverage(70, 70)), "solid ink is not a gap");

        doc.selection = Some(Selection::from_rect(&doc, 0.0, 0.0, 70.0, 70.0));
        let sel = doc.dust_selection(DustMode::GapsSurrounding, 4).unwrap();
        assert!(selected(sel.coverage(50, 50)));
        assert!(!selected(sel.coverage(90, 90)), "outside the window");
    }

    /// The cost claim: a component far bigger than the threshold is never
    /// walked whole. A page-filling drawing with one speck in it must scrub
    /// in time proportional to the buffer, not to the drawing — the
    /// observable proxy is that it finishes at all with a 256 px threshold
    /// on a canvas whose ink is 250 000 px in one piece.
    #[test]
    fn a_page_of_solid_ink_costs_no_more_than_the_page() {
        let mut doc = Document::new(512, 512);
        rect(&mut doc, 0, 0, 0, 500, 500, [0.0, 0.0, 0.0]);
        clear(&mut doc, 0, 250, 250);
        let t = std::time::Instant::now();
        assert!(doc.apply_filter(dust(DustMode::GapsSurrounding, 256)));
        assert_eq!(px(&doc, 0, 250, 250)[3], ONE as u16, "the pinhole plugged");
        assert!(inked(&doc, 100, 100), "the drawing survived");
        // Generous — this is a shape assertion (linear, not quadratic), and
        // debug builds are slow. A per-speck flood of the 250 000 px blob
        // would be minutes.
        assert!(
            t.elapsed().as_secs() < 30,
            "the component pass stayed linear: {:?}",
            t.elapsed()
        );
    }
}
