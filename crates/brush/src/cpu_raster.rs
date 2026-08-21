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
        // The C dispatch's precomputed opacities (process_op, paint<1 arm —
        // paint>0 dabs never take the GPU path, so (1-paint) is 1 there, but
        // the formula stays faithful). `op->normal` in the C is scaled by
        // (1-lock_alpha)(1-colorize)(1-posterize); the LockAlpha stamp's
        // opacity carries the (1-colorize)(1-posterize) factors too.
        let cp = (1.0 - d.colorize) * (1.0 - d.posterize);
        let normal = (1.0 - d.lock_alpha) * cp * d.opaque * (1.0 - d.paint);
        let lock = d.lock_alpha * d.opaque * cp * (1.0 - d.paint);
        let f15 = |v: f32| (v.clamp(0.0, 1.0) * 32768.0) as u32;
        let opa_normal = f15(normal);
        let opa_lock = f15(lock);
        let opa_colorize = f15(d.colorize * d.opaque);
        let opa_posterize = f15(d.posterize * d.opaque);
        let poster_n = d.posterize_num.max(1) as u32;
        let color_a = f15(d.alpha);

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
