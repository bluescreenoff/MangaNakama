//! Border effect (CSP 境界効果 ▸ フチ, `LP-002`/`LP-003`) — a per-layer
//! outline, and the exact distance transform it shares with the text engine.
//!
//! The want, in the owner's words: *the white keyline around a character
//! sitting on a tone* — today only reachable by hand-inking it. So: take the
//! layer's OWN alpha, grow it by N px, paint that ring in a chosen colour and
//! put the layer's pixels back on top. Nothing is baked; the painted pixels
//! never change, and turning the effect off restores the drawing exactly.
//!
//! ## One distance transform, not two
//!
//! `mn_text` already had an exact Euclidean distance transform written for
//! text フチ (Felzenszwalb/Huttenlocher — two 1-D passes, O(n), round joins
//! for free). It lives HERE now and the text engine calls it, so the outline
//! a text layer draws and the outline this effect draws are the same
//! geometry and cannot drift apart.
//!
//! ## Why the effect is a derived raster and not a shader
//!
//! A dilation is not pointwise: a stroke one pixel inside a tile edge throws
//! outline pixels into the NEIGHBOURING tile, which may hold no source
//! pixels at all. The compositors are pointwise per tile, so the growth
//! happens once, into a derived tile set (`Layer::refresh_edge`, the
//! screentone model), and both compositors just draw what they are given.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::doc::Document;
use crate::tile::{TILE_PIXELS, TILE_SIZE, Tile, TileIdx};

/// "Unreachably far" seed value for [`dist_sq`]. Not `f32::INFINITY`: the
/// 1-D pass subtracts two of these, and `inf - inf` is NaN.
pub const INF: f32 = 1e12;

/// The widest outline the effect will grow, in canvas pixels. A cap exists
/// because the derived tile set dilates by whole TILES — every extra 64 px
/// of reach is another ring of tiles to derive and upload.
pub const WIDTH_MAX: f32 = 32.0;

/// Border-effect parameters (`LP-003`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct EdgeParams {
    /// Outline thickness in canvas pixels (0..=[`WIDTH_MAX`]).
    pub width_px: f32,
    /// Outline colour, straight RGB. Drawn at full alpha under the art.
    pub colour: [u8; 3],
    /// LP-004 (CSP 水彩境界): `Solid` = the keyline above; `Watercolour`
    /// = a pale stain rim whose colour is DERIVED from the layer's own
    /// nearest ink (the picked colour is ignored). Absent on every file
    /// written before the field existed = Solid, byte-for-byte.
    #[serde(default)]
    pub style: EdgeStyle,
}

/// What the border effect draws (see [`EdgeParams::style`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum EdgeStyle {
    #[default]
    Solid,
    Watercolour,
}

impl Default for EdgeParams {
    /// White, 3 px — the manga keyline, which is the reason this exists.
    fn default() -> Self {
        Self {
            width_px: 3.0,
            colour: [255, 255, 255],
            style: EdgeStyle::Solid,
        }
    }
}

impl EdgeParams {
    /// The clamped thickness the raster actually uses.
    pub fn width(self) -> f32 {
        self.width_px.clamp(0.0, WIDTH_MAX)
    }

    /// How far, in whole pixels, the outline can reach out of an inked pixel.
    pub fn reach(self) -> usize {
        self.width().ceil() as usize
    }

    /// Bit-exact signature for the GPU's per-layer presentation hash.
    ///
    /// Same contract as `ToneParams::sig` and for the same reason: the GPU
    /// re-uploads a tile when its revision moves, and an effect parameter
    /// that is not in this hash means the canvas keeps showing the old
    /// raster. Guarded by `sig_covers_every_field` below — widen both
    /// together or the canvas silently lies.
    pub fn sig(self) -> [u32; 2] {
        [
            self.width_px.to_bits(),
            (self.colour[0] as u32) << 16
                | (self.colour[1] as u32) << 8
                | self.colour[2] as u32
                | ((self.style as u32) << 28),
        ]
    }
}

/// In-place exact squared Euclidean distance transform (Felzenszwalb &
/// Huttenlocher 2012): two 1-D lower-envelope passes over a `w × h` field.
///
/// Input: `0.0` at every "inked" sample, [`INF`] everywhere else. Output:
/// the SQUARED distance to the nearest inked sample. Take `sqrt` at the end,
/// once, where you need the real distance.
pub fn dist_sq(f: &mut [f32], w: usize, h: usize) {
    debug_assert_eq!(f.len(), w * h);
    if w == 0 || h == 0 {
        return;
    }
    let mut row = vec![0f32; w.max(h)];
    let mut tmp = Vec::new();
    for y in 0..h {
        row[..w].copy_from_slice(&f[y * w..y * w + w]);
        edt_1d(&row[..w], &mut tmp);
        f[y * w..y * w + w].copy_from_slice(&tmp);
    }
    for x in 0..w {
        for y in 0..h {
            row[y] = f[y * w + x];
        }
        edt_1d(&row[..h], &mut tmp);
        for y in 0..h {
            f[y * w + x] = tmp[y];
        }
    }
}

