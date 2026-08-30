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
    // bit0: colour_a < 1 (Normal_and_Eraser); bit1: LockAlpha applies;
    // bit2: the spectral Normal/Eraser_Paint arm is called (paint > 0 and
    // op->normal nonzero — the C's CALL condition, kept separate from
    // opa_paint because Normal_Paint clamps its opacity up to 150 even when
    // the u16 conversion rounded it to 0); bit3: LockAlpha_Paint is called.
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
    // count (1..=128, C-clamped). 22 scalars = 88 bytes, matching Rust's
    // `GpuDab` exactly; the array stride must agree or dabs[1..] read
    // misaligned.
    opa_colorize: u32,
    opa_posterize: u32,
    poster_num: u32,
    // Spectral-paint stamp opacities, fix15 (the paint>0 half of process_op):
    //   opa_paint      = normal   *opaque*paint * 32768   (normal = the
    //                    (1-lock)(1-colorize)(1-posterize) knob)
    //   opa_lock_paint = lock_alpha*opaque*(1-colorize)(1-posterize)*paint
    opa_paint: u32,
    opa_lock_paint: u32,
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

// --- Spectral paint (the WGM pigment engine, brushmodes.c *_Paint arms) ---
//
// The C reference mixes through a 10-band spectral upsampling with a
// weighted geometric mean, and its pow is NOT libm's — it is fastapprox's
// `fastpow` (Mineiro), a bit-trick approximation. The port below reproduces
// those bit tricks exactly (bitcast for the union casts, i32()/u32() for the
// C's truncating conversions); a real pow() here would be MORE accurate and
// fail the <=1 parity bar against the C rasterizer.

const WGM_EPSILON: f32 = 0.001;
// helpers.c: spectral_r_small / spectral_g_small / spectral_b_small.
const SPEC_R = array<f32, 10>(
    0.009281362787953, 0.009732627042016, 0.011254252737167,
    0.015105578649573, 0.024797924177217, 0.083622585502406,
    0.977865045723212, 1.0, 0.999961046144372, 0.999999992756822);
const SPEC_G = array<f32, 10>(
    0.002854127435775, 0.003917589679914, 0.012132151699187,
    0.748259205918013, 1.0, 0.865695937531795,
    0.037477469241101, 0.022816789725717, 0.021747419446456,
    0.021384940572308);
const SPEC_B = array<f32, 10>(
    0.537052150373386, 0.546646402401469, 0.575501819073983,
    0.258778829633924, 0.041709923751716, 0.012662638828324,
    0.007485593127390, 0.006766900622462, 0.006699764779016,
    0.006676219883241);
// helpers.c: T_MATRIX_SMALL, row-major.
const T_ROW0 = array<f32, 10>(
    0.026595621243689, 0.049779426257903, 0.022449850859496,
    -0.218453689278271, -0.256894883201278, 0.445881722194840,
    0.772365886289756, 0.194498761382537, 0.014038157587820,
    0.007687264480513);
const T_ROW1 = array<f32, 10>(
    -0.032601672674412, -0.061021043498478, -0.052490001018404,
    0.206659098273522, 0.572496335158169, 0.317837248815438,
    -0.021216624031211, -0.019387668756117, -0.001521339050858,
    -0.000835181622534);
const T_ROW2 = array<f32, 10>(
    0.339475473216284, 0.635401374177222, 0.771520797089589,
    0.113222640692379, -0.055251113343776, -0.048222578468680,
    -0.012966666339586, -0.001523814504223, -0.000094718948810,
    -0.000051604594741);

// fastapprox fastlog2: exponent bits read as a float plus a rational
// correction on the mantissa (fastlog.h, bit for bit).
fn fastlog2(x: f32) -> f32 {
    let vx = bitcast<u32>(x);
    let mx = bitcast<f32>((vx & 0x007FFFFFu) | 0x3f000000u);
    let y = f32(vx) * 1.1920928955078125e-7;
    return y - 124.22551499 - 1.498030302 * mx - 1.72587999 / (0.3520887068 + mx);
}

// fastapprox fastpow2 (fastexp.h): the (1<<23)*(...) float built straight
// into the exponent field. i32()/u32() truncate toward zero like C casts.
fn fastpow2(p: f32) -> f32 {
    let offset = select(0.0, 1.0, p < 0.0);
    let clipp = select(p, -126.0, p < -126.0);
    let w = i32(clipp);
    let z = clipp - f32(w) + offset;
    let e = 8388608.0 * (clipp + 121.2740575 + 27.7280233 / (4.84252568 - z) - 1.49012907 * z);
    return bitcast<f32>(u32(e));
}

