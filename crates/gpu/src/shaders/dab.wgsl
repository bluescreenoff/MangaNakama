// GPU dab rasterization — docs/design/GPU-DABS.md P1.
//
// One workgroup (16x16 threads) owns ONE 64x64 tile; each thread holds a 4x4
// pixel block in registers: the dst tile is loaded ONCE from a scratch copy
// (rgba16uint cannot be read_write storage, so the pass samples a copy and
// writes the original), every dab of this flush is applied IN ORDER —
// libmypaint's per-tile op-queue semantics — and the block is stored once.
// All integer math mirrors brushmodes.c exactly (u32, fix15 = 1<<15,
// truncating division); the f32 mask mirrors render_dab_mask.

struct DabG {
    x: f32,
    y: f32,
    radius: f32,
    hardness: f32,
    aspect: f32,
    angle: f32,
    // Straight colour, fix15, widened.
    color_r: u32,
    color_g: u32,
    color_b: u32,
    color_a: u32,
    // Precomputed blend opacities, fix15 (the C dispatch math done on CPU):
    //   opa_normal = (1-lock_alpha)*opaque*(1-paint) * 32768
    //   opa_lock   =  lock_alpha     *opaque*(1-paint) * 32768
    opa_normal: u32,
    opa_lock: u32,
    // bit0: colour_a < 1 (Normal_and_Eraser); bit1: LockAlpha applies.
    flags: u32,
    // Texture-tip crawl offset (mask px) this dab sees; 0 when off.
    tex_u: i32,
    tex_v: i32,
    // Dab-anchored stamp rotation (#10 amendment 2), CPU-precomputed
    // sin/cos — GPU trig intrinsics are orders coarser than libm and broke
    // the <=1 parity bar.
    tex_sn: f32,
    tex_cs: f32,
    // Colorize / Posterize stamp opacities, fix15, and the posterize level
    // count (1..=128, C-clamped). 20 scalars = 80 bytes, matching Rust's
    // `GpuDab` exactly; the array stride must agree or dabs[1..] read
    // misaligned.
    opa_colorize: u32,
    opa_posterize: u32,
    poster_num: u32,
};

struct TileUni {
    // Tile origin in canvas px (tile index * 64).
    ox: i32,
    oy: i32,
    dab_count: u32,
    // bit0: hard-stamp dabs (exact AA disc instead of hardness falloff).
    flags: u32,
    // Texture-tip mask side length; 0 = no texture this flush.
    tex_size: u32,
    // Scalar pads (not vecs — uniform vec alignment is 16) to match Rust's
    // 64-byte TileUni exactly; never read.
    _p0: u32, _p1: u32, _p2: u32, _p3: u32, _p4: u32, _p5: u32,
    _p6: u32, _p7: u32, _p8: u32, _p9: u32, _p10: u32,
};

@group(0) @binding(0) var dst_src: texture_2d<u32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba16uint, write>;
@group(0) @binding(2) var<storage, read> dabs: array<DabG>;
@group(0) @binding(3) var<uniform> tile: TileUni;
// Cursed-driver canary: every dispatched workgroup bumps this once; the
// stroke-end readback compares it against the expected dispatch count and
// repairs on CPU when they disagree (design doc §6).
@group(0) @binding(4) var<storage, read_write> canary: atomic<u32>;
// The texture-tip mask (#0.1): gray u8 widened to R32Uint texels. A 1×1
// zero dummy is bound when off — the tex_size gate means it never loads.
@group(0) @binding(5) var tex: texture_2d<u32>;

const FIX15: u32 = 32768u;

// BT.601 luma of a straight fix15 triple, as brushmodes.c's LUMA macro:
// float products of the fix15-scaled coefficients; the caller divides by
// 32768 and truncates, exactly like the C's int conversions.
fn luma(r: i32, g: i32, b: i32) -> f32 {
    return f32(r) * (0.2126 * 32768.0)
        + f32(g) * (0.7152 * 32768.0)
        + f32(b) * (0.0722 * 32768.0);
}