/// 1-D squared EDT: the lower envelope of the parabolas rooted at each sample.
fn edt_1d(f: &[f32], out: &mut Vec<f32>) {
    let n = f.len();
    out.clear();
    out.resize(n, 0.0);
    if n == 0 {
        return;
    }
    let mut v = vec![0usize; n];
    let mut z = vec![0f32; n + 1];
    let mut k = 0usize;
    v[0] = 0;
    z[0] = -INF;
    z[1] = INF;
    for q in 1..n {
        loop {
            let s = ((f[q] + (q * q) as f32) - (f[v[k]] + (v[k] * v[k]) as f32))
                / (2.0 * (q as f32 - v[k] as f32));
            if s <= z[k] {
                if k == 0 {
                    break;
                }
                k -= 1;
            } else {
                k += 1;
                v[k] = q;
                z[k] = s;
                z[k + 1] = INF;
                break;
            }
        }
    }
    k = 0;
    for q in 0..n {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let dq = q as f32 - v[k] as f32;
        out[q] = dq * dq + f[v[k]];
    }
}

/// Alpha at or above which a source pixel seeds the distance transform
/// (fix15 half). The text engine's フチ uses the same 50 % rule.
pub const INK_ALPHA: u16 = 16384;

/// Derive ONE displayed tile: the outline ring with the source pixels
/// composited back over it.
///
/// `seed` is the padded alpha field around the tile — side `TILE_SIZE + 2r`,
/// row-major, `0.0` where the source is inked and [`INF`] elsewhere — and is
/// consumed (turned into squared distances). Passing the scratch buffer in
/// lets the caller reuse one allocation across a whole layer.
///
/// `cwin` is the same window's PREMULTIPLIED fix15 pixels (side²×4, zero
/// where nothing inked), required only by [`EdgeStyle::Watercolour`]'s
/// colour sampling — pass an empty slice for the solid style.
pub fn derive_tile(
    seed: &mut [f32],
    r: usize,
    src: Option<&Tile>,
    p: EdgeParams,
    cwin: &[u16],
) -> Tile {
    let side = TILE_SIZE + 2 * r;
    let w = p.width();
    // Zero width is a real OFF, not a hairline. Without this the half-pixel
    // edge convention below still puts HALF coverage on the ink's own pixel,
    // so dragging the width bar to 0 would tint the drawing instead of
    // clearing the effect. Also skips the transform, which is the expensive
    // part.
    if w <= 0.0 {
        let mut out = Tile::new_transparent();
        if let Some(t) = src {
            out.data_mut().copy_from_slice(t.data());
        }
        return out;
    }
    dist_sq(seed, side, side);
    let colour = [
        p.colour[0] as f32 / 255.0,
        p.colour[1] as f32 / 255.0,
        p.colour[2] as f32 / 255.0,
    ];
    let mut out = Tile::new_transparent();
    let d = out.data_mut();
    for y in 0..TILE_SIZE {
        for x in 0..TILE_SIZE {
            // The boundary sits AT `w` px from the ink and is antialiased
            // across the one pixel around it: fully covered out to w-0.5,
            // half at exactly w, gone by w+0.5. Same convention (and the
            // same line) as the text engine's フチ — the two outlines must
            // read identically at the same number.
            let oa = (w + 0.5 - seed[(y + r) * side + (x + r)].sqrt()).clamp(0.0, 1.0);
            let s = src.map(|t| t.pixel(x, y)).unwrap_or([0; 4]);
            if oa <= 0.0 {
                if s[3] > 0 || s[0] > 0 || s[1] > 0 || s[2] > 0 {
                    let o = Tile::offset(x, y);
                    d[o..o + 4].copy_from_slice(&s);
                }
                continue;
            }
            // Watercolour (LP-004): the rim's colour comes from the layer's
            // OWN nearest ink (gradient descent on the distance field — the
            // EDT already knows the way) and lands paler than the solid
            // keyline, a stain rather than a line. The picked `colour` is
            // ignored entirely.
            let (rim_colour, rim_a) = if p.style == EdgeStyle::Watercolour {
                let stain = nearest_ink_colour(seed, cwin, side, x + r, y + r, colour);
                (stain, oa * 0.75)
            } else {
                (colour, oa)
            };
            // Source OVER outline, premultiplied — the same three lines the
            // text engine's フチ runs, in fix15 instead of 8-bit.
            let fa = s[3] as f32 / 32768.0;
            let blend = rim_a * (1.0 - fa);
            let o = Tile::offset(x, y);
            for c in 0..3 {
                d[o + c] = (s[c] as f32 + rim_colour[c] * blend * 32768.0)
                    .round()
                    .clamp(0.0, 32768.0) as u16;
            }
            d[o + 3] = ((fa + blend) * 32768.0).round().clamp(0.0, 32768.0) as u16;
        }
    }
    out
}

