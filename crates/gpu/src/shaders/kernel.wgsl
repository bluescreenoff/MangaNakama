// The shared tile-kernel compute shader — see `crates/gpu/src/kernel.rs`.
//
// Two entry points over one binding layout:
//
//   adjust_main  — the pointwise colour family (`mn_core::Adjust`), one
//                  invocation per pixel, tiles laid end to end in `src`.
//   sep_main     — one axis of a separable symmetric convolution (the blur
//                  family), one invocation per pixel of a region.
//   smear_main   — the radial / spin smear: n bilinear taps at affine
//                  offsets about a centre, averaged. One invocation per
//                  pixel of a region.
//   free_main    — the freeform gradient (`FI-050`): distances to two
//                  culled guide polylines, the eased ratio, the ramp, and
//                  src-over onto the tile. Pointwise but POSITION-DEPENDENT,
//                  so unlike `adjust_main` it carries a per-tile header.
//
// PIXEL FORMAT. Everything is premultiplied fix15 RGBA (0..32768) in u16,
// packed two channels per u32 exactly as the CPU tiles are laid out in
// memory: `src[p*2]` = r | g<<16, `src[p*2 + 1]` = b | a<<16. Buffers, not
// textures, on purpose (kernel.rs records why).
//
// PARITY. Every expression here is a transcription of the Rust reference —
// `Adjust::map` / `correct_tile` in core's adjust.rs, `box_h`/`box_v` in
// filter.rs — in the same order, so the only difference is f32 rounding.
// Changing one side without the other is what the parity tests exist to
// catch. The tone curve's Fritsch–Carlson tangents are NOT recomputed here:
// `mn_core::adjust::curve_tangents` produces them on the CPU and they arrive
// in `pts[i].z`, so there is exactly one limiter implementation in the tree.
//
// CANARY. Every workgroup bumps `canary` once; the host compares it with the
// dispatch count after readback. A driver that silently drops a dispatch (the
// cursed-iGPU trap the dab path already guards) therefore fails the compare
// and the host falls back to the CPU reference rather than writing a torn
// tile. Same defence, same reason.

const FIX15_ONE: f32 = 32768.0;
// core::adjust::LUMA, spelled the same way (integer numerators / 32768).
const LUMA: vec3<f32> = vec3<f32>(6967.0 / 32768.0, 23435.0 / 32768.0, 2366.0 / 32768.0);
// `mn_core::tile` geometry — `free_main` maps an invocation to a canvas
// pixel through them.
const TILE_SIDE: u32 = 64u;
const TILE_PIXELS: u32 = 4096u;
// `mn_core::freeform::SEG_WORDS`.
const SEG_WORDS: u32 = 5u;

// Op ids — must match `kernel.rs`'s `op_id`.
const OP_BRIGHTNESS: u32 = 0u;
const OP_HUESAT: u32 = 1u;
const OP_POSTERIZE: u32 = 2u;
const OP_INVERT: u32 = 3u;
const OP_BINARIZE: u32 = 4u;
const OP_LEVELS: u32 = 5u;
const OP_TONECURVE: u32 = 6u;
const OP_BALANCE: u32 = 7u;
const OP_GRADMAP: u32 = 8u;

struct Params {
    /// Which colour op (`OP_*`); ignored by `sep_main`.
    op: u32,
    /// Pixels this dispatch covers.
    count: u32,
    /// Region width / height — `sep_main` only.
    w: u32,
    h: u32,
    /// 0 = horizontal, 1 = vertical — `sep_main` only.
    axis: u32,
    /// Half-kernel length (`taps - 1` is the reach) — `sep_main`.
    /// `free_main` reuses it: the word in `weights` where the SEGMENT POOL
    /// starts.
    taps: u32,
    /// u32 index in `src` where the per-tile coverage bytes start, or 0 for
    /// "no window". Four coverage bytes per u32, tile-major.
    /// `free_main` reuses it: the word in `weights` where the RAMP STOP
    /// TABLE starts.
    cov_base: u32,
    /// Control-point / stop count for the curve and map ops; the SAMPLE
    /// count for `smear_main`; the RAMP STOP count for `free_main`.
    n: u32,
    /// This pass's integer divisor — `mn_core::BoxPass::denom`.
    /// `free_main` reuses it: 1 when the ramp is flipped (`G-002`).
    denom: u32,
    /// Index of this pass's centre weight in `weights`.
    /// `free_main` reuses it: the word where the PER-TILE HEADERS start.
    k_base: u32,
    pad0: u32,
    pad1: u32,
    /// Op scalars. Which slot means what is spelled out in `kernel.rs`.
    a: vec4<f32>,
    b: vec4<f32>,
    /// Tone curve: (x, y, tangent, _). Gradient map: (pos, r, g, b), sorted.
    pts: array<vec4<f32>, 8>,
    /// Pads the block to the 256-byte dynamic-offset stride the host writes.
    pad2: array<vec4<f32>, 3>,
}

