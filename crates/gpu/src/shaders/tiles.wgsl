// Canvas pass: composite layer tiles into the canvas texture.
//
// One instanced draw per tile quad. `mode == 0` paints the PAPER (PA-001:
// its colour rides the tint slot, its eye the opacity slot) as the reset of
// a damaged region; `mode == 1` samples the bound tile texture.
//
// GEOMETRY: one oversized triangle covering the whole target, clipped by the
// per-draw integer SCISSOR rect — not a quad. The obvious quad-per-region
// approach misrasterizes on this laptop's 2020 Intel DX12 driver: quad edges
// that land exactly on NDC x == 0 intermittently drop their first pixel
// column (reproduced 2026-08-14; WARP is exact either way). Scissor clipping
// is integer-exact on every driver.
//
// The blend equation itself is fixed-function state, not code — see the
// BlendStates in crates/gpu/src/lib.rs and mn_core::blend, which mirror each
// other. This shader's only job in the blend contract: emit the source
// premultiplied and already scaled by layer opacity.

@group(1) @binding(0) var tile_tex: texture_2d<u32>;

struct VsIn {
    @builtin(vertex_index) vi: u32,
    // x, y, w, h in canvas pixels (the scissor rect carries the same bounds)
    @location(0) rect: vec4<f32>,
    @location(1) mode: u32,
    @location(2) opacity: f32,
    // LP-016 layer colour, packed 0x00RRGGBB (0xFFFFFFFF = no tint).
    @location(4) tint: u32,
    // LP-017 sub colour (low 24 bits) + LP-022 expression reduce (bits 24+).
    @location(5) fx: u32,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) rect: vec4<f32>,
    @location(1) @interpolate(flat) mode: u32,
    @location(2) @interpolate(flat) opacity: f32,
    @location(4) @interpolate(flat) tint: u32,
    @location(5) @interpolate(flat) fx: u32,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    // (0,0) (2,0) (0,2) -> NDC (-1,-1) (3,-1) (-1,3): covers the target.
    let corner = vec2<f32>(f32((in.vi << 1u) & 2u), f32(in.vi & 2u));
    var out: VsOut;
    out.pos = vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
    out.rect = in.rect;
    out.mode = in.mode;
    out.opacity = in.opacity;
    out.tint = in.tint;
    out.fx = in.fx;
    return out;
}

// --- per-layer display maths (mn_core::blend's mirror) -------------------
// Kept in lockstep with `expression_reduce` + `layer_colour_tint`; blend2.wgsl
// carries the same pair for the shader-composite blend modes.

fn unpack_rgb(v: u32) -> vec3<f32> {
    return vec3<f32>(
        f32((v >> 16u) & 0xFFu) / 255.0,
        f32((v >> 8u) & 0xFFu) / 255.0,
        f32(v & 0xFFu) / 255.0,
    );
}

// LP-022 decrease-colour PREVIEW: 1 = grey, 2 = 1-bit mono (value AND
// coverage threshold at 50 %). Runs BEFORE the tint, so mono + a two-tone
// pair reads as a real two-colour layer.
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

// LP-016/LP-017 layer colour: the dark ink renders as the MAIN colour, the
// white end as the SUB colour, alpha and luminance structure preserved.
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

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    if in.mode == 0u {
        // PA-001: the reset quad is the PAPER. Colour in the tint slot, the
        // paper's eye in the opacity slot; premultiplied, so a hidden paper
        // is a true zero and the replace blend state punches the region back
        // to transparent for the present pass's checker to show through.
        let p = vec3<f32>(
            f32((in.tint >> 16u) & 0xFFu) / 255.0,
            f32((in.tint >> 8u) & 0xFFu) / 255.0,
            f32(in.tint & 0xFFu) / 255.0,
        );
        return vec4<f32>(p * in.opacity, in.opacity);
    }

    // Framebuffer coordinates ARE canvas pixels (the canvas renders 1:1).
    let dim = vec2<f32>(textureDimensions(tile_tex));
    let local = in.pos.xy - in.rect.xy;
    let t = vec2<i32>(clamp(local, vec2<f32>(0.0), dim - vec2<f32>(1.0)));
    let raw = textureLoad(tile_tex, t, 0);

    // Shader-side fix15 -> unorm (docs/ARCHITECTURE.md allows the display
    // path to approximate; export converts exactly on the CPU). Values stay
    // premultiplied, so the fixed-function blend states in lib.rs apply.
    var px = vec4<f32>(raw) / 32768.0;

    px = expression_reduce(px, (in.fx >> 24u) & 3u);
    px = layer_colour_tint(px, in.tint, in.fx);

    return px * in.opacity;
}
