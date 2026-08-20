// Mip downsample: one full-target triangle, four taps of the level above,
// averaged IN LINEAR LIGHT.
//
// WHY FOUR TAPS AND NOT ONE. A single bilinear sample at the destination
// texel's centre IS an exact 2x2 box average — but the hardware averages
// the DISPLAY-ENCODED bytes, and the canvas holds display-encoded values
// (see present.wgsl's decode). Averaging encoded values is not averaging
// light: equal parts black and white come out at 127 when half the light
// actually looks like ~186. On a manga page — black ink on white paper and
// almost nothing else — that is precisely the content that suffers, and it
// reads as zoomed-out text and linework going harsh and chunky next to an
// app that resamples correctly (owner, 2026-08-20, comparing our text with
// CSP's at the same size and zoom).
//
// So: sample the four source texels explicitly (offsets of half a source
// texel land on their centres, so the linear sampler returns each exactly),
// convert to linear, average with alpha PREMULTIPLIED so a transparent
// texel cannot drag its colour in, then re-encode.

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_smp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    // Oversized triangle (never a quad: NDC-0 seams drop columns on the
    // known-cursed Intel DX12 driver — docs/ARCHITECTURE.md traps).
    var out: VsOut;
    let x = f32(i32(vi & 1u) * 4 - 1);
    let y = f32(i32(vi >> 1u) * 4 - 1);
    out.pos = vec4<f32>(x, -y, 0.0, 1.0);
    out.uv = vec2<f32>((x + 1.0) * 0.5, (y + 1.0) * 0.5);
    return out;
}

// sRGB transfer functions. The decode matches present.wgsl exactly, so a
// value that goes down the chain and comes back is unchanged.
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let dims = vec2<f32>(textureDimensions(src_tex, 0));
    let half_texel = 0.5 / dims;

    var acc = vec4<f32>(0.0);
    for (var i = 0u; i < 4u; i = i + 1u) {
        let sx = select(-1.0, 1.0, (i & 1u) == 1u);
        let sy = select(-1.0, 1.0, (i >> 1u) == 1u);
        let s = textureSampleLevel(
            src_tex,
            src_smp,
            in.uv + vec2<f32>(sx, sy) * half_texel,
            0.0,
        );
        acc = acc + vec4<f32>(srgb_to_linear(s.rgb) * s.a, s.a);
    }
    acc = acc * 0.25;

    if acc.a <= 0.0 {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(linear_to_srgb(acc.rgb / acc.a), acc.a);
}