@group(0) @binding(0) var<storage, read> src: array<u32>;
@group(0) @binding(1) var<storage, read_write> dst: array<u32>;
@group(0) @binding(2) var<storage, read_write> canary: atomic<u32>;
@group(0) @binding(3) var<uniform> P: Params;
/// The pass's coefficient table. For `sep_main`: every separable pass's
/// integer half-kernel, concatenated, with `P.k_base` picking this pass's. A
/// storage binding rather than more uniform block because the widest legal
/// blur (`Filter::MAX_SIGMA`) needs 750 taps and the uniform block has to
/// stay inside one dynamic-offset stride. For `smear_main`: the sample
/// matrices as raw f32 bits, four words each — four storage bindings is the
/// downlevel ceiling and all four are already spoken for, so there was never
/// a fifth for a float table. For `free_main`: three tables end to end in
/// the same buffer for the same reason — the ramp's stop table at
/// `P.cov_base`, the per-tile headers at `P.k_base` (six words: origin x,
/// origin y, then each guide's segment base and length), and the segment
/// pool at `P.taps`.
@group(0) @binding(4) var<storage, read> weights: array<u32>;

fn load_px(p: u32) -> vec4<u32> {
    let lo = src[p * 2u];
    let hi = src[p * 2u + 1u];
    return vec4<u32>(lo & 0xFFFFu, lo >> 16u, hi & 0xFFFFu, hi >> 16u);
}

fn store_px(p: u32, v: vec4<u32>) {
    dst[p * 2u] = (v.x & 0xFFFFu) | (v.y << 16u);
    dst[p * 2u + 1u] = (v.z & 0xFFFFu) | (v.w << 16u);
}

/// `mn_core::blend::f32_to_fix15` — clamp, scale, round half up, cap.
fn to_fix15(v: f32) -> u32 {
    let c = clamp(v, 0.0, 1.0);
    return u32(min(c * FIX15_ONE + 0.5, FIX15_ONE));
}

// ----------------------------------------------------------- colour ops --

/// `rgb_to_hsv`. The `%` in the red branch is Rust's remainder (sign of the
/// dividend), which WGSL's `%` matches for floats.
fn rgb_to_hsv(c: vec3<f32>) -> vec3<f32> {
    let mx = max(c.r, max(c.g, c.b));
    let mn = min(c.r, min(c.g, c.b));
    let d = mx - mn;
    var h = 0.0;
    if (d > 0.0) {
        if (mx == c.r) {
            h = 60.0 * (((c.g - c.b) / d) % 6.0);
        } else if (mx == c.g) {
            h = 60.0 * ((c.b - c.r) / d + 2.0);
        } else {
            h = 60.0 * ((c.r - c.g) / d + 4.0);
        }
    }
    var s = 0.0;
    if (mx > 0.0) {
        s = d / mx;
    }
    // rem_euclid(360)
    return vec3<f32>(h - 360.0 * floor(h / 360.0), s, mx);
}

fn hsv_to_rgb(hsv: vec3<f32>) -> vec3<f32> {
    let h = hsv.x - 360.0 * floor(hsv.x / 360.0);
    let s = clamp(hsv.y, 0.0, 1.0);
    let v = clamp(hsv.z, 0.0, 1.0);
    let c = v * s;
    let x = c * (1.0 - abs((h / 60.0) % 2.0 - 1.0));
    let m = v - c;
    // `(h / 60.0) as u32` truncates toward zero; h is already in [0, 360).
    let seg = u32(h / 60.0);
    var rgb = vec3<f32>(c, 0.0, x);
    switch (seg) {
        case 0u: { rgb = vec3<f32>(c, x, 0.0); }
        case 1u: { rgb = vec3<f32>(x, c, 0.0); }
        case 2u: { rgb = vec3<f32>(0.0, c, x); }
        case 3u: { rgb = vec3<f32>(0.0, x, c); }
        case 4u: { rgb = vec3<f32>(x, 0.0, c); }
        default: { rgb = vec3<f32>(c, 0.0, x); }
    }
    return rgb + vec3<f32>(m, m, m);
}

