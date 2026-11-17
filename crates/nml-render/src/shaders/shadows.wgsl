// Shadows on the terrain (re/shadows). All passes with the projection with depth bias
// (g_matGameProjectionDepthBias), depth LESSEQUAL without writing, SRCALPHA/INVSRCALPHA,
// back faces discarded, fog like the terrain.
// - fs_decal (state Shadow): color = texture * vertex color, alpha = texture only
//   (shadow decal 0x80808080, ground plane terrain color, special class white).
// - fs_receiver (state ShadowReceiver): color black, alpha = atlas * SHADOW_INTENSITY/255,
//   texture coordinate projective per pixel from q = L_v * (P.x, y_min, P.z).
// - fs_ground: ground surface of the model browser (vertex color, opaque).

// Textures in tex_a (group 1), sampler clamp_sampler (CLAMP, linear, no mip levels).

struct DecalIn {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct DecalOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec3<f32>,
    @location(2) fog: f32,
};

@vertex
fn vs_decal(in: DecalIn) -> DecalOut {
    var out: DecalOut;
    out.clip = frame.view_proj_bias * vec4<f32>(in.position, 1.0);
    out.uv = in.uv;
    out.color = in.color.rgb;
    out.fog = fog_factor(in.position);
    return out;
}

@fragment
fn fs_decal(in: DecalOut) -> @location(0) vec4<f32> {
    let tex = textureSample(tex_a, clamp_sampler, in.uv);
    // Alpha test GREATEREQUAL 0 lets everything through.
    return vec4<f32>(apply_fog(tex.rgb * in.color, in.fog), tex.a);
}

@vertex
fn vs_ground(in: DecalIn) -> DecalOut {
    var out: DecalOut;
    out.clip = frame.view_proj * vec4<f32>(in.position, 1.0);
    out.uv = in.uv;
    out.color = in.color.rgb;
    out.fog = 1.0;
    return out;
}

@fragment
fn fs_ground(in: DecalOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}

struct ReceiverIn {
    @location(0) position: vec3<f32>,
    // q = L_v * (P.x, y_min, P.z).
    @location(1) light: vec3<f32>,
    // m00 and m11 of the light projection P_s, alpha factor (SHADOW_INTENSITY * k / 255).
    @location(2) scale: vec3<f32>,
    // Cell in the atlas: origin (u, v), edge length in uv, edge length in texels.
    @location(3) cell: vec4<f32>,
};

struct ReceiverOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) light: vec3<f32>,
    @location(1) scale: vec3<f32>,
    @location(2) cell: vec4<f32>,
    @location(3) fog: f32,
};

@vertex
fn vs_receiver(in: ReceiverIn) -> ReceiverOut {
    var out: ReceiverOut;
    out.clip = frame.view_proj_bias * vec4<f32>(in.position, 1.0);
    out.light = in.light;
    out.scale = in.scale;
    out.cell = in.cell;
    out.fog = fog_factor(in.position);
    return out;
}

@fragment
fn fs_receiver(in: ReceiverOut) -> @location(0) vec4<f32> {
    // Projective per pixel; without the D3D8 half texel (wgpu puts pixel centers at +0.5).
    let local = vec2<f32>(
        0.5 + 0.5 * in.scale.x * in.light.x / in.light.z,
        0.5 - 0.5 * in.scale.y * in.light.y / in.light.z,
    );
    // CLAMP inside the cell: at most up to the center of the edge texel.
    let half_texel = 0.5 / in.cell.w;
    let clamped = clamp(local, vec2<f32>(half_texel), vec2<f32>(1.0 - half_texel));
    let tex = textureSample(tex_a, clamp_sampler, in.cell.xy + clamped * in.cell.z).r;
    let alpha = tex * in.scale.z;
    // Alpha test GREATER 0.
    if (alpha <= 0.0) {
        discard;
    }
    return vec4<f32>(apply_fog(vec3<f32>(0.0), in.fog), alpha);
}
