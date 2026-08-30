//! `I-005` sampling kernels, and the whole-tile-map resample that shares
//! them — the sampler half of [`super`], moved here verbatim when
//! `transform.rs` was split. [`Interp`] and [`resample_tile_map`] keep their
//! old paths (`mn_core::transform::…`) through re-exports in the parent.

use std::collections::HashMap;
use std::sync::Arc;

use crate::tile::{FIX15_ONE, TILE_SIZE, Tile, TileIdx};

/// Read one premultiplied fix15 pixel out of a sparse tile map (missing tile =
/// transparent).
#[inline]
pub(super) fn sample_px(tiles: &HashMap<TileIdx, Arc<Tile>>, x: i32, y: i32) -> [f32; 4] {
    let ti = TileIdx::of_pixel(x, y);
    let Some(tile) = tiles.get(&ti) else {
        return [0.0; 4];
    };
    let lx = (x - ti.x * TILE_SIZE as i32) as usize;
    let ly = (y - ti.y * TILE_SIZE as i32) as usize;
    let p = tile.pixel(lx, ly);
    [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]
}

/// Bilinear sample at a fractional source position (premultiplied fix15, so
/// interpolation at alpha edges is correct without unpremultiplying).
pub(super) fn sample_bilinear(
    tiles: &HashMap<TileIdx, Arc<Tile>>,
    rect: [i32; 4],
    x: f32,
    y: f32,
) -> [f32; 4] {
    // Outside the source rect (with a 1px filter apron) contributes nothing.
    if x < rect[0] as f32 - 1.0
        || y < rect[1] as f32 - 1.0
        || x >= rect[2] as f32
        || y >= rect[3] as f32
    {
        return [0.0; 4];
    }
    let x0 = x.floor();
    let y0 = y.floor();
    let fx = x - x0;
    let fy = y - y0;
    let (x0, y0) = (x0 as i32, y0 as i32);
    let clamp = |px: i32, py: i32| -> [f32; 4] {
        // The rect edge acts as transparent, not clamped — content ends there.
        if px < rect[0] || py < rect[1] || px >= rect[2] || py >= rect[3] {
            [0.0; 4]
        } else {
            sample_px(tiles, px, py)
        }
    };
    let p00 = clamp(x0, y0);
    let p10 = clamp(x0 + 1, y0);
    let p01 = clamp(x0, y0 + 1);
    let p11 = clamp(x0 + 1, y0 + 1);
    let mut out = [0.0f32; 4];
    for c in 0..4 {
        let top = p00[c] * (1.0 - fx) + p10[c] * fx;
        let bot = p01[c] * (1.0 - fx) + p11[c] * fx;
        out[c] = top * (1.0 - fy) + bot * fy;
    }
    out
}

/// `I-005` — CSP Tool Settings ▸ Image settings ▸ **Interpolation method**:
/// which kernel a scaling commit resamples with.
///
/// # Why this is a manga row and not a graphics-nerd row
///
/// Shrink a page and the first thing that dies is the thinnest line on it.
/// [`Self::Bilinear`] reads a 2×2 neighbourhood around the destination
/// pixel's centre; at a 0.4× shrink most 1 px hairlines fall BETWEEN the
/// sampled centres and simply are not there any more — not faint, gone, and
/// gone in a way that reads as a broken line rather than a light one.
/// [`Self::HighAccuracy`] averages the whole area a destination pixel covers,
/// so every source pixel contributes its share and a hairline comes through
/// grey instead of absent. That is CSP's 高精度 (「平均色」), and the reason
/// its own manual singles it out for reduction.
///
/// `a_hairline_survives_the_high_accuracy_shrink` pins the difference with
/// counted lines rather than a feeling.
///
/// # What is NOT here
///
/// CSP's fifth entry, **Smooth (oversampling)**, is deferred: it is a
/// multi-sample refinement of the same box this already integrates exactly,
/// and CSP itself forbids it on image-material layers. Nothing else in the
/// tree can express it either.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Interp {
    /// CSP **Hard edges** — nearest neighbour. No new colours at all, so a
    /// 1-bit page stays 1-bit and pixel art stays crisp. Everything else it
    /// touches gets stair-stepped.
    Nearest,
    /// CSP **Smooth edges** — bilinear. THE DEFAULT, and byte-for-byte the
    /// only kernel this module had before the row existed: an artist who
    /// never opens the dropdown must get the pixels they got yesterday.
    #[default]
    Bilinear,
    /// CSP **Clear edges** — bicubic (Catmull-Rom). Keeps more edge contrast
    /// than bilinear when ENLARGING; on a shrink it rings, which is why it
    /// is not the manga answer.
    Bicubic,
    /// CSP **High accuracy (average colors)** — the exact box area average.
    /// The reduction kernel. On an enlargement the box is smaller than a
    /// pixel and there is nothing to average, so it falls through to
    /// bilinear rather than degenerating into Nearest.
    HighAccuracy,
}