// calculate_r_sample: squared (unnormalized) distance from the dab centre,
// aspect-stretched, rotated.
fn r_of(xx: f32, yy: f32, aspect: f32, sn: f32, cs: f32) -> f32 {
    let yyr = (yy * cs - xx * sn) * aspect;
    let xxr = yy * sn + xx * cs;
    return yyr * yyr + xxr * xxr;
}

// calculate_rr: squared distance normalized by radius².
fn rr_of(xx: f32, yy: f32, aspect: f32, sn: f32, cs: f32, one_over_radius2: f32) -> f32 {
    return r_of(xx, yy, aspect, sn, cs) * one_over_radius2;
}

// calculate_rr_antialiased: blend the rr of the pixel's nearest and farthest
// in-pixel points, de-occluded — small-dab AA without supersampling.
fn rr_antialiased(
    px: f32, py: f32,          // pixel top-left (dab centre at the origin)
    x: f32, y: f32,            // dab centre in tile-local coords
    aspect: f32, sn: f32, cs: f32, one_over_radius2: f32, r_aa_start: f32,
) -> f32 {
    let pixel_right = x - px;
    let pixel_bottom = y - py;
    let pixel_center_x = pixel_right - 0.5;
    let pixel_center_y = pixel_bottom - 0.5;
    let pixel_left = pixel_right - 1.0;
    let pixel_top = pixel_bottom - 1.0;

    var nearest_x: f32;
    var nearest_y: f32;
    var rr_near: f32;
    if (pixel_left < 0.0 && pixel_right > 0.0 && pixel_top < 0.0 && pixel_bottom > 0.0) {
        // Dab's centre is inside this pixel.
        nearest_x = 0.0;
        nearest_y = 0.0;
        rr_near = 0.0;
    } else {
        // Closest point of the dab's major axis to the pixel centre, clamped
        // into the pixel (closest_point_to_line + CLAMP, inlined).
        let l2 = cs * cs + sn * sn;
        let t = (pixel_center_x * cs + pixel_center_y * sn) / l2;
        nearest_x = clamp(cs * t, pixel_left, pixel_right);
        nearest_y = clamp(sn * t, pixel_top, pixel_bottom);
        rr_near = rr_of(nearest_x, nearest_y, aspect, sn, cs, one_over_radius2);
    }
    if (rr_near > 1.0) { return rr_near; }

    // Which side of the axis the pixel centre is on decides the direction of
    // the farthest point (sign_point_in_line(pcx, pcy, cs, -sn), inlined).
    let center_sign = (pixel_center_x - cs) * sn - cs * (pixel_center_y + sn);
    let rad_area_1 = sqrt(1.0 / 3.141592653589793);
    var farthest_x: f32;
    var farthest_y: f32;
    if (center_sign < 0.0) {
        farthest_x = nearest_x - sn * rad_area_1;
        farthest_y = nearest_y + cs * rad_area_1;
    } else {
        farthest_x = nearest_x + sn * rad_area_1;
        farthest_y = nearest_y - cs * rad_area_1;
    }
    // The skip-test compares the UNSCALED r_far (calculate_r_sample) against
    // r_aa_start — not rr_far (which folds in 1/radius²).
    let r_far = r_of(farthest_x, farthest_y, aspect, sn, cs);
    let rr_far = r_far * one_over_radius2;
    if (r_far < r_aa_start) {
        return (rr_far + rr_near) * 0.5;
    }
    let visibility_near = (1.0 - rr_near) / (1.0 + (rr_far - rr_near));
    return 1.0 - visibility_near;
}

