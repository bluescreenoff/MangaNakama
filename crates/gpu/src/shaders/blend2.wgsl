// Blend part 2 — the shader compositor pass (docs/DECISIONS.md §3).
//
// The modes fixed-function blending cannot express (wgpu's min/max factor
// rule for Darken/Lighten; the operator families beyond the two-term
// states) composite HERE instead: the fragment samples the layer source
// (tile or group) AND a SNAPSHOT of the destination taken just before this
// pass (a render pass cannot read its own target), computes the exact
// premultiplied formula from mn_core::blend, and writes the full RGBA with
// a REPLACE state.
//
// Mode values arrive in the instance pad (`BLEND2_MODES` in lib.rs — the
// order of that array IS these numbers, so append there, never reorder):
//   16 Darken, 17 Lighten, 18 Overlay, 19 Soft light, 20 Hard light,
//   21 Difference, 22 Exclusion, 23 Hue, 24 Saturation, 25 Color,
//   26 Color burn, 27 Linear burn, 28 Color dodge, 29 Glow dodge,
//   30 Vivid light, 31 Linear light, 32 Pin light, 33 Hard mix, 34 Divide,
//   35 Darker color, 36 Lighter color, 37 Brightness (SVG luminosity),
//   38 Subtract (moved off fixed-function ReverseSubtract so a transparent
//   destination keeps the source — see core::blend's Subtract arm).
// Separable: 16..22, 26..34 and 38. Nonseparable: 23..25 and 35..37.
//
// Same oversized-triangle + scissor geometry as tiles.wgsl (the driver note
// there). Everything is premultiplied f32 in and out, opacity folded into
// the source exactly like tiles.wgsl/group.wgsl do.

struct CanvasUniform {
    size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> canvas: CanvasUniform;

// --- tile variant (group 1 = the tile texture + the snapshot) ------------
@group(1) @binding(0) var tile_tex: texture_2d<u32>;
@group(1) @binding(1) var snap_tex: texture_2d<f32>;

// --- the exact mn_core::blend formulas (premultiplied) --------------------
fn hard_light_op(cs: f32, cb: f32) -> f32 {
    if cs <= 0.5 { return 2.0 * cs * cb; }
    return 1.0 - 2.0 * (1.0 - cs) * (1.0 - cb);
}

fn soft_light_op(cs: f32, cb: f32) -> f32 {
    let dd = select(sqrt(cb), ((16.0 * cb - 12.0) * cb + 4.0) * cb, cb <= 0.25);
    if cs <= 0.5 {
        return cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb);
    }
    return cb + (2.0 * cs - 1.0) * (dd - cb);
}

// --- part 3: the burn/dodge/light family ---------------------------------
// Every one of these is branch-for-branch the twin of the matching arm in
// mn_core::blend, INCLUDING the min-before-divide forms: WGSL leaves division
// by zero implementation defined, so the guards are load-bearing, not tidy.
fn color_burn_op(cs: f32, cb: f32) -> f32 {
    if cb >= 1.0 { return 1.0; }
    if cs <= 0.0 { return 0.0; }
    return 1.0 - min(1.0 - cb, cs) / cs;
}

fn color_dodge_op(cs: f32, cb: f32) -> f32 {
    if cb <= 0.0 { return 0.0; }
    if cs >= 1.0 { return 1.0; }
    return min(cb, 1.0 - cs) / (1.0 - cs);
}

fn glow_dodge_op(cs: f32, cb: f32) -> f32 {
    if cs >= 1.0 { return 1.0; }
    return min(cb + cs, 1.0 - cs) / (1.0 - cs);
}

fn vivid_light_op(cs: f32, cb: f32) -> f32 {
    if cs <= 0.5 { return color_burn_op(2.0 * cs, cb); }
    return color_dodge_op(2.0 * cs - 1.0, cb);
}

fn pin_light_op(cs: f32, cb: f32) -> f32 {
    if cs <= 0.5 { return min(cb, 2.0 * cs); }
    return max(cb, 2.0 * cs - 1.0);
}

