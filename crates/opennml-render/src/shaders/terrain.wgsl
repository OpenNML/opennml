// Terrain like TerrainChunk_Draw (0x00558950), state block TerrainSplat:
// base pass  C = base texture(UV0) * vertex color, opaque, writes depth;
// alpha pass C = ground texture(UV2) * vertex color, A = alpha map(UV1), SRCALPHA/INVSRCALPHA.
// Edge skirts (TerrainChunk_DrawEdgeSkirts 0x00558F10): vertex color without texture.

struct TerrainIn {
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) uv0: vec2<f32>,
    @location(3) uv1: vec2<f32>,
    @location(4) uv2: vec2<f32>,
};

struct TerrainOut {
    // Base and alpha pass must deliver exactly the same depth (comparison LESSEQUAL).
    @invariant @builtin(position) clip: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) uv0: vec2<f32>,
    @location(2) uv1: vec2<f32>,
    @location(3) uv2: vec2<f32>,
    @location(4) fog: f32,
};

@vertex
fn vs_terrain(in: TerrainIn) -> TerrainOut {
    var out: TerrainOut;
    out.clip = frame.view_proj * vec4<f32>(in.position, 1.0);
    out.color = in.color.rgb;
    out.uv0 = in.uv0;
    out.uv1 = in.uv1;
    out.uv2 = in.uv2;
    out.fog = fog_factor(in.position);
    return out;
}

@fragment
fn fs_base(in: TerrainOut) -> @location(0) vec4<f32> {
    let color = textureSample(tex_a, mirror_sampler, in.uv0).rgb * in.color;
    return vec4<f32>(apply_fog(color, in.fog), 1.0);
}

@fragment
fn fs_splat(in: TerrainOut) -> @location(0) vec4<f32> {
    let color = textureSample(tex_a, wrap_sampler, in.uv2).rgb * in.color;
    let alpha = textureSample(tex_b, mirror_sampler, in.uv1).r;
    return vec4<f32>(apply_fog(color, in.fog), alpha);
}

struct SkirtIn {
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
};

struct SkirtOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_skirt(in: SkirtIn) -> SkirtOut {
    var out: SkirtOut;
    out.clip = frame.view_proj * vec4<f32>(in.position, 1.0);
    out.color = vec4<f32>(apply_fog(in.color.rgb, fog_factor(in.position)), in.color.a);
    return out;
}

@fragment
fn fs_skirt(in: SkirtOut) -> @location(0) vec4<f32> {
    return in.color;
}
