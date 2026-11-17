// Occluder of the shadow textures (state ShadowOccluder, ShadowTexture_RenderOccluder
// 0x0054E7B0): the same model with the same pose as in the object pass (vertex data and
// Draw/bones from object.wgsl), but from the light view L_v with the projection P_s into one cell
// of the atlas. No depth, no light and fog; output = alpha of the model texture (color TFACTOR
// black), alpha test GREATER 0.

struct Caster {
    // P_s * L_v.
    view_proj: mat4x4<f32>,
};

@group(3) @binding(0) var<uniform> caster: Caster;

struct OccluderOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_occluder(in: ObjectIn) -> OccluderOut {
    var out: OccluderOut;
    out.clip = caster.view_proj * (draw.world * vec4<f32>(in.position, 1.0));
    out.uv = in.uv;
    return out;
}

// blendingN_texshd.vsh: skin like blendingN, output position and texture coordinate.
@vertex
fn vs_occluder_skinned(in: SkinnedIn) -> OccluderOut {
    var position = vec3<f32>(0.0);
    for (var k = 0u; k < 4u; k = k + 1u) {
        let w = in.weights[k];
        if (w != 0.0) {
            let m = bones[draw.bones.x + in.joints[k]];
            position = position + w * (m * vec4<f32>(in.position, 1.0)).xyz;
        }
    }
    var out: OccluderOut;
    out.clip = caster.view_proj * (draw.world * vec4<f32>(position, 1.0));
    out.uv = in.uv;
    return out;
}

@fragment
fn fs_occluder(in: OccluderOut) -> @location(0) vec4<f32> {
    let alpha = textureSample(tex_a, wrap_sampler, in.uv).a;
    if (alpha <= 0.0) {
        discard;
    }
    return vec4<f32>(alpha, 0.0, 0.0, alpha);
}

// Clear the cell before drawing (Renderer_ClearTarget 0x00571620 with 0): triangle over the
// whole cell (viewport = cell).
@vertex
fn vs_clear(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let x = f32((index << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(index & 2u) * 2.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs_clear() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0);
}