impl Interp {
    pub fn label(self) -> &'static str {
        match self {
            Interp::Nearest => "Hard edges",
            Interp::Bilinear => "Smooth edges",
            Interp::Bicubic => "Clear edges",
            Interp::HighAccuracy => "High accuracy",
        }
    }

    pub const ALL: [Interp; 4] = [
        Interp::Bilinear,
        Interp::Nearest,
        Interp::Bicubic,
        Interp::HighAccuracy,
    ];
}

/// Nearest neighbour. `x`/`y` are in pixel-INDEX space (integer = the pixel's
/// centre), the same convention `sample_bilinear` takes.
pub(super) fn sample_nearest(
    tiles: &HashMap<TileIdx, Arc<Tile>>,
    rect: [i32; 4],
    x: f32,
    y: f32,
) -> [f32; 4] {
    let (px, py) = (x.round() as i32, y.round() as i32);
    if px < rect[0] || py < rect[1] || px >= rect[2] || py >= rect[3] {
        return [0.0; 4];
    }
    sample_px(tiles, px, py)
}

/// Catmull-Rom weights for the four taps around a fractional offset.
#[inline]
fn catrom(t: f32) -> [f32; 4] {
    let t2 = t * t;
    let t3 = t2 * t;
    [
        0.5 * (-t3 + 2.0 * t2 - t),
        0.5 * (3.0 * t3 - 5.0 * t2 + 2.0),
        0.5 * (-3.0 * t3 + 4.0 * t2 + t),
        0.5 * (t3 - t2),
    ]
}

/// Bicubic (Catmull-Rom) on premultiplied fix15.
///
/// The negative lobes are the point of the filter and also its hazard: they
/// can push a channel below zero or above its own alpha, and a premultiplied
/// pixel whose colour exceeds its alpha composites as a bright fringe. So the
/// result is clamped back into the premultiplied invariant (`0 <= c <= a`)
/// before it leaves — ringing is allowed to sharpen an edge, not to invent
/// impossible pixels.
pub(super) fn sample_bicubic(
    tiles: &HashMap<TileIdx, Arc<Tile>>,
    rect: [i32; 4],
    x: f32,
    y: f32,
) -> [f32; 4] {
    if x < rect[0] as f32 - 2.0
        || y < rect[1] as f32 - 2.0
        || x >= rect[2] as f32 + 1.0
        || y >= rect[3] as f32 + 1.0
    {
        return [0.0; 4];
    }
    let x0 = x.floor();
    let y0 = y.floor();
    let (wx, wy) = (catrom(x - x0), catrom(y - y0));
    let (x0, y0) = (x0 as i32, y0 as i32);
    let mut out = [0.0f32; 4];
    for (j, wj) in wy.iter().enumerate() {
        let py = y0 - 1 + j as i32;
        for (i, wi) in wx.iter().enumerate() {
            let px = x0 - 1 + i as i32;
            // The rect edge acts as transparent, exactly as bilinear treats
            // it — content ends there, it does not smear outward.
            if px < rect[0] || py < rect[1] || px >= rect[2] || py >= rect[3] {
                continue;
            }
            let p = sample_px(tiles, px, py);
            let w = wi * wj;
            for c in 0..4 {
                out[c] += p[c] * w;
            }
        }
    }
    let a = out[3].clamp(0.0, FIX15_ONE as f32);
    [
        out[0].clamp(0.0, a),
        out[1].clamp(0.0, a),
        out[2].clamp(0.0, a),
        a,
    ]
}