/// The watercolour rim's colour at one ring pixel: unpremultiplied colour
/// of the nearest inked window pixel, falling back to the picked colour
/// when the descent finds nothing (cannot happen on a real ring — the
/// field has ink somewhere within `reach` — but a derived tile must never
/// guess garbage).
fn nearest_ink_colour(
    seed: &[f32],
    cwin: &[u16],
    side: usize,
    mut x: usize,
    mut y: usize,
    fallback: [f32; 3],
) -> [f32; 3] {
    if cwin.len() < side * side * 4 {
        return fallback;
    }
    // Descend the squared-distance field: some 8-neighbour is strictly
    // smaller until we stand on ink. Bounded by the window's diagonal.
    for _ in 0..(side * side) {
        if seed[y * side + x] == 0.0 {
            break;
        }
        let mut best = seed[y * side + x];
        let (mut bx, mut by) = (x, y);
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                if nx < 0 || ny < 0 || nx >= side as i32 || ny >= side as i32 {
                    continue;
                }
                let v = seed[ny as usize * side + nx as usize];
                if v < best {
                    best = v;
                    (bx, by) = (nx as usize, ny as usize);
                }
            }
        }
        if (bx, by) == (x, y) {
            break; // local minimum that is not ink: give up, fall back
        }
        (x, y) = (bx, by);
    }
    let o = (y * side + x) * 4;
    let a = cwin[o + 3] as f32 / 32768.0;
    if a <= 0.0 {
        return fallback;
    }
    [
        cwin[o] as f32 / 32768.0 / a,
        cwin[o + 1] as f32 / 32768.0 / a,
        cwin[o + 2] as f32 / 32768.0 / a,
    ]
}

/// Brush-side watercolour edge (CSP 水彩境界, `W-001`–`005`, triage row 71):
/// the darker bleed rim CSP adds OUTSIDE a finished stroke.
///
/// Rows 28 and 71 are one look seen from two ends. Row 28 ([`EdgeStyle::
/// Watercolour`]) derives a rim from a whole LAYER's alpha and keeps it
/// non-destructive; this one derives it from ONE stroke's own coverage and
/// BAKES it, because that is what CSP does — the effect belongs to the sub
/// tool, not to the layer, and it is in the pixels the moment you lift the
/// pen. Both share [`dist_sq`] and the half-pixel boundary convention, so
/// the same number reads as the same width whichever end you set it from.
///
/// [`EdgeStyle::Watercolour`]: EdgeStyle::Watercolour
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WaterEdge {
    /// `W-001` width: how far the rim reaches outside the stroke, canvas px.
    /// **0 is the off switch** — CSP's toggle and its width slider are one
    /// knob here, because a toggle whose slider is at zero draws nothing
    /// anyway. Off is byte-exact: [`apply_stroke_rim`] returns before it
    /// reads a pixel.
    pub px: f32,
    /// `W-002` opacity 0..1. CSP: "the bigger the value is, the darker the
    /// line border becomes". 0 is a second off switch, and honestly so.
    pub opacity: f32,
    /// `W-003` darkness 0..1. CSP: "the bigger the value is, the less
    /// saturated the edge color becomes, making it look darker" — one knob,
    /// two moves; see [`rim_colour`].
    pub darkness: f32,
    /// `W-005` blurring width, canvas px: the rim's outer boundary ramps off
    /// over this distance instead of over the single antialiased pixel.
    /// 0 = the hard edge, bit-identical to the layer effect's convention.
    pub blur_px: f32,
}

impl Default for WaterEdge {
    /// Off, at CSP's own authored defaults for the rest: every stock sub
    /// tool in the owner's `EditImageTool.todb` ships
    /// `BrushUseWaterEdge = 0`, `BrushWaterEdgeAlphaPower = 20`,
    /// `BrushWaterEdgeValuePower = 0`, `BrushWaterEdgeBlur = 0`, so turning
    /// the width up alone lands on the CSP brush nobody has tuned.
    fn default() -> Self {
        Self {
            px: 0.0,
            opacity: 0.2,
            darkness: 0.0,
            blur_px: 0.0,
        }
    }
}

impl WaterEdge {
    /// Whether a stroke should run the rim pass at all.
    pub fn on(self) -> bool {
        self.px > 0.0 && self.opacity > 0.0
    }

    /// The clamped rim width, canvas px. Shares [`WIDTH_MAX`] with the layer
    /// effect: the same reach, so the same cost ceiling.
    pub fn width(self) -> f32 {
        self.px.clamp(0.0, WIDTH_MAX)
    }

    /// The clamped blur ramp, canvas px. Floored at one pixel by the caller
    /// — a zero-wide ramp IS the one-pixel antialiased boundary.
    pub fn blur(self) -> f32 {
        self.blur_px.clamp(0.0, WIDTH_MAX)
    }

    /// How far, in whole pixels, the ring reaches out of a covered pixel.
    /// Blur only softens inward, so it does not widen this.
    pub fn reach(self) -> usize {
        (self.width() + 0.5).ceil() as usize
    }
}

/// The faintest coverage that still counts as "this stroke touched here"
/// when [`apply_stroke_rim`] decides where the stroke ends and the rim
/// begins: one 8-bit alpha level. Below it the pixel is arithmetic, not ink,
/// and letting a 3e-5 fix15 crumb push the boundary outward would put a soft
/// brush's rim wherever its skirt happened to round up.
const INK_EPS: f32 = 1.0 / 255.0;

