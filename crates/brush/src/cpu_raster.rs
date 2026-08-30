//! CPU re-rasterization of a recorded dab list — the GPU-dabs canary-repair
//! path (docs/design/GPU-DABS.md §6) and the P2 parity reference. The math
//! mirrors `render_dab_mask` + `process_op` in the vendored C exactly (the
//! blend is u32 fix15 arithmetic; the mask is the same f32 formulas, so a
//! repaired stroke matches the C raster within the parity tolerance, not
//! bit-for-bit).

use mn_core::dab::DabParams;
use mn_core::{Document, TILE_SIZE, TileIdx};

/// Tiles a dab touches (the C's `floor(floor(x ± r_fringe) / 64)` range).
fn dab_tiles(d: &DabParams, stamp: bool) -> impl Iterator<Item = (i32, i32)> {
    // #10 amendment 3: an anchored stamp rotates a square — sqrt(2) reach.
    let fringe = if stamp {
        d.radius * std::f32::consts::SQRT_2 + 1.0
    } else {
        d.radius + 1.0
    };
    let x0 = (d.x - fringe).floor().div_euclid(64.0) as i32;
    let x1 = (d.x + fringe).floor().div_euclid(64.0) as i32;
    let y0 = (d.y - fringe).floor().div_euclid(64.0) as i32;
    let y1 = (d.y + fringe).floor().div_euclid(64.0) as i32;
    (y0..=y1).flat_map(move |ty| (x0..=x1).map(move |tx| (tx, ty)))
}

/// calculate_r_sample: squared (unnormalized) distance.
fn r_of(xx: f32, yy: f32, aspect: f32, sn: f32, cs: f32) -> f32 {
    let yyr = (yy * cs - xx * sn) * aspect;
    let xxr = yy * sn + xx * cs;
    yyr * yyr + xxr * xxr
}

/// calculate_rr: squared distance normalized by radius².
fn rr_of(xx: f32, yy: f32, aspect: f32, sn: f32, cs: f32, one_over_radius2: f32) -> f32 {
    r_of(xx, yy, aspect, sn, cs) * one_over_radius2
}

/// calculate_rr_antialiased — the small-radius (radius < 3) AA path.
fn rr_antialiased(
    px: f32,
    py: f32,
    x: f32,
    y: f32,
    aspect: f32,
    sn: f32,
    cs: f32,
    one_over_radius2: f32,
    r_aa_start: f32,
) -> f32 {
    let pixel_right = x - px;
    let pixel_bottom = y - py;
    let pixel_center_x = pixel_right - 0.5;
    let pixel_center_y = pixel_bottom - 0.5;
    let pixel_left = pixel_right - 1.0;
    let pixel_top = pixel_bottom - 1.0;

    let (nearest_x, nearest_y, rr_near);
    if pixel_left < 0.0 && pixel_right > 0.0 && pixel_top < 0.0 && pixel_bottom > 0.0 {
        nearest_x = 0.0;
        nearest_y = 0.0;
        rr_near = 0.0;
    } else {
        let l2 = cs * cs + sn * sn;
        let t = (pixel_center_x * cs + pixel_center_y * sn) / l2;
        let nx = (cs * t).clamp(pixel_left, pixel_right);
        let ny = (sn * t).clamp(pixel_top, pixel_bottom);
        nearest_x = nx;
        nearest_y = ny;
        rr_near = rr_of(nx, ny, aspect, sn, cs, one_over_radius2);
    }
    if rr_near > 1.0 {
        return rr_near;
    }
    // sign_point_in_line(pcx, pcy, cs, -sn), inlined.
    let center_sign = (pixel_center_x - cs) * sn - cs * (pixel_center_y + sn);
    let rad_area_1 = (1.0 / std::f32::consts::PI).sqrt();
    let (fx, fy) = if center_sign < 0.0 {
        (nearest_x - sn * rad_area_1, nearest_y + cs * rad_area_1)
    } else {
        (nearest_x + sn * rad_area_1, nearest_y - cs * rad_area_1)
    };
    // The skip-test compares the UNSCALED r_far (calculate_r_sample) against
    // r_aa_start — not rr_far (which folds in 1/radius²).
    let r_far = r_of(fx, fy, aspect, sn, cs);
    let rr_far = r_far * one_over_radius2;
    if r_far < r_aa_start {
        return (rr_far + rr_near) * 0.5;
    }
    let visibility_near = (1.0 - rr_near) / (1.0 + (rr_far - rr_near));
    1.0 - visibility_near
}

