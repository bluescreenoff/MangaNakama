// The shared tile-kernel compute shader — see `crates/gpu/src/kernel.rs`.
//
// Two entry points over one binding layout:
//
//   adjust_main  — the pointwise colour family (`mn_core::Adjust`), one
//                  invocation per pixel, tiles laid end to end in `src`.
//   sep_main     — one axis of a separable symmetric convolution (the blur
//                  family), one invocation per pixel of a region.
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
    /// Half-kernel length (`taps - 1` is the reach) — `sep_main` only.
    taps: u32,
    /// u32 index in `src` where the per-tile coverage bytes start, or 0 for
    /// "no window". Four coverage bytes per u32, tile-major.
    cov_base: u32,
    /// Control-point / stop count for the curve and map ops.
    n: u32,
    /// This pass's integer divisor — `mn_core::BoxPass::denom`.
    denom: u32,
    /// Index of this pass's centre weight in `weights`.
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
/// Every separable pass's integer half-kernel, concatenated; `P.k_base`
/// picks this pass's. A storage binding rather than more uniform block
/// because the widest legal blur (`Filter::MAX_SIGMA`) needs 750 taps and
/// the uniform block has to stay inside one dynamic-offset stride.
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