/// `curve_eval`, with the tangents already computed (pts[i].z).
fn curve_eval(x: f32) -> f32 {
    let n = P.n;
    if (n == 0u) {
        return clamp(x, 0.0, 1.0);
    }
    if (n == 1u) {
        return clamp(P.pts[0].y, 0.0, 1.0);
    }
    if (x <= P.pts[0].x) {
        return clamp(P.pts[0].y, 0.0, 1.0);
    }
    if (x >= P.pts[n - 1u].x) {
        return clamp(P.pts[n - 1u].y, 0.0, 1.0);
    }
    // `rposition(|p| p[0] <= x)` over pts[..n-1], clamped to n-2.
    var i = 0u;
    for (var k = 0u; k < n - 1u; k = k + 1u) {
        if (P.pts[k].x <= x) {
            i = k;
        }
    }
    let p0 = P.pts[i];
    let p1 = P.pts[i + 1u];
    let h = max(p1.x - p0.x, 1e-6);
    let t = clamp((x - p0.x) / h, 0.0, 1.0);
    let t2 = t * t;
    let t3 = t * t * t;
    let y = (2.0 * t3 - 3.0 * t2 + 1.0) * p0.y
        + (t3 - 2.0 * t2 + t) * h * p0.z
        + (-2.0 * t3 + 3.0 * t2) * p1.y
        + (t3 - t2) * h * p1.z;
    return clamp(y, 0.0, 1.0);
}

/// The gradient map's ramp lookup — `Adjust::map`'s `at()` closure.
///
/// **Single exit, deliberately.** The obvious transcription returns from
/// inside the search loop, the way the Rust does. That compiles correctly on
/// the Intel UHD 620 / DX12 and *miscompiles* on the Windows-10-era WARP
/// (10.0.19041.x): the map came back up to 29490/32768 wrong there while
/// hardware was within one unit. `MN_WARP=1` is what found it, which is the
/// whole argument for running the parity suite on both adapters — a shader
/// that is right on the machine you develop on is not a shader that is
/// right. The loop now writes a `var` and falls out the bottom.
fn gradient_map(luma: f32) -> vec3<f32> {
    let n = P.n;
    if (n == 0u) {
        return vec3<f32>(luma, luma, luma);
    }
    let lo = vec3<f32>(0.0);
    let hi = vec3<f32>(1.0);
    // Past the last stop the last stop's colour stands — the Rust's
    // fall-through, spelled as the initial value.
    var out = clamp(P.pts[n - 1u].yzw, lo, hi);
    if (luma <= P.pts[0].x) {
        out = clamp(P.pts[0].yzw, lo, hi);
    } else {
        var found = false;
        for (var k = 0u; k + 1u < n; k = k + 1u) {
            let a = P.pts[k];
            let b = P.pts[k + 1u];
            if (!found && luma <= b.x) {
                var f = 0.0;
                if (b.x - a.x >= 1e-6) {
                    f = (luma - a.x) / (b.x - a.x);
                }
                out = clamp(a.yzw + (b.yzw - a.yzw) * f, lo, hi);
                found = true;
            }
        }
    }
    return out;
}