/// The dab's mask at one tile-local pixel (render_dab_mask's per-pixel math,
/// minus the RLE encoding — irrelevant off the hot path). `tex` is the
/// active texture-tip mask (data, size); the multiply is canvas-anchored
/// with the dab's own crawl offset — the C order (f32 profile × gray before
/// the u16 quantization), matching draw-time snapshots since #0.1.
fn mask_of(
    d: &DabParams,
    lx: f32,
    ly: f32,
    xp: i32,
    yp: i32,
    tx: i32,
    ty: i32,
    hard: bool,
    tex: Option<(&[u8], u32, bool)>,
) -> u32 {
    // #10 amendment 3: PURE STAMP — dab-anchored mode takes coverage from
    // the tip sample alone (no radial profile, no hard-dab disc); the
    // profile made every stamp a disc with texture only at the edges.
    if let Some((data, size, true)) = tex {
        let ta = d.tex_angle / 360.0 * 2.0 * std::f32::consts::PI;
        let (tsn, tcs) = ta.sin_cos();
        let xx = xp as f32 + 0.5 - lx;
        let yy = yp as f32 + 0.5 - ly;
        let xxr = yy * tsn + xx * tcs;
        let yyr = yy * tcs - xx * tsn;
        let u = (xxr / d.radius * 0.5 + 0.5) * size as f32;
        let v = (yyr / d.radius * 0.5 + 0.5) * size as f32;
        if u < 0.0 || v < 0.0 || u >= size as f32 || v >= size as f32 {
            return 0;
        }
        // BILINEAR, texel centres at +0.5 — the exact arithmetic of the C
        // and the shader (nearest would let 1-ulp trig skew flip texels).
        let (uf, vf) = (u - 0.5, v - 0.5);
        let (u0f, v0f) = (uf.floor(), vf.floor());
        let (fu, fv) = (uf - u0f, vf - v0f);
        let cl = |i: i32| i.clamp(0, size as i32 - 1) as usize;
        let (u0, v0) = (cl(u0f as i32), cl(v0f as i32));
        let (u1, v1) = (cl(u0f as i32 + 1), cl(v0f as i32 + 1));
        let at = |vv: usize, uu: usize| data[vv * size as usize + uu] as f32;
        let g = at(v0, u0) * (1.0 - fu) * (1.0 - fv)
            + at(v0, u1) * fu * (1.0 - fv)
            + at(v1, u0) * (1.0 - fu) * fv
            + at(v1, u1) * fu * fv;
        return (g / 255.0 * 32768.0) as u32;
    }

    let hardness = d.hardness.clamp(0.0, 1.0);
    let angle_rad = d.angle / 360.0 * 2.0 * std::f32::consts::PI;
    let cs = angle_rad.cos();
    let sn = angle_rad.sin();
    let one_over_radius2 = 1.0 / (d.radius * d.radius);

    let rr = if d.radius < 3.0 {
        let aa_border = 1.0;
        let mut r_aa_start = if d.radius > aa_border {
            d.radius - aa_border
        } else {
            0.0
        };
        r_aa_start = r_aa_start * r_aa_start / d.aspect_ratio;
        rr_antialiased(
            xp as f32,
            yp as f32,
            lx,
            ly,
            d.aspect_ratio,
            sn,
            cs,
            one_over_radius2,
            r_aa_start,
        )
    } else {
        let yy = yp as f32 + 0.5 - ly;
        let xx = xp as f32 + 0.5 - lx;
        rr_of(xx, yy, d.aspect_ratio, sn, cs, one_over_radius2)
    };

    let mut opa = if hard {
        (d.radius * (1.0 - rr) + 0.5).clamp(0.0, 1.0)
    } else {
        // calculate_opa: two linear segments meeting at the hardness knot.
        let segment1_offset = 1.0f32;
        let segment1_slope = -(1.0 / hardness - 1.0);
        let segment2_offset = hardness / (1.0 - hardness);
        let segment2_slope = -hardness / (1.0 - hardness);
        let mut o = if rr <= hardness {
            segment1_offset + rr * segment1_slope
        } else {
            segment2_offset + rr * segment2_slope
        };
        if rr > 1.0 {
            o = 0.0;
        }
        o
    };
    // Canvas-anchored grain (the dab-anchored stamp returned above).
    if let Some((data, size, _)) = tex {
        let n = size as i32;
        let cx = tx * TILE_SIZE as i32 + xp + d.tex_off[0];
        let cy = ty * TILE_SIZE as i32 + yp + d.tex_off[1];
        let ui = cx.rem_euclid(n) as usize;
        let vi = cy.rem_euclid(n) as usize;
        opa *= data[vi * size as usize + ui] as f32 / 255.0;
    }
    (opa * 32768.0) as u32
}

