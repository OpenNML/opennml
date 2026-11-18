// Shared part of all world shaders: frame data (group 0) and texture pair (group 1).
// Matrices in the coordinate system of the original (left-handed), depth 0...1 like Direct3D.

struct Frame {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    projection: mat4x4<f32>,
    // xyz: normalized LIGHT_POSITION of the [LIGHT] block = direction to the light.
    light_dir: vec4<f32>,
    // Light diffuse (LIGHT_COLOR / 255).
    light_diffuse: vec4<f32>,
    // Light ambient (k, k, k) with k = 1 - 0.01 * LIGHT_INTENSITY.
    light_ambient: vec4<f32>,
    fog_color: vec4<f32>,
    // x: fog start, y: fog end (view space z), z: 1 = fog on.
    fog: vec4<f32>,
    // Water counters Terrain+0x2C, +0x30, +0x34, +0x38.
    water: vec4<f32>,
    // x: Terrain+0x3C, y: 1 = WATER_USETEXTURE.
    water2: vec4<f32>,
    // Projection with depth bias * view (g_matGameProjectionDepthBias) for shadows and decals.
    view_proj_bias: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;
// WRAP, linear, mip levels linear.
@group(0) @binding(1) var wrap_sampler: sampler;
// MIRROR, linear.
@group(0) @binding(2) var mirror_sampler: sampler;
// CLAMP, linear, no mip levels (shadows and decals).
@group(0) @binding(3) var clamp_sampler: sampler;

@group(1) @binding(0) var tex_a: texture_2d<f32>;
@group(1) @binding(1) var tex_b: texture_2d<f32>;

// Linear vertex fog (FOGVERTEXMODE LINEAR): 1 = no fog.
fn fog_factor(world: vec3<f32>) -> f32 {
    if (frame.fog.z == 0.0) {
        return 1.0;
    }
    let z = (frame.view * vec4<f32>(world, 1.0)).z;
    return clamp((frame.fog.y - z) / max(frame.fog.y - frame.fog.x, 1e-3), 0.0, 1.0);
}

fn apply_fog(color: vec3<f32>, f: f32) -> vec3<f32> {
    return mix(frame.fog_color.rgb, color, f);
}