/// `Adjust::map` — straight RGB 0..1 in, straight RGB 0..1 out.
fn adjust_map(rgb: vec3<f32>) -> vec3<f32> {
    switch (P.op) {
        case OP_BRIGHTNESS: {
            let c = clamp(P.a.y, -1.0, 0.99);
            let k = (1.0 + c) / (1.0 - c);
            return clamp((rgb - vec3<f32>(0.5)) * k + vec3<f32>(0.5 + P.a.x),
                         vec3<f32>(0.0), vec3<f32>(1.0));
        }
        case OP_HUESAT: {
            var hsv = rgb_to_hsv(rgb);
            let h = hsv.x + P.a.x;
            hsv.x = h - 360.0 * floor(h / 360.0);
            hsv.y = clamp(hsv.y * (1.0 + P.a.y), 0.0, 1.0);
            hsv.z = clamp(hsv.z * (1.0 + P.a.z), 0.0, 1.0);
            return hsv_to_rgb(hsv);
        }
        case OP_POSTERIZE: {
            let n = clamp(P.a.x, 2.0, 256.0);
            let q = min(floor(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)) * n), vec3<f32>(n - 1.0));
            return q / (n - 1.0);
        }
        case OP_INVERT: {
            return vec3<f32>(1.0) - rgb;
        }
        case OP_BINARIZE: {
            let luma = dot(LUMA, rgb);
            if (luma >= P.a.x) {
                return vec3<f32>(1.0);
            }
            return vec3<f32>(0.0);
        }
        case OP_LEVELS: {
            let span = max(P.a.y - P.a.x, 1e-6);
            let inv_g = 1.0 / clamp(P.a.z, 0.1, 10.0);
            let t = pow(clamp((rgb - vec3<f32>(P.a.x)) / span, vec3<f32>(0.0), vec3<f32>(1.0)),
                        vec3<f32>(inv_g));
            return clamp(vec3<f32>(P.a.w) + t * (P.b.x - P.a.w), vec3<f32>(0.0), vec3<f32>(1.0));
        }
        case OP_TONECURVE: {
            return vec3<f32>(curve_eval(rgb.r), curve_eval(rgb.g), curve_eval(rgb.b));
        }
        case OP_BALANCE: {
            let cr = P.a.x;
            let mg = P.a.y;
            let yb = P.a.z;
            return clamp(vec3<f32>(rgb.r + cr - mg * 0.5 - yb * 0.5,
                                   rgb.g + mg - cr * 0.5 - yb * 0.5,
                                   rgb.b + yb - cr * 0.5 - mg * 0.5),
                         vec3<f32>(0.0), vec3<f32>(1.0));
        }
        case OP_GRADMAP: {
            return gradient_map(dot(vec3<f32>(0.2126, 0.7152, 0.0722), rgb));
        }
        default: {
            return rgb;
        }
    }
}

// ------------------------------------------------------------- entries --

@compute @workgroup_size(256)
fn adjust_main(@builtin(global_invocation_id) gid: vec3<u32>,
               @builtin(local_invocation_index) li: u32) {
    if (li == 0u) {
        atomicAdd(&canary, 1u);
    }
    let p = gid.x;
    if (p >= P.count) {
        return;
    }
    let s = load_px(p);

    // `correct_tile`'s coverage: 255 when no window.
    var cov = 255u;
    if (P.cov_base != 0u) {
        let word = src[P.cov_base + p / 4u];
        cov = (word >> ((p % 4u) * 8u)) & 0xFFu;
    }
    let a = s.w;
    if (a == 0u || cov == 0u) {
        store_px(p, s);
        return;
    }
    let inv = 1.0 / f32(a);
    let straight = min(vec3<f32>(f32(s.x), f32(s.y), f32(s.z)) * inv, vec3<f32>(1.0));
    let outc = adjust_map(straight);
    let af = f32(a) / FIX15_ONE;
    store_px(p, vec4<u32>(
        chan(outc.r, af, a, s.x, cov),
        chan(outc.g, af, a, s.y, cov),
        chan(outc.b, af, a, s.z, cov),
        a,
    ));
}

/// One channel of `correct_tile`'s write-back: re-premultiply, never exceed
/// alpha (rounding could), then blend toward the source under partial window
/// coverage — the same integer formula `mask_op_to_selection` uses.
/// Spelled per channel rather than looped so nothing here relies on dynamic
/// vector indexing.
fn chan(out_c: f32, af: f32, a: u32, src_c: u32, cov: u32) -> u32 {
    var v = min(to_fix15(clamp(out_c, 0.0, 1.0) * af), a);
    if (cov != 255u) {
        v = (v * cov + src_c * (255u - cov) + 127u) / 255u;
    }
    return v;
}