/// `set_rgb16_lum_from_rgb16` (brushmodes.c): set the bottom triple's
/// luminance to the top's (BT.601 coeffs), then ClipColor per the PDF
/// blend-modes addendum. Straight (non-premult) fix15 in and out. The C
/// mixes float LUMA products with truncating integer divisions — mirrored
/// exactly; the two clip divisions are guarded against the degenerate
/// all-equal case (0/0 in the C is unreachable for real pixel values, but
/// Rust would panic where C shrugs).
fn set_lum(topr: i32, topg: i32, topb: i32, botr: i32, botg: i32, botb: i32) -> (i32, i32, i32) {
    const RC: f32 = 0.2126 * 32768.0;
    const GC: f32 = 0.7152 * 32768.0;
    const BC: f32 = 0.0722 * 32768.0;
    let luma = |r: i32, g: i32, b: i32| -> f32 { r as f32 * RC + g as f32 * GC + b as f32 * BC };
    let botlum = (luma(botr, botg, botb) / 32768.0) as i32;
    let toplum = (luma(topr, topg, topb) / 32768.0) as i32;
    let diff = botlum - toplum;
    let mut r = topr + diff;
    let mut g = topg + diff;
    let mut b = topb + diff;
    let lum = (luma(r, g, b) / 32768.0) as i32;
    let cmin = r.min(g).min(b);
    let cmax = r.max(g).max(b);
    if cmin < 0 && lum != cmin {
        r = lum + ((r - lum) * lum) / (lum - cmin);
        g = lum + ((g - lum) * lum) / (lum - cmin);
        b = lum + ((b - lum) * lum) / (lum - cmin);
    }
    if cmax > 32768 && cmax != lum {
        r = lum + ((r - lum) * (32768 - lum)) / (cmax - lum);
        g = lum + ((g - lum) * (32768 - lum)) / (cmax - lum);
        b = lum + ((b - lum) * (32768 - lum)) / (cmax - lum);
    }
    (r, g, b)
}

// --- Spectral paint (the WGM pigment engine, brushmodes.c *_Paint arms) ---
// The C's pow is fastapprox's `fastpow` (bit tricks, not libm) — mirrored
// exactly, like the shader's port; a real powf here would be MORE accurate
// and drift off the reference.