fn hard_mix_op(cs: f32, cb: f32) -> f32 {
    return select(0.0, 1.0, cb + cs >= 1.0);
}

fn divide_op(cs: f32, cb: f32) -> f32 {
    if cs <= 0.0 { return 1.0; }
    return min(cb, cs) / cs;
}

fn lum3(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.3, 0.59, 0.11));
}

fn clip_color3(c: vec3<f32>) -> vec3<f32> {
    var o = c;
    let l = lum3(c);
    let n = min(min(c.r, c.g), c.b);
    let x = max(max(c.r, c.g), c.b);
    if n < 0.0 {
        let d = l - n;
        if d != 0.0 {
            o = vec3<f32>(l) + (c - vec3<f32>(l)) * l / d;
        }
    }
    if x > 1.0 {
        let d = x - l;
        if d != 0.0 {
            o = vec3<f32>(l) + (c - vec3<f32>(l)) * (1.0 - l) / d;
        }
    }
    return o;
}

fn set_lum3(c: vec3<f32>, l: f32) -> vec3<f32> {
    return clip_color3(c + vec3<f32>(l - lum3(c)));
}

// SetSat by channel POSITION (the W3C spec form; value-sorting breaks ties).
fn sat3(c: vec3<f32>) -> f32 {
    return max(max(c.r, c.g), c.b) - min(min(c.r, c.g), c.b);
}

fn set_sat3(c: vec3<f32>, s: f32) -> vec3<f32> {
    // WGSL forbids assignment to a dynamically-indexed vector component,
    // so the min/mid/max mapping happens per channel by comparison.
    let lo = min(min(c.r, c.g), c.b);
    let hi = max(max(c.r, c.g), c.b);
    if hi <= lo {
        return vec3<f32>(0.0);
    }
    let mid = c.r + c.g + c.b - lo - hi;
    let m = (mid - lo) * s / (hi - lo);
    return vec3<f32>(
        select(select(m, 0.0, c.r == lo), s, c.r == hi),
        select(select(m, 0.0, c.g == lo), s, c.g == hi),
        select(select(m, 0.0, c.b == lo), s, c.b == hi),
    );
}