fn fastpow(x: f32, p: f32) -> f32 {
    return fastpow2(p * fastlog2(x));
}

// helpers.c rgb_to_spectral: straight rgb 0..1 upsampled to 10 reflectance
// bands. Sum order matches the C ((r-term + g-term) + b-term).
fn rgb_to_spectral(r0: f32, g0: f32, b0: f32) -> array<f32, 10> {
    let off = 1.0 - WGM_EPSILON;
    let r = r0 * off + WGM_EPSILON;
    let g = g0 * off + WGM_EPSILON;
    let b = b0 * off + WGM_EPSILON;
    // Local copies: dynamic indexing needs an addressable array.
    var sr = SPEC_R;
    var sg = SPEC_G;
    var sb = SPEC_B;
    var out: array<f32, 10>;
    for (var i = 0; i < 10; i++) {
        out[i] = sr[i] * r + sg[i] * g + sb[i] * b;
    }
    return out;
}

// helpers.c spectral_to_rgb: 3x10 matrix, sequential accumulation like the
// C loop, then the epsilon un-offset and clamp.
fn spectral_to_rgb(spec: array<f32, 10>) -> vec3<f32> {
    let off = 1.0 - WGM_EPSILON;
    var t0 = T_ROW0;
    var t1 = T_ROW1;
    var t2 = T_ROW2;
    var s = spec;
    var tmp = vec3<f32>(0.0);
    for (var i = 0; i < 10; i++) {
        tmp.x += t0[i] * s[i];
        tmp.y += t1[i] * s[i];
        tmp.z += t2[i] * s[i];
    }
    return clamp((tmp - vec3<f32>(WGM_EPSILON)) / off, vec3<f32>(0.0), vec3<f32>(1.0));
}

// brushmodes.c spectral_blend_factor: the sigmoid-ish additive->spectral
// fade the eraser-paint arm runs on canvas alpha.
fn spectral_blend_factor(x: f32) -> f32 {
    let b = x * 8.0 - 3.0;
    return 0.5 + b / (1.0 + abs(b) * 1.65);
}

