// Effect quads like ParticleEmitter_Render (0x00595470) and BillboardLayer_Render
// (0x00598E50, 0x00597B10): FVF 0x1C2, state eff_billboard (SRCALPHA/ONE) or
// eff_billboard_alpha (SRCALPHA/INVSRCALPHA), depth LESSEQUAL without writing, no culling,
// no fog, MODULATE(TEXTURE, DIFFUSE) for color and alpha, alpha test GREATER 0.
// vs_view: corners in view space (world = view = identity, only the projection applies);
// vs_world: flat planes in world coordinates with the camera view.

struct EffectIn {
    @location(0) position: vec3<f32>,
    // D3DCOLOR 0xAARRGGBB als Bytes B, G, R, A.
    @location(1) color: vec4<f32>,
    @location(2) uv: vec2<f32>,
};

struct EffectOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
};

@vertex
fn vs_view(in: EffectIn) -> EffectOut {
    var out: EffectOut;
    out.clip = frame.projection * vec4<f32>(in.position, 1.0);
    out.color = in.color.zyxw;
    out.uv = in.uv;
    return out;
}

@vertex
fn vs_world(in: EffectIn) -> EffectOut {
    var out: EffectOut;
    out.clip = frame.view_proj * vec4<f32>(in.position, 1.0);
    out.color = in.color.zyxw;
    out.uv = in.uv;
    return out;
}

@fragment
fn fs_effect(in: EffectOut) -> @location(0) vec4<f32> {
    let color = textureSample(tex_a, wrap_sampler, in.uv) * in.color;
    if (color.a <= 0.0) {
        discard;
    }
    return color;
}
