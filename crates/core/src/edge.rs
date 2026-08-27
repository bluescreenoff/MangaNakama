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

use serde::{Deserialize, Serialize};

use crate::tile::{TILE_SIZE, Tile};

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
}
