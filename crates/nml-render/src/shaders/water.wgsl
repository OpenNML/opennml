// Water like Terrain_RenderWaterPass (0x00559570) in alpha map mode, state block Water:
// wave w = 5 * sin(i + Terrain+0x2C) except at the map edge (x += w, y += w, z -= w),
// texture matrices M0/M1 shift the UV by 0.02 * (cos, sin) of the counters,
// C = vertex color * base water(uv*M0) * refraction(uv*M1), A = vertex alpha.
// Without WATER_USETEXTURE (state Highlight) only vertex color and alpha.
// Detail texture mode (TerrainQuality >= 2): two layers on stage 0, both with M0 and the
// vertex alpha: first vertex color * base water, then vertex color * refraction. Stage 1 has no
// texture there; it is assumed that it leaves the color unchanged (open question).

struct WaterIn {
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) uv: vec2<f32>,
    // x: grid index z * (DIM_X + 1) + x, y: 1 at the map edge.
    @location(3) wave: vec2<f32>,
};

struct WaterOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv0: vec2<f32>,
    @location(2) uv1: vec2<f32>,
    @location(3) fog: f32,
};

@vertex
fn vs_water(in: WaterIn) -> WaterOut {
    var position = in.position;
    if (in.wave.y == 0.0) {
        let w = 5.0 * sin(in.wave.x + frame.water.x);
        position = position + vec3<f32>(w, w, -w);
    }
    var out: WaterOut;
    out.clip = frame.view_proj * vec4<f32>(position, 1.0);
    out.color = in.color;
    out.uv0 = in.uv + vec2<f32>(0.02 * cos(frame.water.y), 0.02 * sin(frame.water.z));
    out.uv1 = in.uv + vec2<f32>(0.02 * cos(frame.water.w), 0.02 * sin(frame.water2.x));
    out.fog = fog_factor(position);
    return out;
}

@fragment
fn fs_water(in: WaterOut) -> @location(0) vec4<f32> {
    var color = in.color.rgb;
    if (frame.water2.y != 0.0) {
        color = color * textureSample(tex_a, wrap_sampler, in.uv0).rgb
            * textureSample(tex_b, wrap_sampler, in.uv1).rgb;
    }
    return vec4<f32>(apply_fog(color, in.fog), in.color.a);
}

@fragment
fn fs_water_detail_base(in: WaterOut) -> @location(0) vec4<f32> {
    let color = in.color.rgb * textureSample(tex_a, wrap_sampler, in.uv0).rgb;
    return vec4<f32>(apply_fog(color, in.fog), in.color.a);
}

@fragment
fn fs_water_detail_refraction(in: WaterOut) -> @location(0) vec4<f32> {
    let color = in.color.rgb * textureSample(tex_b, wrap_sampler, in.uv0).rgb;
    return vec4<f32>(apply_fog(color, in.fog), in.color.a);
}
