// Gamma ramp as post-processing: replacement for `IDirect3DDevice8::SetGammaRamp` of the original
// (`Renderer_SetGammaRamp` 0x00572060). Each channel of the finished image is looked up in the ramp
// (256 entries, value/65535), alpha stays.

struct Ramp {
    // Entry i is in values[i / 4][i % 4].
    values: array<vec4<f32>, 64>,
};

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var<uniform> ramp: Ramp;

// Triangle over the whole target.
@vertex
fn vs_gamma(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let x = f32((index << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(index & 2u) * 2.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

fn lookup(c: f32) -> f32 {
    let i = u32(round(clamp(c, 0.0, 1.0) * 255.0));
    return ramp.values[i >> 2u][i & 3u];
}

@fragment
fn fs_gamma(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureLoad(source, vec2<i32>(pos.xy), 0);
    return vec4<f32>(lookup(c.r), lookup(c.g), lookup(c.b), c.a);
}
