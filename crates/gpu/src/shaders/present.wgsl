// Present pass: the canvas texture, placed on screen by the Viewport.
//
// The CPU sends the canvas quad's four corners already transformed into NDC —
// pan, zoom AND rotation all land there (Viewport::corners_screen), so this
// shader is a lookup and the vertex stage stays free.

struct PresentUniform {
    // Canvas corners in NDC, triangle-strip order:
    //   c01.xy = top-left,    c01.zw = top-right
    //   c23.xy = bottom-left, c23.zw = bottom-right
    c01: vec4<f32>,
    c23: vec4<f32>,
    // flags.x = 1 when the render target is an sRGB format.
    // flags.y = 1 when the paper's eye is off (PA-001): the canvas texture
    //           then carries real alpha and this pass shows the transparency
    //           checker through the holes.
    flags: vec4<u32>,
};

// PA-001 transparency checker. SCREEN-SPACE on purpose — the cell size is
// fixed in device pixels, so zooming in does not turn the checker into big
// grey slabs that read as artwork, and zooming out does not turn it into a
// moiré. It is under the canvas, it is not part of the image, and it never
// reaches an exported PNG (the export path forces the paper on).
const CHECK_CELL: f32 = 8.0;
const CHECK_A: vec3<f32> = vec3<f32>(1.0, 1.0, 1.0);
const CHECK_B: vec3<f32> = vec3<f32>(0.796, 0.796, 0.808);

fn checker(frag: vec2<f32>) -> vec3<f32> {
    let c = floor(frag / CHECK_CELL);
    let odd = (c.x + c.y) - 2.0 * floor((c.x + c.y) * 0.5);
    return select(CHECK_A, CHECK_B, odd > 0.5);
}

@group(0) @binding(0) var<uniform> u: PresentUniform;
@group(0) @binding(1) var canvas_tex: texture_2d<f32>;
@group(0) @binding(2) var canvas_smp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    let c = vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u));

    // Bilinear pick of the corner: works for any quadrilateral, so a rotated
    // canvas needs no matrix here.
    let top = mix(u.c01.xy, u.c01.zw, c.x);
    let bottom = mix(u.c23.xy, u.c23.zw, c.x);
    let p = mix(top, bottom, c.y);

    var out: VsOut;
    out.pos = vec4<f32>(p, 0.0, 1.0);
    out.uv = c;
    return out;
}

// sRGB electro-optical transfer function, i.e. encoded value -> linear light.
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var c = textureSample(canvas_tex, canvas_smp, in.uv);

    // PA-001: paper hidden. The canvas arrives PREMULTIPLIED, so compositing
    // it over the checker is one multiply-add. Done here rather than in the
    // canvas pass because the checker must not scale, rotate or flip with the
    // page — it belongs to the screen, not to the drawing.
    if u.flags.y == 1u {
        c = vec4<f32>(c.rgb + checker(in.pos.xy) * (1.0 - c.a), 1.0);
    }

    // The canvas texture holds display-encoded 8-bit values (that is what the
    // brush authored and what export writes). A *_SRGB swapchain format makes
    // the hardware encode whatever the shader returns, which would encode
    // already-encoded values and wash the image out. Decoding here cancels it
    // exactly, so both surface kinds show the same picture.
    if u.flags.x == 1u {
        return vec4<f32>(srgb_to_linear(c.rgb), c.a);
    }
    return c;
}
