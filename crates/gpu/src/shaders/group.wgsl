// Group blit: blend an isolation buffer (a folder's composited children, or a
// clip layer's masked scratch) onto its backdrop as ONE source, at the
// folder/layer opacity, through the fixed-function blend states.
//
// Same oversized-triangle + scissor geometry as tiles.wgsl (see the driver
// note there). The group texture is canvas-sized, so the sample coordinate is
// just the framebuffer position over the canvas size.

struct CanvasUniform {
    size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> canvas: CanvasUniform;
@group(1) @binding(0) var group_tex: texture_2d<f32>;
@group(1) @binding(1) var group_smp: sampler;

struct VsIn {
    @builtin(vertex_index) vi: u32,
    @location(0) rect: vec4<f32>,
    @location(1) mode: u32,
    @location(2) opacity: f32,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) opacity: f32,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let corner = vec2<f32>(f32((in.vi << 1u) & 2u), f32(in.vi & 2u));
    var out: VsOut;
    out.pos = vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
    out.opacity = in.opacity;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let uv = in.pos.xy / canvas.size;
    // Premultiplied; opacity scales all four channels, same as tiles.wgsl.
    return textureSample(group_tex, group_smp, uv) * in.opacity;
}