/// ONE pass of a separable integer convolution — `box_h` / `box_v` /
/// `tent_h` / `tent_v` in core's filter.rs, gathered instead of running-sum.
///
/// Integer throughout, on purpose: the accumulator, the weights and the
/// `(acc + denom/2) / denom` rounding are the same u32 arithmetic the CPU
/// does, so the results are bit-identical and the parity test asserts
/// equality rather than a tolerance. The widest legal window is
/// `2·250 + 1 = 501` taps of at most 65535, which is 32.8 M — comfortably
/// inside u32.
@compute @workgroup_size(256)
fn sep_main(@builtin(global_invocation_id) gid: vec3<u32>,
            @builtin(local_invocation_index) li: u32) {
    if (li == 0u) {
        atomicAdd(&canary, 1u);
    }
    let p = gid.x;
    if (p >= P.count) {
        return;
    }
    let x = i32(p % P.w);
    let y = i32(p / P.w);
    let reach = i32(P.taps) - 1;
    var acc = vec4<u32>(0u);
    for (var k = -reach; k <= reach; k = k + 1) {
        var sx = x;
        var sy = y;
        if (P.axis == 0u) {
            sx = x + k;
        } else {
            sy = y + k;
        }
        // Outside the buffer counts as transparent, and the denominator
        // stays the FULL window — `box_h`'s convention verbatim. Each pass
        // re-applies it to the previous pass's output, which is why the
        // chain is run pass by pass and not folded into one wide kernel.
        if (sx < 0 || sy < 0 || sx >= i32(P.w) || sy >= i32(P.h)) {
            continue;
        }
        let wgt = weights[P.k_base + u32(abs(k))];
        let s = load_px(u32(sy) * P.w + u32(sx));
        acc = acc + wgt * s;
    }
    let bias = vec4<u32>(P.denom / 2u);
    let r = (acc + bias) / vec4<u32>(P.denom);
    store_px(p, min(r, vec4<u32>(65535u)));
}

/// One corner of a bilinear tap — `mn_core::filter::sample_bilinear`'s inner
/// body, including its two skips (a non-positive weight and an out-of-buffer
/// coordinate both contribute nothing, and outside the region is
/// transparent).
///
/// `wx` and `wy` arrive separately, NOT pre-multiplied, because the Rust
/// spells the product as `p * wx * wy` — left to right — and `(p*wx)*wy` is
/// not always `p*(wx*wy)` in f32. Anything that changes the association here
/// widens the parity gap for no reason.
///
/// **Single exit, and this one is not a style preference.** Written the
/// obvious way — `if (out of range) { return vec4(0.0); }` and then the load
/// — the Windows-10-era WARP (10.0.19041.x) LOSES THE DEVICE on the first
/// dispatch: not a wrong number, a `DEVICE LOST (Unknown)` and a declined
/// job. Hardware (Intel UHD 620 / DX12) runs the same shader perfectly, so
/// only `MN_WARP=1` found it — the second time on this file, after
/// `gradient_map` below. Writing the guarded load into a `var` and falling
/// out the bottom fixes it, which reads like the load being hoisted above
/// its own bounds test and executed with the out-of-range index.
fn tap(xx: i32, yy: i32, wx: f32, wy: f32) -> vec4<f32> {
    var out = vec4<f32>(0.0);
    if (wx > 0.0 && wy > 0.0 && xx >= 0 && yy >= 0 && xx < i32(P.w) && yy < i32(P.h)) {
        out = vec4<f32>(load_px(u32(yy) * P.w + u32(xx))) * wx * wy;
    }
    return out;
}