fn blend2(s: vec4<f32>, d: vec4<f32>, m: u32) -> vec4<f32> {
    let sa = s.a;
    let da = d.a;
    var out = vec4<f32>(0.0, 0.0, 0.0, sa + da * (1.0 - sa));

    // The general separable frame shared with Darken/Lighten (core::blend):
    //   out.rgb = s.rgb*(1-da) + sa*da*B(cs,cb) + d.rgb*(1-sa)
    if (m >= 16u && m <= 22u) || (m >= 26u && m <= 34u) || m == 38u {
        let cs_ok = select(vec3<f32>(0.0), s.rgb / sa, sa > 0.0);
        let cb_ok = select(vec3<f32>(0.0), d.rgb / da, da > 0.0);
        // Per-mode B (guarding divide-by-zero the same way core does: the
        // B term only lives when both alphas are non-zero).
        var bb = vec3<f32>(0.0);
        if sa > 0.0 && da > 0.0 {
            if m == 16u {
                bb = min(cs_ok, cb_ok);
            } else if m == 17u {
                bb = max(cs_ok, cb_ok);
            } else if m == 18u {
                bb = vec3<f32>(hard_light_op(cb_ok.r, cs_ok.r), hard_light_op(cb_ok.g, cs_ok.g), hard_light_op(cb_ok.b, cs_ok.b));
            } else if m == 19u {
                bb = vec3<f32>(soft_light_op(cs_ok.r, cb_ok.r), soft_light_op(cs_ok.g, cb_ok.g), soft_light_op(cs_ok.b, cb_ok.b));
            } else if m == 20u {
                bb = vec3<f32>(hard_light_op(cs_ok.r, cb_ok.r), hard_light_op(cs_ok.g, cb_ok.g), hard_light_op(cs_ok.b, cb_ok.b));
            } else if m == 21u {
                bb = abs(cb_ok - cs_ok);
            } else if m == 22u {
                bb = cb_ok + cs_ok - 2.0 * cb_ok * cs_ok;
            } else if m == 26u {
                bb = vec3<f32>(color_burn_op(cs_ok.r, cb_ok.r), color_burn_op(cs_ok.g, cb_ok.g), color_burn_op(cs_ok.b, cb_ok.b));
            } else if m == 27u {
                bb = clamp(cb_ok + cs_ok - vec3<f32>(1.0), vec3<f32>(0.0), vec3<f32>(1.0));
            } else if m == 28u {
                bb = vec3<f32>(color_dodge_op(cs_ok.r, cb_ok.r), color_dodge_op(cs_ok.g, cb_ok.g), color_dodge_op(cs_ok.b, cb_ok.b));
            } else if m == 29u {
                bb = vec3<f32>(glow_dodge_op(cs_ok.r, cb_ok.r), glow_dodge_op(cs_ok.g, cb_ok.g), glow_dodge_op(cs_ok.b, cb_ok.b));
            } else if m == 30u {
                bb = vec3<f32>(vivid_light_op(cs_ok.r, cb_ok.r), vivid_light_op(cs_ok.g, cb_ok.g), vivid_light_op(cs_ok.b, cb_ok.b));
            } else if m == 31u {
                bb = clamp(cb_ok + 2.0 * cs_ok - vec3<f32>(1.0), vec3<f32>(0.0), vec3<f32>(1.0));
            } else if m == 32u {
                bb = vec3<f32>(pin_light_op(cs_ok.r, cb_ok.r), pin_light_op(cs_ok.g, cb_ok.g), pin_light_op(cs_ok.b, cb_ok.b));
            } else if m == 33u {
                bb = vec3<f32>(hard_mix_op(cs_ok.r, cb_ok.r), hard_mix_op(cs_ok.g, cb_ok.g), hard_mix_op(cs_ok.b, cb_ok.b));
            } else if m == 34u {
                bb = vec3<f32>(divide_op(cs_ok.r, cb_ok.r), divide_op(cs_ok.g, cb_ok.g), divide_op(cs_ok.b, cb_ok.b));
            } else if m == 38u {
                // Subtract: base minus blend, floored at zero (CSP BM-014).
                bb = max(cb_ok - cs_ok, vec3<f32>(0.0));
            }
        }
        let rgb = s.rgb * (1.0 - da) + sa * da * bb + d.rgb * (1.0 - sa);
        return vec4<f32>(rgb, out.a);
    }

    // The nonseparable modes: the whole RGB triple through one operator.
    if (m >= 23u && m <= 25u) || (m >= 35u && m <= 37u) {
        let cs_ok = select(vec3<f32>(0.0), s.rgb / sa, sa > 0.0);
        let cb_ok = select(vec3<f32>(0.0), d.rgb / da, da > 0.0);
        var bb = vec3<f32>(0.0);
        if sa > 0.0 && da > 0.0 {
            if m == 23u {
                bb = set_lum3(set_sat3(cs_ok, sat3(cb_ok)), lum3(cb_ok));
            } else if m == 24u {
                bb = set_lum3(set_sat3(cb_ok, sat3(cs_ok)), lum3(cb_ok));
            } else if m == 25u {
                bb = set_lum3(cs_ok, lum3(cb_ok));
            } else if m == 35u {
                // Darker color — brightness compare, ties to the source.
                bb = select(cb_ok, cs_ok, lum3(cs_ok) <= lum3(cb_ok));
            } else if m == 36u {
                bb = select(cb_ok, cs_ok, lum3(cs_ok) >= lum3(cb_ok));
            } else {
                // Brightness (SVG luminosity) — the inverse of Color.
                bb = set_lum3(cb_ok, lum3(cs_ok));
            }
        }
        let rgb = s.rgb * (1.0 - da) + sa * da * bb + d.rgb * (1.0 - sa);
        return vec4<f32>(rgb, out.a);
    }
    return out;
}

