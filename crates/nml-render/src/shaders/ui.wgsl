// 2D pass of the UI (re/ui-rendering 3.4, re/menu-system 8.4): color = texture * diffuse. The corners
// already arrive as NDC (the raster shift of +0.5 px is in `ui.rs`). No `discard`: equivalent to the
// alpha test of the original (ALPHAREF open, re/menu-system 8.4, section "Open") - with
// SRCALPHA/INVSRCALPHA on color and alpha, alpha = 0 leaves the target unchanged.

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var tex_sampler: sampler;

struct UiIn {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct UiOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_ui(in: UiIn) -> UiOut {
    var out: UiOut;
    out.clip = vec4<f32>(in.position, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

@fragment
fn fs_ui(in: UiOut) -> @location(0) vec4<f32> {
    return textureSample(tex, tex_sampler, in.uv) * in.color;
}