/// The smear family — `blur.rs`'s `smear`, transcribed.
///
/// `P.n` sample matrices live in `weights` as f32 bits, four words each:
/// the tap for sample i is at `centre + M_i · (p − centre)`. The matrices
/// are built on the HOST (`Filter::smear_samples`) precisely so nothing here
/// evaluates a transcendental — WGSL only promises `sin`/`cos` to 2⁻¹¹
/// absolute, which at a page's radius is most of a pixel of drift, while
/// multiply-add divergence is sub-ULP. What is left is a f32 tolerance
/// argument of the same shape the colour ops already make.
@compute @workgroup_size(256)
fn smear_main(@builtin(global_invocation_id) gid: vec3<u32>,
              @builtin(local_invocation_index) li: u32) {
    if (li == 0u) {
        atomicAdd(&canary, 1u);
    }
    let p = gid.x;
    if (p >= P.count) {
        return;
    }
    let ux = f32(p % P.w) - P.a.x;
    let uy = f32(p / P.w) - P.a.y;
    var acc = vec4<f32>(0.0);
    for (var i = 0u; i < P.n; i = i + 1u) {
        let b = P.k_base + i * 4u;
        let fx = P.a.x + bitcast<f32>(weights[b]) * ux + bitcast<f32>(weights[b + 1u]) * uy;
        let fy = P.a.y + bitcast<f32>(weights[b + 2u]) * ux + bitcast<f32>(weights[b + 3u]) * uy;
        let x0 = floor(fx);
        let y0 = floor(fy);
        let tx = fx - x0;
        let ty = fy - y0;
        let xi = i32(x0);
        let yi = i32(y0);
        // The four corners in the reference's order (row y0 then row y0+1),
        // spelled out rather than looped: a skipped corner adds exactly
        // zero, so summing zeros is the same f32 sum the Rust builds by
        // skipping them. Summed into their OWN value before joining `acc`,
        // because that is the association `sample_bilinear` has — it
        // accumulates one tap from zero and the caller adds the total.
        let s = tap(xi, yi, 1.0 - tx, 1.0 - ty)
              + tap(xi + 1, yi, tx, 1.0 - ty)
              + tap(xi, yi + 1, 1.0 - tx, ty)
              + tap(xi + 1, yi + 1, tx, ty);
        acc = acc + s;
    }
    let v = acc / f32(P.n) + vec4<f32>(0.5);
    store_px(p, vec4<u32>(clamp(v, vec4<f32>(0.0), vec4<f32>(65535.0))));
}

// ------------------------------------------------------- freeform field --

/// `mn_core::freeform::Seg::dist2` — squared distance from `p` to the
/// segment whose five f32 start at word `b` of `weights`. Multiply, add and
/// one `clamp`; no divide (the reciprocal `inv_l2` is precomputed on the
/// host, for the same reason the Rust precomputes it) and no transcendental.
fn seg_dist2(b: u32, px: f32, py: f32) -> f32 {
    let ax = bitcast<f32>(weights[b]);
    let ay = bitcast<f32>(weights[b + 1u]);
    let dx = bitcast<f32>(weights[b + 2u]);
    let dy = bitcast<f32>(weights[b + 3u]);
    let inv = bitcast<f32>(weights[b + 4u]);
    let wx = px - ax;
    let wy = py - ay;
    let t = clamp((wx * dx + wy * dy) * inv, 0.0, 1.0);
    let ex = wx - t * dx;
    let ey = wy - t * dy;
    return ex * ex + ey * ey;
}

/// `freeform::list_dist2` over `len` segments from segment index `base`.
///
/// **Single exit, and every load is inside the loop's own bound.** The
/// `min` is the Rust's `if d < best` — identical for non-NaN, and a NaN
/// cannot reach here (a guide drops non-finite points at construction).
/// A guide always has at least one segment, so `best` is always written;
/// the initial value is `f32` infinity, the Rust's own starting point.
fn list_dist2(base: u32, len: u32, px: f32, py: f32) -> f32 {
    var best = bitcast<f32>(0x7F800000u);
    for (var i = 0u; i < len; i = i + 1u) {
        best = min(best, seg_dist2(P.taps + (base + i) * SEG_WORDS, px, py));
    }
    return best;
}

/// `freeform::param` — the ramp parameter from a pair of distances, with
/// its smoothstep easing. `d1 + d2` is non-negative and finite by
/// construction here; the guard is the Rust's, kept so the two read alike.
fn field_param(d1: f32, d2: f32) -> f32 {
    var t = 0.5;
    let sum = d1 + d2;
    if (sum > 0.0 && sum < bitcast<f32>(0x7F800000u)) {
        t = clamp(d1 / sum, 0.0, 1.0);
    }
    return t * t * (3.0 - 2.0 * t);
}