/// `I-005` High accuracy: the exact box area average over the source
/// footprint of one destination pixel, `(x±hx, y±hy)` in pixel-index space.
///
/// Fractional edge weights, computed per destination pixel straight off the
/// tiles — the same shape as `export::comic_downscale`, and for the same
/// reason: no full-canvas float buffer exists to be reused, and a page-sized
/// one is exactly what this module refuses to allocate.
///
/// Source pixels OUTSIDE the lifted rect count toward the area but
/// contribute no colour, which is what makes the float's own edge fade out
/// instead of stopping dead a pixel early.
pub(super) fn sample_area(
    tiles: &HashMap<TileIdx, Arc<Tile>>,
    rect: [i32; 4],
    x: f32,
    y: f32,
    hx: f32,
    hy: f32,
) -> [f32; 4] {
    let (bx0, bx1) = (x - hx, x + hx);
    let (by0, by1) = (y - hy, y + hy);
    // Wholly outside the source? Nothing to integrate.
    if bx1 < rect[0] as f32 - 0.5
        || by1 < rect[1] as f32 - 0.5
        || bx0 >= rect[2] as f32 - 0.5
        || by0 >= rect[3] as f32 - 0.5
    {
        return [0.0; 4];
    }
    // Pixel k covers [k-0.5, k+0.5) in index space.
    let ix0 = (bx0 + 0.5).floor() as i32;
    let ix1 = (bx1 + 0.5).ceil() as i32;
    let iy0 = (by0 + 0.5).floor() as i32;
    let iy1 = (by1 + 0.5).ceil() as i32;
    let mut acc = [0.0f32; 4];
    let mut area = 0.0f32;
    for py in iy0..iy1 {
        let wy = ((py as f32 + 0.5).min(by1) - (py as f32 - 0.5).max(by0)).max(0.0);
        if wy <= 0.0 {
            continue;
        }
        for px in ix0..ix1 {
            let wx = ((px as f32 + 0.5).min(bx1) - (px as f32 - 0.5).max(bx0)).max(0.0);
            if wx <= 0.0 {
                continue;
            }
            let w = wx * wy;
            area += w;
            if px < rect[0] || py < rect[1] || px >= rect[2] || py >= rect[3] {
                continue; // outside the float: transparent, but it took space
            }
            let p = sample_px(tiles, px, py);
            for c in 0..4 {
                acc[c] += p[c] * w;
            }
        }
    }
    if area <= 0.0 {
        return [0.0; 4];
    }
    [
        acc[0] / area,
        acc[1] / area,
        acc[2] / area,
        acc[3] / area,
    ]
}

