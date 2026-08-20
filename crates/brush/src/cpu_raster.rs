//! CPU re-rasterization of a recorded dab list — the GPU-dabs canary-repair
//! path (docs/design/GPU-DABS.md §6) and the P2 parity reference. The math
//! mirrors `render_dab_mask` + `process_op` in the vendored C exactly (the
//! blend is u32 fix15 arithmetic; the mask is the same f32 formulas, so a
//! repaired stroke matches the C raster within the parity tolerance, not
//! bit-for-bit).

use mn_core::dab::DabParams;
use mn_core::{Document, TILE_SIZE, TileIdx};

/// Tiles a dab touches (the C's `floor(floor(x ± r_fringe) / 64)` range).
fn dab_tiles(d: &DabParams) -> impl Iterator<Item = (i32, i32)> {
    let fringe = d.radius + 1.0;
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
    tex: Option<(&[u8], u32)>,
) -> u32 {
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
    if let Some((data, size)) = tex {
        let n = size as i32;
        let cx = tx * TILE_SIZE as i32 + xp + d.tex_off[0];
        let cy = ty * TILE_SIZE as i32 + yp + d.tex_off[1];
        let ui = cx.rem_euclid(n) as usize;
        let vi = cy.rem_euclid(n) as usize;
        opa *= data[vi * size as usize + ui] as f32 / 255.0;
    }
    (opa * 32768.0) as u32
}

/// Stamp a dab list onto one layer's tiles — the repair path's pixel writer.
/// Call inside the stroke's open undo op: `tile_mut` snapshots pre-images,
/// exactly as the vendored rasterizer's would have.
pub fn rasterize_dabs(
    doc: &mut Document,
    layer: usize,
    dabs: &[DabParams],
    hard_dab: bool,
    texture: Option<(&[u8], u32)>,
) {
    let (ex, ey) = doc.tile_extent();
    let Some(dst) = doc.layers.get_mut(layer) else {
        return;
    };
    for d in dabs {
        // The C dispatch's precomputed opacities (process_op, paint<1 arm —
        // paint>0 dabs never take the GPU path, so (1-paint) is 1 there, but
        // the formula stays faithful).
        let normal = (1.0 - d.lock_alpha) * d.opaque * (1.0 - d.paint);
        let lock = d.lock_alpha * d.opaque * (1.0 - d.paint);
        let f15 = |v: f32| (v.clamp(0.0, 1.0) * 32768.0) as u32;
        let opa_normal = f15(normal);
        let opa_lock = f15(lock);
        let color_a = f15(d.alpha);

        for (tx, ty) in dab_tiles(d) {
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
                }
            }
        }
    }
}