// The dab profile in 0..1 — UNQUANTIZED. The C multiplies the texture-tip
// mask into the f32 opa before the u16 quantization (render_dab_mask), so
// the caller applies the texture first and quantizes second.
fn mask_of(d: DabG, ox: f32, oy: f32, px: i32, py: i32, hard: bool) -> f32 {
    let hardness = clamp(d.hardness, 0.0, 1.0);
    let cs = cos(d.angle / 360.0 * 6.283185307179586);
    let sn = sin(d.angle / 360.0 * 6.283185307179586);
    let one_over_radius2 = 1.0 / (d.radius * d.radius);
    var rr: f32;
    if (d.radius < 3.0) {
        let aa_border = 1.0;
        var r_aa_start = select(0.0, d.radius - aa_border, d.radius > aa_border);
        r_aa_start = r_aa_start * r_aa_start / d.aspect;
        rr = rr_antialiased(
            f32(px), f32(py), ox, oy, d.aspect, sn, cs, one_over_radius2, r_aa_start,
        );
    } else {
        let yy = f32(py) + 0.5 - oy;
        let xx = f32(px) + 0.5 - ox;
        rr = rr_of(xx, yy, d.aspect, sn, cs, one_over_radius2);
    }
    if (hard) {
        // Round 25 hard stamp: exact AA disc — distance inside the edge in px.
        return clamp(d.radius * (1.0 - rr) + 0.5, 0.0, 1.0);
    }
    // calculate_opa: two linear segments meeting at the hardness knot.
    let segment1_offset = 1.0;
    let segment1_slope = -(1.0 / hardness - 1.0);
    let segment2_offset = hardness / (1.0 - hardness);
    let segment2_slope = -hardness / (1.0 - hardness);
    var opa: f32;
    if (rr <= hardness) {
        opa = segment1_offset + rr * segment1_slope;
    } else {
        opa = segment2_offset + rr * segment2_slope;
    }
    if (rr > 1.0) { opa = 0.0; }
    return clamp(opa, 0.0, 1.0);
}