/// `Ramp::color_at` for the lerp-table case — `Ramp::lerp_table` refuses to
/// produce a table for the two cases that are not one (a non-Standard
/// mixing space, a non-zero mixing rate), so this is only ever asked the
/// question it can answer.
///
/// **Single exit, deliberately, and this file has TWO scars that say why**
/// (see `gradient_map` and `tap` above): the obvious spelling returns from
/// inside the bracket search, which the Windows-10-era WARP either
/// miscompiles or loses the device over. The bracket index is written into
/// a `var` and the walk falls out the bottom.
///
/// The walk itself reproduces the Rust's forward-then-reverse scan on a
/// SORTED table: `i0` is the last entry at or below `t`, the bracket is
/// `i0 → i0 + 1`, and `i0` being the final entry is the Rust's
/// `p1 <= p0 => return c1`.
fn ramp_colour(t: f32) -> vec4<f32> {
    var i0 = 0u;
    for (var i = 0u; i < P.n; i = i + 1u) {
        if (bitcast<f32>(weights[P.cov_base + i * 5u]) <= t) {
            i0 = i;
        }
    }
    let b0 = P.cov_base + i0 * 5u;
    var out = vec4<f32>(
        bitcast<f32>(weights[b0 + 1u]),
        bitcast<f32>(weights[b0 + 2u]),
        bitcast<f32>(weights[b0 + 3u]),
        bitcast<f32>(weights[b0 + 4u]),
    );
    if (i0 + 1u < P.n) {
        let b1 = b0 + 5u;
        let p0 = bitcast<f32>(weights[b0]);
        let p1 = bitcast<f32>(weights[b1]);
        let c1 = vec4<f32>(
            bitcast<f32>(weights[b1 + 1u]),
            bitcast<f32>(weights[b1 + 2u]),
            bitcast<f32>(weights[b1 + 3u]),
            bitcast<f32>(weights[b1 + 4u]),
        );
        if (p1 > p0) {
            // `mix::mix_rgba`'s Standard arm, spelled `a + (b - a) * s` the
            // way the Rust spells it — the association is the parity.
            let s = clamp((t - p0) / (p1 - p0), 0.0, 1.0);
            out = out + (c1 - out) * s;
        } else {
            out = c1;
        }
    }
    return out;
}

/// The freeform gradient — `Document::paint_gradient_freeform`'s inner loop,
/// transcribed: distances to two culled polylines, the eased ratio, the ramp
/// lookup, premultiply, src-over onto the tile that was handed in.
///
/// One invocation per pixel of a tile batch laid end to end, exactly like
/// `adjust_main`. Each tile's own culled segment lists are addressed through
/// a six-word header at `P.k_base` — the host does the culling, because it
/// is per TILE and this is per pixel.
///
/// Off-canvas pixels are written back unchanged, which is what the CPU's
/// `continue` leaves them as.
@compute @workgroup_size(256)
fn free_main(@builtin(global_invocation_id) gid: vec3<u32>,
             @builtin(local_invocation_index) li: u32) {
    if (li == 0u) {
        atomicAdd(&canary, 1u);
    }
    let p = gid.x;
    if (p >= P.count) {
        return;
    }
    let tile = p / TILE_PIXELS;
    let inner = p % TILE_PIXELS;
    let hdr = P.k_base + tile * 6u;
    let x = bitcast<i32>(weights[hdr]) + i32(inner % TILE_SIDE);
    let y = bitcast<i32>(weights[hdr + 1u]) + i32(inner / TILE_SIDE);

    var out = load_px(p);
    if (x < i32(P.w) && y < i32(P.h)) {
        let fx = f32(x) + 0.5;
        let fy = f32(y) + 0.5;
        let d1 = sqrt(list_dist2(weights[hdr + 2u], weights[hdr + 3u], fx, fy));
        let d2 = sqrt(list_dist2(weights[hdr + 4u], weights[hdr + 5u], fx, fy));
        var t = field_param(d1, d2);
        // `Ramp::eval_unit`: clamp, flip, look up. Dithering is the third
        // thing it does and the host declines a dithering ramp outright.
        t = clamp(t, 0.0, 1.0);
        if (P.denom == 1u) {
            t = 1.0 - t;
        }
        let c = ramp_colour(t);
        // Premultiply exactly as the Rust does — `f32_to_fix15(v * a)`
        // capped at the alpha, never `(v * a)` in fix15 space.
        let al = to_fix15(c.w);
        let s = vec4<u32>(
            min(to_fix15(c.x * c.w), al),
            min(to_fix15(c.y * c.w), al),
            min(to_fix15(c.z * c.w), al),
            al,
        );
        // `Document::over_pixel`, integer for integer.
        let inv = 32768u - al;
        out = min(s + ((out * inv + vec4<u32>(16384u)) >> vec4<u32>(15u)),
                  vec4<u32>(32768u));
    }
    store_px(p, out);
}