const WGM_EPSILON: f32 = 0.001;
const SPEC_R: [f32; 10] = [
    0.009281362787953,
    0.009732627042016,
    0.011254252737167,
    0.015105578649573,
    0.024797924177217,
    0.083622585502406,
    0.977865045723212,
    1.0,
    0.999961046144372,
    0.999999992756822,
];
const SPEC_G: [f32; 10] = [
    0.002854127435775,
    0.003917589679914,
    0.012132151699187,
    0.748259205918013,
    1.0,
    0.865695937531795,
    0.037477469241101,
    0.022816789725717,
    0.021747419446456,
    0.021384940572308,
];
const SPEC_B: [f32; 10] = [
    0.537052150373386,
    0.546646402401469,
    0.575501819073983,
    0.258778829633924,
    0.041709923751716,
    0.012662638828324,
    0.007485593127390,
    0.006766900622462,
    0.006699764779016,
    0.006676219883241,
];
const T_MATRIX_SMALL: [[f32; 10]; 3] = [
    [
        0.026595621243689,
        0.049779426257903,
        0.022449850859496,
        -0.218453689278271,
        -0.256894883201278,
        0.445881722194840,
        0.772365886289756,
        0.194498761382537,
        0.014038157587820,
        0.007687264480513,
    ],
    [
        -0.032601672674412,
        -0.061021043498478,
        -0.052490001018404,
        0.206659098273522,
        0.572496335158169,
        0.317837248815438,
        -0.021216624031211,
        -0.019387668756117,
        -0.001521339050858,
        -0.000835181622534,
    ],
    [
        0.339475473216284,
        0.635401374177222,
        0.771520797089589,
        0.113222640692379,
        -0.055251113343776,
        -0.048222578468680,
        -0.012966666339586,
        -0.001523814504223,
        -0.000094718948810,
        -0.000051604594741,
    ],
];

/// fastapprox `fastlog2` (fastlog.h), bit for bit.
fn fastlog2(x: f32) -> f32 {
    let vx = x.to_bits();
    let mx = f32::from_bits((vx & 0x007F_FFFF) | 0x3f00_0000);
    let y = vx as f32 * 1.1920928955078125e-7;
    y - 124.22551499 - 1.498030302 * mx - 1.72587999 / (0.3520887068 + mx)
}

/// fastapprox `fastpow2` (fastexp.h): the (1<<23)*(...) float built straight
/// into the exponent field; `as` casts truncate like the C's.
fn fastpow2(p: f32) -> f32 {
    let offset = if p < 0.0 { 1.0f32 } else { 0.0 };
    let clipp = if p < -126.0 { -126.0 } else { p };
    let w = clipp as i32;
    let z = clipp - w as f32 + offset;
    let e = 8388608.0 * (clipp + 121.2740575 + 27.7280233 / (4.84252568 - z) - 1.49012907 * z);
    f32::from_bits(e as u32)
}

fn fastpow(x: f32, p: f32) -> f32 {
    fastpow2(p * fastlog2(x))
}

/// helpers.c `rgb_to_spectral`: straight rgb 0..1 upsampled to 10 bands.
fn rgb_to_spectral(r0: f32, g0: f32, b0: f32) -> [f32; 10] {
    let off = 1.0 - WGM_EPSILON;
    let r = r0 * off + WGM_EPSILON;
    let g = g0 * off + WGM_EPSILON;
    let b = b0 * off + WGM_EPSILON;
    let mut out = [0.0f32; 10];
    for i in 0..10 {
        out[i] = SPEC_R[i] * r + SPEC_G[i] * g + SPEC_B[i] * b;
    }
    out
}

/// helpers.c `spectral_to_rgb`: 3x10 matrix, sequential accumulation.
fn spectral_to_rgb(spec: &[f32; 10]) -> [f32; 3] {
    let off = 1.0 - WGM_EPSILON;
    let mut tmp = [0.0f32; 3];
    for i in 0..10 {
        tmp[0] += T_MATRIX_SMALL[0][i] * spec[i];
        tmp[1] += T_MATRIX_SMALL[1][i] * spec[i];
        tmp[2] += T_MATRIX_SMALL[2][i] * spec[i];
    }
    tmp.map(|t| ((t - WGM_EPSILON) / off).clamp(0.0, 1.0))
}