// --- per-layer display maths -------------------------------------------
// A COPY of tiles.wgsl's pair, because WGSL has no include and this pass is
// the same layer source seen through a different blend. Leaving it out was a
// real bug: a layer with a layer colour AND a part-2 blend mode (Overlay,
// Darken, …) composited untinted on the GPU while the CPU tinted it, so the
// screen and the exported page disagreed. Change these two functions and
// tiles.wgsl and mn_core::blend together — all three or none.

fn unpack_rgb(v: u32) -> vec3<f32> {
    return vec3<f32>(
        f32((v >> 16u) & 0xFFu) / 255.0,
        f32((v >> 8u) & 0xFFu) / 255.0,
        f32(v & 0xFFu) / 255.0,
    );
}

fn expression_reduce(px: vec4<f32>, e: u32) -> vec4<f32> {
    if e == 0u || px.a <= 0.0 {
        return px;
    }
    let lum = (px.r + px.g + px.b) / (3.0 * px.a);
    if e == 1u {
        return vec4<f32>(vec3<f32>(lum * px.a), px.a);
    }
    let a1 = select(0.0, 1.0, px.a >= 0.5);
    let s = select(0.0, a1, lum >= 0.5);
    return vec4<f32>(vec3<f32>(s), a1);
}

fn layer_colour_tint(px: vec4<f32>, tint: u32, fx: u32) -> vec4<f32> {
    if tint == 0xFFFFFFFFu || px.a <= 0.0 {
        return px;
    }
    let v = px.rgb / px.a;
    let lum = (v.r + v.g + v.b) / 3.0;
    let main = unpack_rgb(tint);
    let sub = unpack_rgb(fx);
    return vec4<f32>(px.a * (main * (1.0 - lum) + sub * lum), px.a);
}

struct VsIn {
    @builtin(vertex_index) vi: u32,
    @location(0) rect: vec4<f32>,
    @location(1) mode: u32,
    @location(2) opacity: f32,
    @location(3) blend_mode: u32,
    @location(4) tint: u32,
    @location(5) fx: u32,
}

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) rect: vec4<f32>,
    @location(1) @interpolate(flat) opacity: f32,
    @location(2) @interpolate(flat) blend_mode: u32,
    @location(3) @interpolate(flat) tint: u32,
    @location(4) @interpolate(flat) fx: u32,
}

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let corner = vec2<f32>(f32((in.vi << 1u) & 2u), f32(in.vi & 2u));
    var out: VsOut;
    out.pos = vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
    out.rect = in.rect;
    out.opacity = in.opacity;
    out.blend_mode = in.blend_mode;
    out.tint = in.tint;
    out.fx = in.fx;
    return out;
}

@fragment
fn fs_tile(in: VsOut) -> @location(0) vec4<f32> {
    let dim = vec2<f32>(textureDimensions(tile_tex));
    let local = in.pos.xy - in.rect.xy;
    let t = vec2<i32>(clamp(local, vec2<f32>(0.0), dim - vec2<f32>(1.0)));
    let raw = textureLoad(tile_tex, t, 0);
    // Same order as tiles.wgsl: reduce, tint, THEN fold in opacity.
    var src = vec4<f32>(raw) / 32768.0;
    src = expression_reduce(src, (in.fx >> 24u) & 3u);
    src = layer_colour_tint(src, in.tint, in.fx);
    let s = src * in.opacity;
    // The destination snapshot: framebuffer coords ARE canvas pixels.
    let d = textureLoad(snap_tex, vec2<i32>(in.pos.xy), 0);
    return blend2(s, d, in.blend_mode);
}

// --- group-blit variant (a folder/clip isolation buffer as the source) ----
@group(1) @binding(0) var group_tex: texture_2d<f32>;
@group(1) @binding(1) var group_smp: sampler;
@group(1) @binding(2) var snap2_tex: texture_2d<f32>;

@fragment
fn fs_blit(in: VsOut) -> @location(0) vec4<f32> {
    let uv = in.pos.xy / canvas.size;
    let s = textureSample(group_tex, group_smp, uv) * in.opacity;
    let d = textureLoad(snap2_tex, vec2<i32>(in.pos.xy), 0);
    return blend2(s, d, in.blend_mode);
}
