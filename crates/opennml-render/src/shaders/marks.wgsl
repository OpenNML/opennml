// Ground marks of layer 4 (GameEngine_RenderWorld step 7, EffectManager_RenderLayer(4) 0x004F21F0)
// in state Footprint (re/renderer, section "Appendix: state blocks", recording from 0x004C36B8):
// Z test LESSEQUAL without writing, SRCALPHA/INVSRCALPHA, CULL CCW, fog like the terrain, alpha test
// GREATEREQUAL 0 (lets everything through). Projection with depth bias (g_matGameProjectionDepthBias;
// ZBIAS 16 is not mapped, as with state Shadow).
// - fs_footprint (FootprintEffect_Render 0x004F1B90): COLOROP SELECTARG1(TEXTURE) -> color only
//   from the texture, ALPHAOP MODULATE(TEXTURE, DIFFUSE) -> alpha = texture alpha * vertex alpha.
// - fs_tread (wheel track 0x004F5560): without texture (SetTexture(0, NULL)), black with vertex alpha.

// Textures in tex_a (group 1), sampler clamp_sampler (CLAMP, linear, no mip levels).

struct MarkIn {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct MarkOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) alpha: f32,
    @location(2) fog: f32,
};

@vertex
fn vs_mark(in: MarkIn) -> MarkOut {
    var out: MarkOut;
    out.clip = frame.view_proj_bias * vec4<f32>(in.position, 1.0);
    out.uv = in.uv;
    out.alpha = in.color.a;
    out.fog = fog_factor(in.position);
    return out;
}

@fragment
fn fs_footprint(in: MarkOut) -> @location(0) vec4<f32> {
    let tex = textureSample(tex_a, clamp_sampler, in.uv);
    return vec4<f32>(apply_fog(tex.rgb, in.fog), tex.a * in.alpha);
}

@fragment
fn fs_tread(in: MarkOut) -> @location(0) vec4<f32> {
    return vec4<f32>(apply_fog(vec3<f32>(0.0), in.fog), in.alpha);
}