/// brushmodes.c `spectral_blend_factor`: the additive->spectral fade the
/// eraser-paint arm runs on canvas alpha.
fn spectral_blend_factor(x: f32) -> f32 {
    let b = x * 8.0 - 3.0;
    0.5 + b / (1.0 + b.abs() * 1.65)
}

/// The WGM mix at one pixel, back to rgb — shared by the three paint arms.
fn wgm_mix(spec_a: &[f32; 10], spec_b: &[f32; 10], fac_a: f32) -> [f32; 3] {
    let fac_b = 1.0 - fac_a;
    let mut mixed = [0.0f32; 10];
    for i in 0..10 {
        mixed[i] = fastpow(spec_a[i], fac_a) * fastpow(spec_b[i], fac_b);
    }
    spectral_to_rgb(&mixed)
}

/// Stamp a dab list onto one layer's tiles — the repair path's pixel writer.
/// Call inside the stroke's open undo op: `tile_mut` snapshots pre-images,
/// exactly as the vendored rasterizer's would have.
pub fn rasterize_dabs(
    doc: &mut Document,
    layer: usize,
    dabs: &[DabParams],
    hard_dab: bool,
    texture: Option<(&[u8], u32, bool)>,
) {
    let (ex, ey) = doc.tile_extent();
    let Some(dst) = doc.layers.get_mut(layer) else {
        return;
    };
    for d in dabs {
        // The C dispatch's precomputed opacities (process_op), BOTH halves:
        // the (1-paint) additive stamps and the paint>0 spectral stamps.
        // `op->normal` in the C is scaled by
        // (1-lock_alpha)(1-colorize)(1-posterize); the LockAlpha stamp's
        // opacity carries the (1-colorize)(1-posterize) factors too.
        let cp = (1.0 - d.colorize) * (1.0 - d.posterize);
        let normal_knob = (1.0 - d.lock_alpha) * cp;
        let normal = normal_knob * d.opaque * (1.0 - d.paint);
        let lock = d.lock_alpha * d.opaque * cp * (1.0 - d.paint);
        let f15 = |v: f32| (v.clamp(0.0, 1.0) * 32768.0) as u32;
        let opa_normal = f15(normal);
        let opa_lock = f15(lock);
        let opa_colorize = f15(d.colorize * d.opaque);
        let opa_posterize = f15(d.posterize * d.opaque);
        let poster_n = d.posterize_num.max(1) as u32;
        let color_a = f15(d.alpha);
        // Paint-arm CALL conditions mirror the C's, separate from the u16
        // opacity: a called Normal/LockAlpha_Paint clamps its opacity up to
        // 150 even when the conversion rounded it to 0.
        let paint_normal_on = d.paint > 0.0 && normal_knob != 0.0;
        let paint_lock_on = d.paint > 0.0 && d.lock_alpha > 0.0 && d.alpha != 0.0;
        let opa_paint = f15(normal_knob * d.opaque * d.paint);
        let opa_lock_paint = f15(d.lock_alpha * d.opaque * cp * d.paint);
        let spec_brush = (paint_normal_on || paint_lock_on).then(|| {
            rgb_to_spectral(
                d.color[0] as f32 / 32768.0,
                d.color[1] as f32 / 32768.0,
                d.color[2] as f32 / 32768.0,
            )
        });

        let stamp = texture.is_some_and(|(_, _, a)| a);
        for (tx, ty) in dab_tiles(d, stamp) {
            // Off-canvas dabs are DROPPED — the engine's surface hands them
            // a scratch tile and discards the writes, and flush_dabs clamps
            // its dispatch set the same way. The repair replays what the CPU
            // path would have painted; it must never grow the layer past
            // the document bounds (caught by the round-33 repair test: the
            // default viewport maps part of a screen stroke off-canvas).
            if tx < 0 || ty < 0 || tx >= ex || ty >= ey {
                continue;
            }
            // Tile-local dab bbox, clamped to the tile (render_dab_mask).
            let fringe = d.radius + 1.0;
            let x0 = (((d.x - fringe).floor() as i32) - tx * TILE_SIZE as i32).max(0);
            let y0 = (((d.y - fringe).floor() as i32) - ty * TILE_SIZE as i32).max(0);
            let x1 =
                (((d.x + fringe).floor() as i32) - tx * TILE_SIZE as i32).min(TILE_SIZE as i32 - 1);
            let y1 =
                (((d.y + fringe).floor() as i32) - ty * TILE_SIZE as i32).min(TILE_SIZE as i32 - 1);
            if x0 > x1 || y0 > y1 {
                continue;
            }
            let tile = dst.tile_mut(TileIdx::new(tx, ty));
            let lx = d.x - (tx * TILE_SIZE as i32) as f32;
            let ly = d.y - (ty * TILE_SIZE as i32) as f32;
            for yp in y0..=y1 {
                for xp in x0..=x1 {
                    let mask = mask_of(d, lx, ly, xp, yp, tx, ty, hard_dab, texture);
                    if mask == 0 {
                        continue;
                    }
                    let o = (yp as usize * TILE_SIZE + xp as usize) * 4;
                    let px = &mut tile.data_mut()[o..o + 4];

                    // draw_dab_pixels_BlendMode_Normal / _Normal_and_Eraser
                    // (brushmodes.c): u32 fix15, truncating division.
                    if opa_normal > 0 {
                        let opa_pre = mask * opa_normal / 32768;
                        if d.alpha >= 1.0 {
                            let opa_b = 32768 - opa_pre;
                            let opa_a = opa_pre;
                            px[3] = (opa_a + opa_b * px[3] as u32 / 32768) as u16;
                            px[0] =
                                ((opa_a * d.color[0] as u32 + opa_b * px[0] as u32) / 32768) as u16;
                            px[1] =
                                ((opa_a * d.color[1] as u32 + opa_b * px[1] as u32) / 32768) as u16;
                            px[2] =
                                ((opa_a * d.color[2] as u32 + opa_b * px[2] as u32) / 32768) as u16;
                        } else {
                            // opa_b uses the UNSCALED opa_pre (the C computes
                            // it before folding colour_a in).
                            let opa_b = 32768 - opa_pre;
                            let opa_a = opa_pre * color_a / 32768;
                            px[3] = (opa_a + opa_b * px[3] as u32 / 32768) as u16;
                            px[0] =
                                ((opa_a * d.color[0] as u32 + opa_b * px[0] as u32) / 32768) as u16;
                            px[1] =
                                ((opa_a * d.color[1] as u32 + opa_b * px[1] as u32) / 32768) as u16;
                            px[2] =
                                ((opa_a * d.color[2] as u32 + opa_b * px[2] as u32) / 32768) as u16;
                        }
                    }

                    // draw_dab_pixels_BlendMode_LockAlpha: alpha untouched.
                    if opa_lock > 0 && d.alpha != 0.0 && d.lock_alpha > 0.0 {
                        let opa_a0 = mask * opa_lock / 32768;
                        let opa_b = 32768 - opa_a0;
                        let opa_a = opa_a0 * px[3] as u32 / 32768;
                        px[0] = ((opa_a * d.color[0] as u32 + opa_b * px[0] as u32) / 32768) as u16;
                        px[1] = ((opa_a * d.color[1] as u32 + opa_b * px[1] as u32) / 32768) as u16;
                        px[2] = ((opa_a * d.color[2] as u32 + opa_b * px[2] as u32) / 32768) as u16;
                    }

                    // Spectral paint arms (the paint>0 half of process_op),
                    // in the C's dispatch order: after the (1-paint) stamps,
                    // before Colorize/Posterize.
                    if paint_normal_on {
                        let spec_a = spec_brush.as_ref().unwrap();
                        if d.alpha >= 1.0 {
                            // draw_dab_pixels_BlendMode_Normal_Paint, incl.
                            // the C's min-opacity clamp (150).
                            let opacity = opa_paint.max(150);
                            let opa_a = mask * opacity / 32768;
                            let opa_b = 32768 - opa_a;
                            if px[3] == 0 {
                                px[3] = (opa_a + opa_b * px[3] as u32 / 32768) as u16;
                                px[0] = ((opa_a * d.color[0] as u32 + opa_b * px[0] as u32)
                                    / 32768) as u16;
                                px[1] = ((opa_a * d.color[1] as u32 + opa_b * px[1] as u32)
                                    / 32768) as u16;
                                px[2] = ((opa_a * d.color[2] as u32 + opa_b * px[2] as u32)
                                    / 32768) as u16;
                            } else {
                                let fac_a = opa_a as f32
                                    / (opa_a + opa_b * px[3] as u32 / 32768) as f32;
                                let spec_b = rgb_to_spectral(
                                    px[0] as f32 / px[3] as f32,
                                    px[1] as f32 / px[3] as f32,
                                    px[2] as f32 / px[3] as f32,
                                );
                                let rgb = wgm_mix(spec_a, &spec_b, fac_a);
                                // Alpha first — the C re-premultiplies with
                                // the NEW alpha; +0.5 is its round-on-store.
                                px[3] = (opa_a + opa_b * px[3] as u32 / 32768) as u16;
                                px[0] = (rgb[0] * px[3] as f32 + 0.5) as u16;
                                px[1] = (rgb[1] * px[3] as f32 + 0.5) as u16;
                                px[2] = (rgb[2] * px[3] as f32 + 0.5) as u16;
                            }
                        } else {
                            // draw_dab_pixels_BlendMode_Normal_and_Eraser_
                            // Paint: no min clamp; additive/spectral fade on
                            // canvas alpha.
                            let opa_a = mask * opa_paint / 32768;
                            let opa_b = 32768 - opa_a;
                            let opa_a2 = opa_a * color_a / 32768;
                            let opa_out = opa_a2 + opa_b * px[3] as u32 / 32768;
                            let sf =
                                spectral_blend_factor(px[3] as f32 / 32768.0).clamp(0.0, 1.0);
                            let af = 1.0 - sf;
                            let mut out = [0u32; 3];
                            if af != 0.0 {
                                for c in 0..3 {
                                    out[c] = (opa_a2 * d.color[c] as u32
                                        + opa_b * px[c] as u32)
                                        / 32768;
                                }
                            }
                            if sf != 0.0 && px[3] != 0 {
                                let spec_b = rgb_to_spectral(
                                    px[0] as f32 / px[3] as f32,
                                    px[1] as f32 / px[3] as f32,
                                    px[2] as f32 / px[3] as f32,
                                );
                                let mut fac_a = opa_a as f32
                                    / (opa_a + opa_b * px[3] as u32 / 32768) as f32;
                                fac_a *= color_a as f32 / 32768.0;
                                let rgb = wgm_mix(spec_a, &spec_b, fac_a);
                                for c in 0..3 {
                                    out[c] = (af * out[c] as f32
                                        + sf * rgb[c] * opa_out as f32)
                                        as u32;
                                }
                            }
                            px[3] = opa_out as u16;
                            px[0] = out[0] as u16;
                            px[1] = out[1] as u16;
                            px[2] = out[2] as u16;
                        }
                    }
                    if paint_lock_on {
                        // draw_dab_pixels_BlendMode_LockAlpha_Paint. NOTE
                        // the C DOES rewrite alpha here (a truncating
                        // near-identity — opa_a was pre-scaled by it);
                        // mirrored, not "fixed".
                        let spec_a = spec_brush.as_ref().unwrap();
                        let opacity = opa_lock_paint.max(150);
                        let opa_a0 = mask * opacity / 32768;
                        let opa_b = 32768 - opa_a0;
                        let opa_a = opa_a0 * px[3] as u32 / 32768;
                        if px[3] == 0 {
                            // opa_a is 0 here; the C still runs the blend.
                            for c in 0..3 {
                                px[c] = ((opa_a * d.color[c] as u32 + opa_b * px[c] as u32)
                                    / 32768) as u16;
                            }
                        } else {
                            let fac_a =
                                opa_a as f32 / (opa_a + opa_b * px[3] as u32 / 32768) as f32;
                            let spec_b = rgb_to_spectral(
                                px[0] as f32 / px[3] as f32,
                                px[1] as f32 / px[3] as f32,
                                px[2] as f32 / px[3] as f32,
                            );
                            let rgb = wgm_mix(spec_a, &spec_b, fac_a);
                            px[3] = (opa_a + opa_b * px[3] as u32 / 32768) as u16;
                            px[0] = (rgb[0] * px[3] as f32 + 0.5) as u16;
                            px[1] = (rgb[1] * px[3] as f32 + 0.5) as u16;
                            px[2] = (rgb[2] * px[3] as f32 + 0.5) as u16;
                        }
                    }

                    // draw_dab_pixels_BlendMode_Color: de-premult, set the
                    // canvas pixel's hue/sat to the brush colour keeping its
                    // luma (PDF "Color" mode, BT.601 coeffs), re-premult,
                    // blend rgb only. Alpha untouched. Exact C integer math
                    // (i32 divisions truncate toward zero, like C's).
                    if opa_colorize > 0 {
                        let a = px[3] as u32;
                        let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
                        if a != 0 {
                            r = 32768 * px[0] as u32 / a;
                            g = 32768 * px[1] as u32 / a;
                            b = 32768 * px[2] as u32 / a;
                        }
                        let (r2, g2, b2) = set_lum(
                            d.color[0] as i32,
                            d.color[1] as i32,
                            d.color[2] as i32,
                            r as i32,
                            g as i32,
                            b as i32,
                        );
                        let r = r2 as u32 * a / 32768;
                        let g = g2 as u32 * a / 32768;
                        let b = b2 as u32 * a / 32768;
                        let opa_a = mask * opa_colorize / 32768;
                        let opa_b = 32768 - opa_a;
                        px[0] = ((opa_a * r + opa_b * px[0] as u32) / 32768) as u16;
                        px[1] = ((opa_a * g + opa_b * px[1] as u32) / 32768) as u16;
                        px[2] = ((opa_a * b + opa_b * px[2] as u32) / 32768) as u16;
                    }

                    // draw_dab_pixels_BlendMode_Posterize: quantize the
                    // canvas rgb (premult, as the C does) to posterize_num
                    // levels, blend at the stamp's opacity. ROUND is the C's
                    // (int)(x + 0.5); the rest is integer.
                    if opa_posterize > 0 {
                        let post = |v: u16| -> u32 {
                            let f = v as f32 / 32768.0;
                            32768 * ((f * poster_n as f32 + 0.5) as u32) / poster_n
                        };
                        let (pr, pg, pb) = (post(px[0]), post(px[1]), post(px[2]));
                        let opa_a = mask * opa_posterize / 32768;
                        let opa_b = 32768 - opa_a;
                        px[0] = ((opa_a * pr + opa_b * px[0] as u32) / 32768) as u16;
                        px[1] = ((opa_a * pg + opa_b * px[1] as u32) / 32768) as u16;
                        px[2] = ((opa_a * pb + opa_b * px[2] as u32) / 32768) as u16;
                    }
                }
            }
        }
    }
}
