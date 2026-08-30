//! Line correction — LC-001 remove dust and LC-002 adjust line width (the
//! morphology pair). Moved here verbatim when `filter.rs` was split;
//! dispatch and the halo declaration stay in [`super::Filter`].

use super::Raster;
use crate::tile::TILE_CHANNELS;

// -------------------------------------------------------- line correction --

/// The speck size a dust removal is allowed to look for, in pixels of area.
/// Shared by [`super::Filter::reach`] and [`remove_dust`] so the halo and the count
/// can never disagree; the ceiling bounds the halo the same way `MAX_SIGMA`
/// bounds the blur's.
pub(crate) fn dust_max(max_px: u32) -> u32 {
    max_px.clamp(1, 256)
}

/// LC-001: clear every 8-connected blob of `max_px` pixels or fewer.
///
/// 8-connected, not 4-: a scanner speck is as often a diagonal pair as a
/// square one, and under 4-connectivity a four-pixel diagonal reads as four
/// separate one-pixel specks — which would delete a chain the eye sees as one
/// mark, at a threshold the user set to keep it.
///
/// Anything with ink at all counts, at any alpha: dust is usually the faint
/// grey the scanner invented, and thresholding would keep exactly the specks
/// worth removing. The flood is iterative (a page of ink is millions of
/// pixels deep for a recursive one) and stops RECORDING a blob's pixels once
/// it is too big to clear, so the scratch stays bounded by `max_px` rather
/// than by the largest connected drawing on the layer.
pub(super) fn remove_dust(buf: &mut Raster, max_px: u32) {
    let max = dust_max(max_px) as usize;
    let (w, h) = (buf.w, buf.h);
    if w == 0 || h == 0 {
        return;
    }
    let inked = |b: &Raster, p: usize| b.px[p * TILE_CHANNELS + 3] != 0;
    let mut seen = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    let mut speck: Vec<usize> = Vec::new();
    for start in 0..w * h {
        if seen[start] || !inked(buf, start) {
            continue;
        }
        seen[start] = true;
        stack.clear();
        speck.clear();
        stack.push(start);
        let mut count = 0usize;
        while let Some(p) = stack.pop() {
            count += 1;
            if count <= max {
                speck.push(p);
            }
            let (x, y) = ((p % w) as i32, (p / w) as i32);
            for ny in (y - 1).max(0)..=(y + 1).min(h as i32 - 1) {
                for nx in (x - 1).max(0)..=(x + 1).min(w as i32 - 1) {
                    let q = ny as usize * w + nx as usize;
                    if seen[q] || !inked(buf, q) {
                        continue;
                    }
                    seen[q] = true;
                    stack.push(q);
                }
            }
        }
        if count <= max {
            for &p in &speck {
                buf.px[p * TILE_CHANNELS..(p + 1) * TILE_CHANNELS].fill(0);
            }
        }
    }
}

/// How far a line-width adjustment reaches, in pixels. Shared by
/// [`super::Filter::reach`], [`super::Filter::is_identity`] and [`line_width`].
pub(super) fn line_width_radius(delta: i32) -> usize {
    delta.clamp(-64, 64).unsigned_abs() as usize
}

/// One separable pass of a square-ball greyscale morphology along one axis.
///
/// Square ball = Chebyshev ball, and a Chebyshev ball is SEPARABLE — one
/// horizontal pass then one vertical one, so any radius costs two passes
/// instead of `r` rounds of a 3×3. That is `Selection::grow`'s trick; what is
/// different here is the operator. `grow` runs on a boolean mask and can
/// answer its windows from a prefix sum, and this cannot: thresholding the
/// alpha to a mask would throw away the anti-aliasing on every line in the
/// drawing, which for a tool whose whole job is line quality is the one
/// unacceptable outcome. So the window extremum comes from a monotonic deque
/// instead — every index enters and leaves once, still O(1) per pixel.
///
/// The winner's WHOLE premultiplied pixel travels, not just its alpha:
/// thickening a line has to bring the line's colour out with it, and thinning
/// one has to leave the thinned edge the colour it was, not black. Ties keep
/// the centre pixel, which is what stops a flat region of two colours — where
/// every alpha is equal — from swapping one for the other.
fn morph_pass(src: &Raster, dst: &mut Raster, r: usize, vertical: bool, grow: bool) {
    let (outer, inner) = if vertical { (src.w, src.h) } else { (src.h, src.w) };
    let mut dq: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    for o in 0..outer {
        let at = |i: usize| {
            if vertical {
                src.pixel(o, i)
            } else {
                src.pixel(i, o)
            }
        };
        dq.clear();
        let mut next = 0usize;
        for i in 0..inner {
            // Admit everything the window at `i` newly covers…
            let hi = (i + r).min(inner - 1);
            while next <= hi {
                let a = at(next)[3];
                while dq
                    .back()
                    .is_some_and(|&b| if grow { at(b)[3] <= a } else { at(b)[3] >= a })
                {
                    dq.pop_back();
                }
                dq.push_back(next);
                next += 1;
            }
            // …and retire what it has left behind. The front is the extremum.
            while dq.front().is_some_and(|&f| f + r < i) {
                dq.pop_front();
            }
            let own = at(i);
            let win = at(*dq.front().expect("the window always holds `i`"));
            let take = if grow {
                win[3] > own[3]
            } else {
                win[3] < own[3]
            };
            let p = if take { win } else { own };
            if vertical {
                dst.set_pixel(o, i, p);
            } else {
                dst.set_pixel(i, o, p);
            }
        }
    }
}

/// LC-002: thicken (`delta > 0`) or thin (`delta < 0`) the ink by `delta`
/// pixels — a signed square-ball dilation of the coverage, run as two
/// [`morph_pass`]es.
pub(super) fn line_width(buf: &mut Raster, delta: i32) {
    let r = line_width_radius(delta);
    if r == 0 {
        return;
    }
    let grow = delta > 0;
    let mut tmp = Raster::new(buf.w, buf.h);
    morph_pass(buf, &mut tmp, r, false, grow);
    std::mem::swap(buf, &mut tmp);
    morph_pass(buf, &mut tmp, r, true, grow);
    std::mem::swap(buf, &mut tmp);
}
