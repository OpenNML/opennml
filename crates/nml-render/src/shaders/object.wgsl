// Objects like ModelInstance_Draw (0x0058B5E0), state block Objects: fixed-function light per
// vertex (Gouraud, directional light 0), stage 0 MODULATE(TEXTURE, DIFFUSE) for color and alpha,
// alpha test GREATER 0. vs_object: rigid meshes, morph frames and skin meshes in the bind pose
// (ModelInstance_DrawStaticGroup 0x005834B0); vs_skinned: skin meshes with a running animation like
// blendingN.vsh (v' = sum(w * M_bone * v), last weight 1 - sum(w), precomputed on the CPU).

struct Draw {
    world: mat4x4<f32>,
    // Inverse transpose of `world` (3 x 3), without normalization; for vs_object.
    normal: mat3x3<f32>,
    // Material diffuse (alpha = 1 - transparency or fade-out factor).
    diffuse: vec4<f32>,
    // Material ambient (alpha like diffuse; only vs_skinned reads it, D-287).
    ambient: vec4<f32>,
    // x: first bone of this instance in `bones`; y: 1 = no fog; z: 1 = no alpha test
    // (models in the effect pass).
    bones: vec4<u32>,
    // Cut at a horizontal plane (construction phase, fire): x world height, y 0 = none, 1 = keep the
    // part below, 2 = keep the part above.
    cut: vec4<f32>,
};

@group(2) @binding(0) var<uniform> draw: Draw;
@group(2) @binding(1) var<storage, read> bones: array<mat4x4<f32>>;

struct ObjectIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

struct ObjectOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) fog: f32,
    @location(3) world_y: f32,
};

// Light, fog and projection for a position in object space and a normal in world space
// (not normalized, as in the original: NORMALIZENORMALS stays off, the skin shaders do not normalize).
fn shade(position: vec3<f32>, normal: vec3<f32>, uv: vec2<f32>) -> ObjectOut {
    let world = draw.world * vec4<f32>(position, 1.0);
    let diffuse = max(dot(normal, frame.light_dir.xyz), 0.0);
    let lit = draw.ambient.rgb * frame.light_ambient.rgb
        + diffuse * draw.diffuse.rgb * frame.light_diffuse.rgb;
    var out: ObjectOut;
    out.clip = frame.view_proj * world;
    out.color = vec4<f32>(clamp(lit, vec3<f32>(0.0), vec3<f32>(1.0)), draw.diffuse.a);
    out.uv = uv;
    out.fog = select(fog_factor(world.xyz), 1.0, draw.bones.y != 0u);
    out.world_y = world.y;
    return out;
}

// Fixed function (ModelInstance_DrawStaticGroup): normal with the inverse transpose of the
// world matrix incl. node track and instance scaling (length 1/s at scale s).
@vertex
fn vs_object(in: ObjectIn) -> ObjectOut {
    return shade(in.position, draw.normal * in.normal, in.uv);
}

struct SkinnedIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) joints: vec4<u32>,
    @location(4) weights: vec4<f32>,
};

@vertex
fn vs_skinned(in: SkinnedIn) -> ObjectOut {
    var position = vec3<f32>(0.0);
    var normal = vec3<f32>(0.0);
    for (var k = 0u; k < 4u; k = k + 1u) {
        let w = in.weights[k];
        if (w != 0.0) {
            let m = bones[draw.bones.x + in.joints[k]];
            position = position + w * (m * vec4<f32>(in.position, 1.0)).xyz;
            normal = normal + w * (m * vec4<f32>(in.normal, 0.0)).xyz;
        }
    }
    // blendingN.vsh: `m3x3 r5, r3, c12` - the blended normal with the world matrix itself (not
    // the inverse transpose), without normalization; an instance scaling takes effect.
    let world_normal = (draw.world * vec4<f32>(normal, 0.0)).xyz;
    var out = shade(position, world_normal, in.uv);
    out.color.a = skinned_alpha(world_normal);
    return out;
}

// Alpha of the skin shader (D-287): blendingN.vsh computes `lit`/`mul`/`add oD0, r0, c2` on all
// four components, with c2 = light ambient * material ambient and c3 = light diffuse *
// material diffuse (ModelInstance_DrawSkinnedGroup 0x00588170, 0x0058826D/0x00588288); the light
// has alpha 1 (World_ApplyObjectLight 0x0042FFE0). With Material_SetAlpha(a) that gives
// a * (1 + max(0, dot(N, L))), clamped to [0, 1] when rasterizing. Opaque (a = 1) stays 1.
// The fixed function (vs_object), in contrast, takes only the diffuse alpha.
fn skinned_alpha(normal: vec3<f32>) -> f32 {
    let diffuse = max(dot(normal, frame.light_dir.xyz), 0.0);
    return clamp(
        draw.ambient.a * frame.light_ambient.a + diffuse * draw.diffuse.a * frame.light_diffuse.a,
        0.0,
        1.0,
    );
}

@fragment
fn fs_object(in: ObjectOut) -> @location(0) vec4<f32> {
    let color = textureSample(tex_a, wrap_sampler, in.uv) * in.color;
    let below = draw.cut.y == 1.0 && in.world_y > draw.cut.x;
    let above = draw.cut.y == 2.0 && in.world_y < draw.cut.x;
    if (below || above) {
        discard;
    }
    // Alpha test GREATER 0 as in the original. Deviation: the original compares after the 8-bit
    // quantization of the return color, here the comparison is on the floating-point value; alpha
    // values below 1/255 are therefore not discarded if they are greater than 0 (barely visible).
    if (color.a <= 0.0 && draw.bones.z == 0u) {
        discard;
    }
    return vec4<f32>(apply_fog(color.rgb, in.fog), color.a);
}