// The WGM mix at one pixel: spectral_result[i] = a[i]^fac_a * b[i]^fac_b,
// converted back to rgb. Factored out because all three paint arms run it.
fn wgm_mix(spec_a: array<f32, 10>, spec_b: array<f32, 10>, fac_a: f32) -> vec3<f32> {
    let fac_b = 1.0 - fac_a;
    var a = spec_a;
    var b = spec_b;
    var mixed: array<f32, 10>;
    for (var i = 0; i < 10; i++) {
        mixed[i] = fastpow(a[i], fac_a) * fastpow(b[i], fac_b);
    }
    return spectral_to_rgb(mixed);
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

            // --- Spectral paint arms (the paint>0 half of process_op).
            // Dispatch order matches the C: after the (1-paint) Normal +
            // LockAlpha stamps, before Colorize/Posterize.
            if ((d.flags & 4u) != 0u) {
                // spectral_a of the straight brush colour — the C hoists it
                // out of the pixel loop; recomputing per pixel is the same
                // numbers (pure function of the dab).
                let spec_a = rgb_to_spectral(
                    f32(d.color_r) / 32768.0,
                    f32(d.color_g) / 32768.0,
                    f32(d.color_b) / 32768.0,
                );
                if ((d.flags & 1u) == 0u) {
                    // draw_dab_pixels_BlendMode_Normal_Paint (colour_a == 1).
                    // The C clamps a too-low stamp opacity up to 150 — int->
                    // float->int rounding artifacts, its comment says.
                    let opacity = max(d.opa_paint, 150u);
                    let opa_a = mask * opacity / FIX15;
                    let opa_b = FIX15 - opa_a;
                    if (rgba.w == 0u) {
                        // Nothing to mix with: plain additive, the C's
                        // zero-alpha shortcut.
                        rgba.w = opa_a + opa_b * rgba.w / FIX15;
                        rgba.x = (opa_a * d.color_r + opa_b * rgba.x) / FIX15;
                        rgba.y = (opa_a * d.color_g + opa_b * rgba.y) / FIX15;
                        rgba.z = (opa_a * d.color_b + opa_b * rgba.z) / FIX15;
                    } else {
                        let fac_a = f32(opa_a) / f32(opa_a + opa_b * rgba.w / FIX15);
                        let spec_b = rgb_to_spectral(
                            f32(rgba.x) / f32(rgba.w),
                            f32(rgba.y) / f32(rgba.w),
                            f32(rgba.z) / f32(rgba.w),
                        );
                        let rgb = wgm_mix(spec_a, spec_b, fac_a);
                        // Alpha first — the C re-premultiplies with the NEW
                        // alpha; the +0.5 is its round-on-store.
                        rgba.w = opa_a + opa_b * rgba.w / FIX15;
                        rgba.x = u32(rgb.x * f32(rgba.w) + 0.5);
                        rgba.y = u32(rgb.y * f32(rgba.w) + 0.5);
                        rgba.z = u32(rgb.z * f32(rgba.w) + 0.5);
                    }
                } else {
                    // draw_dab_pixels_BlendMode_Normal_and_Eraser_Paint: no
                    // min-opacity clamp; additive and spectral cross-fade on
                    // the canvas alpha (the low-alpha artifact patch).
                    let opa_a = mask * d.opa_paint / FIX15;
                    let opa_b = FIX15 - opa_a;
                    let opa_a2 = opa_a * d.color_a / FIX15;
                    let opa_out = opa_a2 + opa_b * rgba.w / FIX15;
                    let sf = clamp(spectral_blend_factor(f32(rgba.w) / 32768.0), 0.0, 1.0);
                    let af = 1.0 - sf;
                    var outc = vec3<u32>(0u);
                    if (af != 0.0) {
                        outc.x = (opa_a2 * d.color_r + opa_b * rgba.x) / FIX15;
                        outc.y = (opa_a2 * d.color_g + opa_b * rgba.y) / FIX15;
                        outc.z = (opa_a2 * d.color_b + opa_b * rgba.z) / FIX15;
                    }
                    if (sf != 0.0 && rgba.w != 0u) {
                        let spec_b = rgb_to_spectral(
                            f32(rgba.x) / f32(rgba.w),
                            f32(rgba.y) / f32(rgba.w),
                            f32(rgba.z) / f32(rgba.w),
                        );
                        var fac_a = f32(opa_a) / f32(opa_a + opa_b * rgba.w / FIX15);
                        fac_a = fac_a * (f32(d.color_a) / 32768.0);
                        let rgb = wgm_mix(spec_a, spec_b, fac_a);
                        // The C's combine, float then truncate on store.
                        outc.x = u32(af * f32(outc.x) + sf * rgb.x * f32(opa_out));
                        outc.y = u32(af * f32(outc.y) + sf * rgb.y * f32(opa_out));
                        outc.z = u32(af * f32(outc.z) + sf * rgb.z * f32(opa_out));
                    }
                    rgba.w = opa_out;
                    rgba.x = outc.x;
                    rgba.y = outc.y;
                    rgba.z = outc.z;
                }
            }
            if ((d.flags & 8u) != 0u) {
                // draw_dab_pixels_BlendMode_LockAlpha_Paint. NOTE the C DOES
                // rewrite alpha here (opa_a was pre-scaled by it, so the
                // rewrite is a truncating near-identity) — mirrored, not
                // "fixed".
                let opacity = max(d.opa_lock_paint, 150u);
                let opa_a0 = mask * opacity / FIX15;
                let opa_b = FIX15 - opa_a0;
                let opa_a = opa_a0 * rgba.w / FIX15;
                if (rgba.w == 0u) {
                    // opa_a is 0 here; the C still runs the rgb blend.
                    rgba.x = (opa_a * d.color_r + opa_b * rgba.x) / FIX15;
                    rgba.y = (opa_a * d.color_g + opa_b * rgba.y) / FIX15;
                    rgba.z = (opa_a * d.color_b + opa_b * rgba.z) / FIX15;
                } else {
                    let spec_a = rgb_to_spectral(
                        f32(d.color_r) / 32768.0,
                        f32(d.color_g) / 32768.0,
                        f32(d.color_b) / 32768.0,
                    );
                    let fac_a = f32(opa_a) / f32(opa_a + opa_b * rgba.w / FIX15);
                    let spec_b = rgb_to_spectral(
                        f32(rgba.x) / f32(rgba.w),
                        f32(rgba.y) / f32(rgba.w),
                        f32(rgba.z) / f32(rgba.w),
                    );
                    let rgb = wgm_mix(spec_a, spec_b, fac_a);
                    rgba.w = opa_a + opa_b * rgba.w / FIX15;
                    rgba.x = u32(rgb.x * f32(rgba.w) + 0.5);
                    rgba.y = u32(rgb.y * f32(rgba.w) + 0.5);
                    rgba.z = u32(rgb.z * f32(rgba.w) + 0.5);
                }
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