@compute @workgroup_size(16, 16)
fn main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
) {
    // 16x16 threads x 4x4 px = the 64x64 tile.
    let bx = i32(gid.x) * 4;
    let by = i32(gid.y) * 4;

    // Load this thread's block once.
    var px: array<vec4<u32>, 16>;
    for (var i = 0u; i < 16u; i++) {
        let x = bx + i32(i % 4u);
        let y = by + i32(i / 4u);
        px[i] = textureLoad(dst_src, vec2<i32>(x, y), 0);
    }

    let hard = (tile.flags & 1u) != 0u;
    for (var di = 0u; di < tile.dab_count; di++) {
        let d = dabs[di];
        // Tile-local centre; skip dabs whose bbox misses the whole tile
        // (r_fringe = radius + 1, as the C's tile-range math).
        let lx = d.x - f32(tile.ox);
        let ly = d.y - f32(tile.oy);
        // #10 amendment 3: an anchored stamp rotates a square — sqrt(2) reach.
        let stamp = tile.tex_size > 0u && (tile.flags & 2u) != 0u;
        let fringe = select(d.radius + 1.0, d.radius * 1.41421356 + 1.0, stamp);
        if (lx + fringe < f32(bx) || lx - fringe > f32(bx + 4) ||
            ly + fringe < f32(by) || ly - fringe > f32(by + 4))
        {
            continue;
        }
        for (var i = 0u; i < 16u; i++) {
            let x = bx + i32(i % 4u);
            let y = by + i32(i / 4u);
            // #10 amendment 3: PURE STAMP — anchored mode takes coverage
            // from the tip sample alone; no radial profile, no hard disc.
            var opa = select(mask_of(d, lx, ly, x, y, hard), 1.0, stamp);
            // Texture tips (#0.1) — the C's canvas-anchored multiply, exact
            // order: profile f32 × mask/255 BEFORE the u16 quantization. The
            // mod mirrors C's truncating % plus the negative fixup; scroll
            // offsets arrive per dab (tex_u/tex_v, the crawl accumulator
            // int-cast at record time).
            if (tile.tex_size > 0u) {
                if ((tile.flags & 2u) != 0u) {
                    // #10 amendment 2: dab-anchored stamp — the mask covers
                    // the dab's bounding square, rotated by the dab's OWN
                    // unfolded stamp angle (the precomputed tex_sn/tex_cs,
                    // NOT the folded elliptical angle), in the profile's
                    // frame conventions (xxr right, yyr down, +0.5 pixel
                    // centres); outside its square the stamp is over, not
                    // wrapped.
                    let cs = d.tex_cs;
                    let sn = d.tex_sn;
                    let xx = f32(x) + 0.5 - lx;
                    let yy = f32(y) + 0.5 - ly;
                    let xxr = yy * sn + xx * cs;
                    let yyr = yy * cs - xx * sn;
                    let u = (xxr / d.radius * 0.5 + 0.5) * f32(tile.tex_size);
                    let v = (yyr / d.radius * 0.5 + 0.5) * f32(tile.tex_size);
                    if (u < 0.0 || v < 0.0
                        || u >= f32(tile.tex_size) || v >= f32(tile.tex_size)) {
                        opa = 0.0;
                    } else {
                        // BILINEAR, texel centres at +0.5 — the exact
                        // arithmetic of the C and the repair rasterizer.
                        let uf = u - 0.5;
                        let vf = v - 0.5;
                        let u0f = floor(uf);
                        let v0f = floor(vf);
                        let fu = uf - u0f;
                        let fv = vf - v0f;
                        let hi = i32(tile.tex_size) - 1;
                        let u0 = clamp(i32(u0f), 0, hi);
                        let v0 = clamp(i32(v0f), 0, hi);
                        let u1 = clamp(i32(u0f) + 1, 0, hi);
                        let v1 = clamp(i32(v0f) + 1, 0, hi);
                        let g00 = f32(textureLoad(tex, vec2<i32>(u0, v0), 0).r);
                        let g10 = f32(textureLoad(tex, vec2<i32>(u1, v0), 0).r);
                        let g01 = f32(textureLoad(tex, vec2<i32>(u0, v1), 0).r);
                        let g11 = f32(textureLoad(tex, vec2<i32>(u1, v1), 0).r);
                        let g = g00 * (1.0 - fu) * (1.0 - fv) + g10 * fu * (1.0 - fv)
                            + g01 * (1.0 - fu) * fv + g11 * fu * fv;
                        opa = opa * g / 255.0;
                    }
                } else {
                    let n = i32(tile.tex_size);
                    var ui = (tile.ox + x + d.tex_u) % n;
                    if (ui < 0) { ui = ui + n; }
                    var vi = (tile.oy + y + d.tex_v) % n;
                    if (vi < 0) { vi = vi + n; }
                    let g = f32(textureLoad(tex, vec2<i32>(ui, vi), 0).r) / 255.0;
                    opa = opa * g;
                }
            }
            let mask = u32(opa * 32768.0);
            if (mask == 0u) { continue; }
            var rgba = px[i];

            // --- draw_dab_pixels_BlendMode_Normal (exactly the C u32 math) ---
            if ((d.flags & 1u) == 0u && d.opa_normal > 0u) {
                // colour_a == 1
                var opa_a = mask * d.opa_normal / FIX15;
                let opa_b = FIX15 - opa_a;
                rgba.w = opa_a + opa_b * rgba.w / FIX15;
                rgba.x = (opa_a * d.color_r + opa_b * rgba.x) / FIX15;
                rgba.y = (opa_a * d.color_g + opa_b * rgba.y) / FIX15;
                rgba.z = (opa_a * d.color_b + opa_b * rgba.z) / FIX15;
            } else if ((d.flags & 1u) != 0u && d.opa_normal > 0u) {
                // Normal_and_Eraser (colour_a < 1): opa_b uses the UNSCALED
                // opa_a — the C computes it before folding colour_a in.
                let opa_pre = mask * d.opa_normal / FIX15;
                let opa_b = FIX15 - opa_pre;
                let opa_a = opa_pre * d.color_a / FIX15;
                rgba.w = opa_a + opa_b * rgba.w / FIX15;
                rgba.x = (opa_a * d.color_r + opa_b * rgba.x) / FIX15;
                rgba.y = (opa_a * d.color_g + opa_b * rgba.y) / FIX15;
                rgba.z = (opa_a * d.color_b + opa_b * rgba.z) / FIX15;
            }

            // --- LockAlpha (separate stamp, alpha untouched) ---
            if ((d.flags & 2u) != 0u && d.opa_lock > 0u) {
                let opa_a0 = mask * d.opa_lock / FIX15;
                let opa_b = FIX15 - opa_a0;
                let opa_a = opa_a0 * rgba.w / FIX15;
                rgba.x = (opa_a * d.color_r + opa_b * rgba.x) / FIX15;
                rgba.y = (opa_a * d.color_g + opa_b * rgba.y) / FIX15;
                rgba.z = (opa_a * d.color_b + opa_b * rgba.z) / FIX15;
            }

            // --- Colorize (draw_dab_pixels_BlendMode_Color): de-premult,
            // set the pixel's luminance-preserving hue/sat from the brush
            // colour (set_rgb16_lum_from_rgb16 — float LUMA products,
            // truncating i32 divisions, BT.601 coeffs), re-premult, blend
            // rgb only. Alpha untouched. The clip divisions are guarded
            // against the all-equal degenerate case, like the CPU mirror.
            if (d.opa_colorize > 0u) {
                let a = rgba.w;
                var sr = 0u; var sg = 0u; var sb = 0u;
                if (a != 0u) {
                    sr = FIX15 * rgba.x / a;
                    sg = FIX15 * rgba.y / a;
                    sb = FIX15 * rgba.z / a;
                }
                let botlum = i32(luma(i32(sr), i32(sg), i32(sb)) / 32768.0);
                let toplum = i32(
                    luma(i32(d.color_r), i32(d.color_g), i32(d.color_b)) / 32768.0);
                let diff = botlum - toplum;
                var r = i32(d.color_r) + diff;
                var g = i32(d.color_g) + diff;
                var b = i32(d.color_b) + diff;
                let lum = i32(luma(r, g, b) / 32768.0);
                let cmin = min(r, min(g, b));
                let cmax = max(r, max(g, b));
                if (cmin < 0 && lum != cmin) {
                    r = lum + ((r - lum) * lum) / (lum - cmin);
                    g = lum + ((g - lum) * lum) / (lum - cmin);
                    b = lum + ((b - lum) * lum) / (lum - cmin);
                }
                if (cmax > 32768 && cmax != lum) {
                    r = lum + ((r - lum) * (32768 - lum)) / (cmax - lum);
                    g = lum + ((g - lum) * (32768 - lum)) / (cmax - lum);
                    b = lum + ((b - lum) * (32768 - lum)) / (cmax - lum);
                }
                let pr = u32(r) * a / FIX15;
                let pg = u32(g) * a / FIX15;
                let pb = u32(b) * a / FIX15;
                let opa_a = mask * d.opa_colorize / FIX15;
                let opa_b = FIX15 - opa_a;
                rgba.x = (opa_a * pr + opa_b * rgba.x) / FIX15;
                rgba.y = (opa_a * pg + opa_b * rgba.y) / FIX15;
                rgba.z = (opa_a * pb + opa_b * rgba.z) / FIX15;
            }

            // --- Posterize (draw_dab_pixels_BlendMode_Posterize): quantize
            // the premultiplied rgb to poster_num levels (ROUND = trunc of
            // x + 0.5, then all-integer), blend at the stamp opacity.
            if (d.opa_posterize > 0u) {
                let n = d.poster_num;
                let fr = f32(rgba.x) / 32768.0;
                let fg = f32(rgba.y) / 32768.0;
                let fb = f32(rgba.z) / 32768.0;
                let pr = 32768u * u32(fr * f32(n) + 0.5) / n;
                let pg = 32768u * u32(fg * f32(n) + 0.5) / n;
                let pb = 32768u * u32(fb * f32(n) + 0.5) / n;
                let opa_a = mask * d.opa_posterize / FIX15;
                let opa_b = FIX15 - opa_a;
                rgba.x = (opa_a * pr + opa_b * rgba.x) / FIX15;
                rgba.y = (opa_a * pg + opa_b * rgba.y) / FIX15;
                rgba.z = (opa_a * pb + opa_b * rgba.z) / FIX15;
            }

            px[i] = rgba;
        }
    }

    for (var i = 0u; i < 16u; i++) {
        let x = bx + i32(i % 4u);
        let y = by + i32(i / 4u);
        textureStore(dst, vec2<i32>(x, y), px[i]);
    }

    // One bump per WORKGROUP. Keyed on the local id, not the global one: with
    // today's `dispatch_workgroups(1,1,1)` the two are identical, but if a
    // flush ever dispatches more than one workgroup, a global-id test would
    // fire only in workgroup 0 and the driver defense would silently stop
    // counting the rest (audit 2026-08-17, finding L2).
    if (lid.x == 0u && lid.y == 0u && lid.z == 0u) {
        atomicAdd(&canary, 1u);
    }
}