/// `W-003` Darkness, applied to the stroke's own ink colour.
///
/// CSP's wording is the spec: *"the bigger the value is, the less saturated
/// the edge color becomes, making it look darker"* — so one knob does two
/// things. Desaturate toward the colour's own Rec.709 luma (the same luma
/// the tone density source uses, `LP-008`), then take the value down, which
/// is where "looks darker" comes from: a fully desaturated rim at the SAME
/// luma would not read darker at all. 0 leaves the ink exactly as drawn.
fn rim_colour(ink: [f32; 3], darkness: f32) -> [f32; 3] {
    let k = darkness.clamp(0.0, 1.0);
    if k <= 0.0 {
        return ink;
    }
    let l = 0.2126 * ink[0] + 0.7152 * ink[1] + 0.0722 * ink[2];
    let v = 1.0 - 0.5 * k;
    [
        (ink[0] + (l - ink[0]) * k) * v,
        (ink[1] + (l - ink[1]) * k) * v,
        (ink[2] + (l - ink[2]) * k) * v,
    ]
}

/// Bake the brush-side watercolour rim onto `doc`'s active layer, from ONE
/// stroke's own coverage. Returns whether it painted anything.
///
/// `pre` is the layer's tile map as it stood before the stroke — `Arc`
/// clones taken at `StrokeSink::begin`, which cost nothing until a dab
/// actually lands (the tile path's copy-on-write does the real work). The
/// stroke's coverage is therefore `alpha now − alpha before`, clamped at
/// zero, and that clamp is load-bearing in three ways:
///
/// - a stroke laid over existing art rims only the ink IT added, not the
///   union with what was already there;
/// - an ERASER stroke drove alpha DOWN everywhere, so it rims nothing,
///   rather than fringing the hole it just made;
/// - a mask or selection stroke never touched these tiles at all, so the
///   pass is a no-op on them without needing to know they exist.
///
/// Runs at stroke end inside the app's `begin_op` bracket, so the rim is
/// part of the same undo step as the ink it belongs to. CSP's `W-004`
/// (*Process after brush stroke*) is therefore permanently on for us; it
/// exists in CSP to trade liveness for speed, and we never had the live
/// half to trade.
pub fn apply_stroke_rim(
    doc: &mut Document,
    pre: &HashMap<TileIdx, Arc<Tile>>,
    p: WaterEdge,
) -> bool {
    if !p.on() {
        return false;
    }
    let (w, r) = (p.width(), p.reach());
    let blur = p.blur().max(1.0);
    let opacity = p.opacity.clamp(0.0, 1.0);
    let ts = TILE_SIZE;

    // ---- pass 1: the stroke's own coverage, and the mean colour of its ink.
    //
    // Built for EVERY touched tile before a single rim pixel is written: the
    // ring of one tile reads the coverage of its neighbours, and a
    // half-written neighbour would have the pass rim its own rim.
    let mut cov: HashMap<TileIdx, Vec<f32>> = HashMap::new();
    let mut csum = [0f64; 3];
    let mut asum = 0f64;
    let idxs: Vec<TileIdx> = doc.active_layer().tiles().map(|(i, _)| i).collect();
    for idx in idxs {
        let Some(now) = doc.active_layer().tile(idx) else {
            continue;
        };
        let now = now.data();
        let before = pre.get(&idx).map(|t| t.data());
        let mut field = vec![0f32; TILE_PIXELS];
        let mut any = false;
        for i in 0..TILE_PIXELS {
            let a = now[i * 4 + 3] as f32;
            let b = before.map_or(0.0, |d| d[i * 4 + 3] as f32);
            let c = ((a - b) / 32768.0).clamp(0.0, 1.0);
            if c <= 0.0 {
                continue;
            }
            any = true;
            field[i] = c;
            // The rim's colour is DERIVED from the stroke's own ink, the way
            // LP-004 derives the layer effect's — CSP calls the result "a
            // faint color variation like watercolour paint", and never the
            // picked colour. Coverage-weighted mean, so the antialiased
            // skirt does not outvote the body.
            //
            // DEVIATION, deliberate: CSP resolves this per rim pixel from
            // its nearest ink; we take one colour for the whole stroke. A
            // stroke is one colour unless Colour Jitter is on, and the
            // gradient-descent sampler the layer effect uses costs a second
            // colour window per tile for a difference only jitter can show.
            let inv = if a > 0.0 { 1.0 / a } else { 0.0 };
            for (ch, s) in csum.iter_mut().enumerate() {
                *s += (now[i * 4 + ch] as f32 * inv * c) as f64;
            }
            asum += c as f64;
        }
        if any {
            cov.insert(idx, field);
        }
    }
    if asum <= 0.0 {
        return false;
    }
    let ink = [
        (csum[0] / asum) as f32,
        (csum[1] / asum) as f32,
        (csum[2] / asum) as f32,
    ];
    let rim = rim_colour(ink, p.darkness);

    // ---- pass 2: the ring. Candidates are the covered tiles grown by the
    // reach; WIDTH_MAX < TILE_SIZE, so that is at most one ring of
    // neighbours, but the arithmetic does not assume it.
    let tr = r.div_ceil(ts) as i32;
    let (ex, ey) = doc.tile_extent();
    let mut cand: Vec<TileIdx> = Vec::new();
    for idx in cov.keys() {
        for dy in -tr..=tr {
            for dx in -tr..=tr {
                let t = TileIdx::new(idx.x + dx, idx.y + dy);
                if t.x >= 0 && t.y >= 0 && t.x < ex && t.y < ey && !cand.contains(&t) {
                    cand.push(t);
                }
            }
        }
    }
    let at = |px: i32, py: i32| -> f32 {
        if px < 0 || py < 0 {
            return 0.0;
        }
        let t = TileIdx::of_pixel(px, py);
        match cov.get(&t) {
            Some(f) => {
                let (ox, oy) = t.origin();
                f[(py - oy) as usize * ts + (px - ox) as usize]
            }
            None => 0.0,
        }
    };

    let side = ts + 2 * r;
    let mut seed = vec![0f32; side * side];
    let mut writes: Vec<(usize, f32)> = Vec::new();
    let mut painted = false;
    for idx in cand {
        // Padded seed window: 0.0 on this stroke's ink, INF elsewhere.
        // "Ink" is [`INK_EPS`] of coverage, not half of it: the stroke's
        // SHAPE is what the rim traces, and a soft brush's skirt is part of
        // its shape however faint it is.
        seed.fill(INF);
        let (ox, oy) = idx.origin();
        let mut seeded = false;
        for wy in 0..side {
            let py = oy + wy as i32 - r as i32;
            for wx in 0..side {
                if at(ox + wx as i32 - r as i32, py) >= INK_EPS {
                    seed[wy * side + wx] = 0.0;
                    seeded = true;
                }
            }
        }
        if !seeded {
            continue; // nothing within reach: no ring can land here
        }
        dist_sq(&mut seed, side, side);
        writes.clear();
        for y in 0..ts {
            for x in 0..ts {
                // "Added to the OUTSIDE of the stroke" is CSP's own wording
                // and it is a hard boundary, not a blend: a pixel this
                // stroke touched at all keeps the exact bytes the dabs left
                // it. Weighting the rim by `1 - coverage` instead reads
                // plausibly at full opacity and then FILLS IN a 30 % wash
                // stroke solid, because every pixel of it is "mostly not
                // covered" — the reason this is a skip and not a factor.
                if at(ox + x as i32, oy + y as i32) >= INK_EPS {
                    continue;
                }
                let ring =
                    ((w + 0.5 - seed[(y + r) * side + (x + r)].sqrt()) / blur).clamp(0.0, 1.0);
                let a = ring * opacity;
                if a > 0.0 {
                    writes.push((Tile::offset(x, y), a));
                }
            }
        }
        if writes.is_empty() {
            continue; // do not materialise a tile the ring never reaches
        }
        painted = true;
        let d = doc.active_layer_mut().tile_mut(idx).data_mut();
        for &(o, a) in &writes {
            // Premultiplied source-over: the rim is the last thing the
            // stroke lays down, so it goes OVER what the dabs left.
            for c in 0..3 {
                d[o + c] = (rim[c] * a * 32768.0 + d[o + c] as f32 * (1.0 - a))
                    .round()
                    .clamp(0.0, 32768.0) as u16;
            }
            d[o + 3] = ((a + d[o + 3] as f32 / 32768.0 * (1.0 - a)) * 32768.0)
                .round()
                .clamp(0.0, 32768.0) as u16;
        }
    }
    painted
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The transform is EXACT: a single seed in a field must report the true
    /// squared distance, diagonals included (a chamfer approximation would be
    /// off by ~6 % on the diagonal and this is what catches that).
    #[test]
    fn distance_transform_is_exact() {
        let (w, h) = (17usize, 13usize);
        let mut f = vec![INF; w * h];
        f[6 * w + 8] = 0.0;
        dist_sq(&mut f, w, h);
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = (x as f32 - 8.0, y as f32 - 6.0);
                let want = dx * dx + dy * dy;
                assert!(
                    (f[y * w + x] - want).abs() < 1e-3,
                    "({x},{y}): {} vs {want}",
                    f[y * w + x]
                );
            }
        }
    }

    /// An empty field stays empty rather than producing garbage distances.
    #[test]
    fn distance_transform_of_nothing_is_far_away() {
        let mut f = vec![INF; 8 * 8];
        dist_sq(&mut f, 8, 8);
        assert!(f.iter().all(|d| *d > 1e6), "no seed => nothing is near");
    }

    /// One opaque black pixel in the middle of a tile, outlined white at 2 px:
    /// the ink survives untouched, the ring is white and opaque, and it fades
    /// out exactly where the number says rather than somewhere near it.
    #[test]
    fn outline_rings_the_ink_and_leaves_it_alone() {
        let mut src = Tile::new_transparent();
        src.set_pixel(32, 32, [0, 0, 0, 32768]);
        let p = EdgeParams {
            width_px: 2.0,
            colour: [255, 255, 255],
            ..EdgeParams::default()
        };
        let r = p.reach();
        let side = TILE_SIZE + 2 * r;
        let mut seed = vec![INF; side * side];
        seed[(32 + r) * side + (32 + r)] = 0.0;
        let out = derive_tile(&mut seed, r, Some(&src), p, &[]);

        assert_eq!(out.pixel(32, 32), [0, 0, 0, 32768], "the ink is untouched");
        let ring = out.pixel(33, 32);
        assert_eq!(ring[3], 32768, "1 px out is fully covered");
        assert_eq!(ring[0], 32768, "and it is white");
        // The boundary sits AT the width and is antialiased across the pixel
        // around it — the text engine's フチ convention. 2 px out of a 2 px
        // outline is therefore the half-covered edge sample, not a full one;
        // pin it, because "the outline is a pixel fatter than the number"
        // would be the classic way for this to rot unnoticed.
        assert_eq!(out.pixel(34, 32)[3], 16384, "the edge sample is half");
        assert_eq!(out.pixel(35, 32), [0; 4], "and it is over by 3 px out");
        assert_eq!(out.pixel(40, 32), [0; 4], "8 px out is still empty");
        assert_eq!(out.pixel(0, 0), [0; 4], "the corner never sees the ring");
    }

    /// Zero width is a real "off": no ring, and the pixels come through
    /// byte-identically. (The UI cannot produce it, `refresh_edge` can — a
    /// user dragging the width bar to 0.)
    #[test]
    fn zero_width_changes_nothing() {
        let mut src = Tile::new_transparent();
        src.set_pixel(10, 10, [1000, 2000, 3000, 20000]);
        let p = EdgeParams {
            width_px: 0.0,
            colour: [255, 0, 0],
            ..EdgeParams::default()
        };
        let side = TILE_SIZE;
        let mut seed = vec![INF; side * side];
        seed[10 * side + 10] = 0.0;
        let out = derive_tile(&mut seed, 0, Some(&src), p, &[]);
        assert_eq!(out.pixel(10, 10), [1000, 2000, 3000, 20000]);
        assert_eq!(out.pixel(11, 10), [0; 4]);
    }

    /// Every field of `EdgeParams` must move `sig()` — the GPU staleness
    /// hash is only as good as this list (see `ToneParams::sig`).
    #[test]
    fn sig_covers_every_field() {
        let base = EdgeParams::default();
        for v in [
            EdgeParams {
                width_px: 9.0,
                ..base
            },
            EdgeParams {
                colour: [1, 2, 3],
                ..base
            },
            EdgeParams {
                style: EdgeStyle::Watercolour,
                ..base
            },
        ] {
            assert_ne!(v.sig(), base.sig(), "{v:?} hashes the same as the default");
        }
    }

    /// LP-004 watercolour edge: the rim's colour comes from the layer's
    /// OWN nearest ink (the picked colour is ignored), and it lands
    /// paler than the solid keyline at the same width.
    #[test]
    fn watercolour_derives_a_pale_rim_from_the_ink() {
        let mut src = Tile::new_transparent();
        // A solid RED ink pixel (premul fix15: full red at full alpha).
        src.set_pixel(32, 32, [32768, 0, 0, 32768]);
        let p = EdgeParams {
            width_px: 2.0,
            // Blue on purpose: a watercolour rim that used it would fail
            // every assertion below.
            colour: [0, 0, 255],
            style: EdgeStyle::Watercolour,
        };
        let r = p.reach();
        let side = TILE_SIZE + 2 * r;
        let mut seed = vec![INF; side * side];
        let mut cwin = vec![0u16; side * side * 4];
        let w = (32 + r) * side + (32 + r);
        seed[w] = 0.0;
        cwin[w * 4..w * 4 + 4].copy_from_slice(&[32768, 0, 0, 32768]);
        let out = derive_tile(&mut seed, r, Some(&src), p, &cwin);

        let ring = out.pixel(33, 32);
        assert!(ring[3] > 20000, "the stain is there: {}", ring[3]);
        assert!(
            ring[3] < 32768,
            "and paler than the solid ring's full coverage"
        );
        assert!(
            ring[0] > ring[2],
            "the rim is RED (derived), not blue (picked): {ring:?}"
        );
        assert_eq!(out.pixel(32, 32), [32768, 0, 0, 32768], "ink untouched");
        assert_eq!(out.pixel(35, 32), [0; 4], "over by 3 px out, as ever");
    }

    /// Old files load as Solid: the style field is serde-defaulted, so a
    /// JSON blob written before it existed deserializes to the keyline.
    #[test]
    fn edge_params_without_a_style_load_solid() {
        let v: EdgeParams = serde_json::from_str(
            r#"{"width_px": 3.0, "colour": [12, 34, 56]}"#,
        )
        .unwrap();
        assert_eq!(v.style, EdgeStyle::Solid);
        assert_eq!(v.width_px, 3.0);
    }

    // ---- row 71, the brush-side rim -------------------------------------

    /// A 96×96 canvas with an opaque square of `colour` covering
    /// `x0..x1 × y0..y1`, plus the empty pre-stroke snapshot that makes the
    /// whole square read as "this stroke's coverage".
    fn inked_square(
        rect: (usize, usize, usize, usize),
        colour: [u16; 4],
    ) -> (Document, HashMap<TileIdx, Arc<Tile>>) {
        let mut doc = Document::new(96, 96);
        let (x0, y0, x1, y1) = rect;
        for y in y0..y1 {
            for x in x0..x1 {
                let idx = TileIdx::of_pixel(x as i32, y as i32);
                let (ox, oy) = idx.origin();
                doc.active_layer_mut().tile_mut(idx).set_pixel(
                    x - ox as usize,
                    y - oy as usize,
                    colour,
                );
            }
        }
        (doc, HashMap::new())
    }

    /// Snapshot the whole active layer the way `MyBrush::begin` does.
    fn snapshot(doc: &Document) -> HashMap<TileIdx, Arc<Tile>> {
        doc.active_layer()
            .tiles()
            .map(|(i, t)| (i, t.clone()))
            .collect()
    }

    /// The differential: with the rim armed, pixels OUTSIDE the stroke gain
    /// ink and pixels inside it keep the exact bytes the dabs left. Width is
    /// obeyed to the pixel — the same half-pixel boundary the layer effect
    /// uses, which is the whole point of sharing `dist_sq`.
    #[test]
    fn stroke_rim_darkens_outside_and_leaves_the_interior_alone() {
        let (mut doc, pre) = inked_square((32, 32, 48, 48), [0, 0, 0, 32768]);
        let p = WaterEdge {
            px: 2.0,
            opacity: 1.0,
            darkness: 0.0,
            blur_px: 0.0,
        };
        assert!(apply_stroke_rim(&mut doc, &pre, p), "the pass painted");

        let px = |x: i32, y: i32| {
            let i = TileIdx::of_pixel(x, y);
            let (ox, oy) = i.origin();
            doc.active_layer()
                .tile(i)
                .map(|t| t.pixel((x - ox) as usize, (y - oy) as usize))
                .unwrap_or([0; 4])
        };
        // Interior: byte-identical to the ink the dabs laid down.
        assert_eq!(px(40, 40), [0, 0, 0, 32768], "the stroke's body");
        assert_eq!(px(33, 33), [0, 0, 0, 32768], "one pixel in is still body");
        // Outside: the rim, out to the width and not past it.
        assert_eq!(px(48, 40)[3], 32768, "1 px out is fully rimmed");
        assert_eq!(px(49, 40)[3], 16384, "2 px out is the half edge sample");
        assert_eq!(px(50, 40), [0; 4], "3 px out of a 2 px rim is clean");
        assert_eq!(px(60, 40), [0; 4], "and far out is untouched");
    }

    /// Zero width is a real OFF: the pass returns before it reads a pixel
    /// and every byte of the layer is the byte the dabs left. This is the
    /// pin that keeps every old brush drawing exactly as it did.
    #[test]
    fn stroke_rim_at_zero_width_is_byte_identical() {
        let (mut doc, pre) = inked_square((32, 32, 48, 48), [0, 0, 0, 32768]);
        let before: Vec<(TileIdx, Vec<u16>)> = doc
            .active_layer()
            .tiles()
            .map(|(i, t)| (i, t.data().to_vec()))
            .collect();
        let before_len = before.len();
        for p in [
            WaterEdge::default(),
            WaterEdge {
                px: 0.0,
                opacity: 1.0,
                darkness: 1.0,
                blur_px: 4.0,
            },
            // Opacity 0 is the other honest off — a rim nobody can see.
            WaterEdge {
                px: 8.0,
                opacity: 0.0,
                ..WaterEdge::default()
            },
        ] {
            assert!(!apply_stroke_rim(&mut doc, &pre, p), "{p:?} painted");
        }
        for (i, want) in before {
            assert_eq!(doc.active_layer().tile(i).unwrap().data(), &want[..]);
        }
        assert_eq!(
            doc.active_layer().tiles().count(),
            before_len,
            "and no empty tile was materialised"
        );
    }

    /// An eraser stroke drove alpha DOWN, so its coverage is zero everywhere
    /// and it rims nothing. Without the clamp the hole would come back
    /// fringed — the classic way this feature ruins an erase.
    #[test]
    fn an_eraser_stroke_rims_nothing() {
        let (mut doc, _) = inked_square((16, 16, 80, 80), [0, 0, 0, 32768]);
        let pre = snapshot(&doc);
        // Erase the middle out of it.
        for y in 32..48 {
            for x in 32..48 {
                let i = TileIdx::of_pixel(x, y);
                let (ox, oy) = i.origin();
                doc.active_layer_mut()
                    .tile_mut(i)
                    .set_pixel((x - ox) as usize, (y - oy) as usize, [0; 4]);
            }
        }
        let after = snapshot(&doc);
        let p = WaterEdge {
            px: 3.0,
            opacity: 1.0,
            ..WaterEdge::default()
        };
        assert!(!apply_stroke_rim(&mut doc, &pre, p), "nothing to rim");
        for (i, t) in after {
            assert_eq!(doc.active_layer().tile(i).unwrap().data(), t.data());
        }
    }

    /// A stroke laid over existing art rims only the ink IT added: the
    /// coverage is a difference, not the union. Drawn as a second square
    /// touching the first — the shared border grows no rim, because from
    /// this stroke's side it is interior.
    #[test]
    fn the_rim_follows_the_new_ink_not_the_layer() {
        let (mut doc, _) = inked_square((16, 32, 32, 48), [0, 0, 0, 32768]);
        let pre = snapshot(&doc);
        for y in 32..48 {
            for x in 32..48 {
                let i = TileIdx::of_pixel(x, y);
                let (ox, oy) = i.origin();
                doc.active_layer_mut().tile_mut(i).set_pixel(
                    (x - ox) as usize,
                    (y - oy) as usize,
                    [0, 0, 0, 32768],
                );
            }
        }
        let p = WaterEdge {
            px: 2.0,
            opacity: 1.0,
            ..WaterEdge::default()
        };
        assert!(apply_stroke_rim(&mut doc, &pre, p));
        let px = |x: i32, y: i32| {
            let i = TileIdx::of_pixel(x, y);
            let (ox, oy) = i.origin();
            doc.active_layer()
                .tile(i)
                .map(|t| t.pixel((x - ox) as usize, (y - oy) as usize))
                .unwrap_or([0; 4])
        };
        assert_eq!(px(48, 40)[3], 32768, "the free edge is rimmed");
        // The old square's far side is 16 px from the new ink: outside the
        // reach, so it keeps its own bytes. If the pass had rimmed the
        // layer's alpha instead of the stroke's, this pixel would be ink.
        assert_eq!(px(15, 40), [0; 4], "the old square grew no rim");
    }

    /// A TRANSLUCENT stroke is rimmed, not filled in.
    ///
    /// Failed against the first cut of this pass, which weighted the rim by
    /// `1 − coverage` instead of skipping covered pixels: it read fine on an
    /// opaque stroke and turned a 25 % wash into a solid slab, because every
    /// pixel of a 25 % stroke is "mostly not covered". CSP's wording —
    /// *added to the outside of the stroke* — is a boundary, not a blend.
    #[test]
    fn a_translucent_stroke_keeps_its_translucency() {
        let faint = [2048, 2048, 2048, 8192]; // 25 % grey
        let (mut doc, pre) = inked_square((32, 32, 48, 48), faint);
        let p = WaterEdge {
            px: 2.0,
            opacity: 1.0,
            ..WaterEdge::default()
        };
        assert!(apply_stroke_rim(&mut doc, &pre, p));
        let px = |x: i32, y: i32| {
            let i = TileIdx::of_pixel(x, y);
            let (ox, oy) = i.origin();
            doc.active_layer()
                .tile(i)
                .map(|t| t.pixel((x - ox) as usize, (y - oy) as usize))
                .unwrap_or([0; 4])
        };
        assert_eq!(px(40, 40), faint, "the wash keeps its own alpha");
        assert_eq!(px(47, 40), faint, "including the pixel at its own edge");
        assert!(px(48, 40)[3] > 0, "and the rim is still outside it");
    }

    /// `W-003`: darkness desaturates the rim toward its own luma AND takes
    /// the value down, because CSP's "less saturated ⇒ looks darker" is only
    /// true if something actually gets darker.
    #[test]
    fn darkness_desaturates_and_darkens_the_rim() {
        let red = [1.0, 0.0, 0.0];
        assert_eq!(rim_colour(red, 0.0), red, "0 is the ink as drawn");
        let k = rim_colour(red, 1.0);
        let luma = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        assert!(
            (k[0] - k[1]).abs() < 1e-6 && (k[1] - k[2]).abs() < 1e-6,
            "fully desaturated: {k:?}"
        );
        assert!(luma(k) < luma(red), "and darker: {} vs {}", luma(k), luma(red));
        let half = rim_colour(red, 0.5);
        assert!(
            luma(red) > luma(half) && luma(half) > luma(k),
            "monotonic in the knob"
        );
    }

    /// `W-005`: blurring softens the rim's outer boundary instead of
    /// widening it — the far edge fades in over the blur distance while the
    /// reach stays where the width put it.
    #[test]
    fn blurring_width_softens_the_rim_without_widening_it() {
        let hard = {
            let (mut d, pre) = inked_square((32, 32, 48, 48), [0, 0, 0, 32768]);
            let p = WaterEdge {
                px: 4.0,
                opacity: 1.0,
                darkness: 0.0,
                blur_px: 0.0,
            };
            apply_stroke_rim(&mut d, &pre, p);
            d
        };
        let (mut soft, pre) = inked_square((32, 32, 48, 48), [0, 0, 0, 32768]);
        apply_stroke_rim(
            &mut soft,
            &pre,
            WaterEdge {
                px: 4.0,
                opacity: 1.0,
                darkness: 0.0,
                blur_px: 3.0,
            },
        );
        let a = |d: &Document, x: i32, y: i32| {
            let i = TileIdx::of_pixel(x, y);
            let (ox, oy) = i.origin();
            d.active_layer()
                .tile(i)
                .map(|t| t.pixel((x - ox) as usize, (y - oy) as usize)[3])
                .unwrap_or(0)
        };
        assert_eq!(a(&hard, 49, 40), 32768, "hard rim is solid mid-way out");
        assert!(
            a(&soft, 49, 40) < a(&hard, 49, 40),
            "blurred is fainter there: {} vs {}",
            a(&soft, 49, 40),
            a(&hard, 49, 40)
        );
        assert_eq!(a(&hard, 52, 40), 0, "hard rim is over at 4 px");
        assert_eq!(a(&soft, 52, 40), 0, "and blur did not widen the reach");
    }
}