/// Resample a WHOLE sparse tile map about the canvas origin — the raster
/// half of `IO-060` (Edit ▸ Change work resolution).
///
/// Unlike [`commit_transform`], which scatters a lifted float back onto one
/// layer of a fixed-size document, this rebuilds an entire tile map at a new
/// scale and is bounded by the CONTENT, not by the canvas: art parked
/// off-page (a sketch in the margin, a balloon half outside the trim) scales
/// with everything else instead of being clipped away. The caller trims to
/// the new canvas if it wants to.
///
/// The kernel is `I-005`'s ([`Interp`]), reached through the same samplers
/// the Transform tool uses, so a work resample and a hand-scaled selection
/// cannot drift apart. [`Interp::HighAccuracy`] is the one that matters
/// here: a page shrunk from 600 to 350 dpi is exactly the "do not lose the
/// 1 px hairline" case the area kernel exists for.
///
/// The scan is per DESTINATION tile, and a destination tile whose source
/// footprint holds no tile at all is skipped before a single pixel is
/// sampled — a page is mostly empty tiles, and without that test this walks
/// the full bounding box of every layer.
pub fn resample_tile_map(
    tiles: &HashMap<TileIdx, Arc<Tile>>,
    sx: f32,
    sy: f32,
    interp: Interp,
) -> HashMap<TileIdx, Arc<Tile>> {
    let mut out: HashMap<TileIdx, Arc<Tile>> = HashMap::new();
    if tiles.is_empty() || !(sx > 0.0) || !(sy > 0.0) {
        return out;
    }
    // Source bounds in canvas px, from the populated tiles.
    let ts = TILE_SIZE as i32;
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for ti in tiles.keys() {
        let (ox, oy) = ti.origin();
        x0 = x0.min(ox);
        y0 = y0.min(oy);
        x1 = x1.max(ox + ts);
        y1 = y1.max(oy + ts);
    }
    let rect = [x0, y0, x1, y1];

    // The source-space half-extent of one destination pixel (the scale is
    // axis-aligned here, so this is exact rather than the affine's bound).
    let hx = 0.5 / sx;
    let hy = 0.5 / sy;
    let area_mode = interp == Interp::HighAccuracy && (hx > 0.5 || hy > 0.5);

    let dst = [
        (x0 as f32 * sx).floor() as i32,
        (y0 as f32 * sy).floor() as i32,
        (x1 as f32 * sx).ceil() as i32,
        (y1 as f32 * sy).ceil() as i32,
    ];
    let (tx0, ty0) = (dst[0].div_euclid(ts), dst[1].div_euclid(ts));
    let (tx1, ty1) = ((dst[2] - 1).div_euclid(ts), (dst[3] - 1).div_euclid(ts));
    // The filter apron, in source px — bicubic reaches two pixels out.
    let apron = if interp == Interp::Bicubic { 2.0 } else { 1.0 };
    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            let di = TileIdx::new(tx, ty);
            let (dox, doy) = di.origin();
            // Does any SOURCE tile feed this destination tile?
            let sfx0 = (dox as f32 / sx - hx - apron).floor() as i32;
            let sfy0 = (doy as f32 / sy - hy - apron).floor() as i32;
            let sfx1 = ((dox + ts) as f32 / sx + hx + apron).ceil() as i32;
            let sfy1 = ((doy + ts) as f32 / sy + hy + apron).ceil() as i32;
            let mut fed = false;
            'probe: for sty in sfy0.div_euclid(ts)..=(sfy1 - 1).div_euclid(ts) {
                for stx in sfx0.div_euclid(ts)..=(sfx1 - 1).div_euclid(ts) {
                    if tiles.contains_key(&TileIdx::new(stx, sty)) {
                        fed = true;
                        break 'probe;
                    }
                }
            }
            if !fed {
                continue;
            }
            let mut tile = Tile::default();
            let mut any = false;
            for ly in 0..TILE_SIZE {
                let cy = doy + ly as i32;
                if cy < dst[1] || cy >= dst[3] {
                    continue;
                }
                let syf = (cy as f32 + 0.5) / sy - 0.5;
                for lx in 0..TILE_SIZE {
                    let cx = dox + lx as i32;
                    if cx < dst[0] || cx >= dst[2] {
                        continue;
                    }
                    let sxf = (cx as f32 + 0.5) / sx - 0.5;
                    let px = match interp {
                        Interp::Nearest => sample_nearest(tiles, rect, sxf, syf),
                        Interp::Bicubic => sample_bicubic(tiles, rect, sxf, syf),
                        Interp::HighAccuracy if area_mode => {
                            sample_area(tiles, rect, sxf, syf, hx, hy)
                        }
                        _ => sample_bilinear(tiles, rect, sxf, syf),
                    };
                    if px[3] < 0.5 {
                        continue;
                    }
                    let a = px[3].round().min(FIX15_ONE as f32) as u16;
                    tile.set_pixel(
                        lx,
                        ly,
                        [
                            (px[0].round().min(FIX15_ONE as f32) as u16).min(a),
                            (px[1].round().min(FIX15_ONE as f32) as u16).min(a),
                            (px[2].round().min(FIX15_ONE as f32) as u16).min(a),
                            a,
                        ],
                    );
                    any = true;
                }
            }
            if any {
                out.insert(di, Arc::new(tile));
            }
        }
    }
    out
}
