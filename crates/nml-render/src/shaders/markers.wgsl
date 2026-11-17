// Line rectangle of the selection without selection texture (state alpharect2, ObjectRenderer_Draw
// 0x0054A55A): color and alpha from the vertex color, no texture, no fog, Z test LESSEQUAL without
// writing, SRCALPHA/INVSRCALPHA, normal projection. The points are already in world coordinates.

struct LineIn {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct LineOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_line(in: LineIn) -> LineOut {
    var out: LineOut;
    out.clip = frame.view_proj * vec4<f32>(in.position, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_line(in: LineOut) -> @location(0) vec4<f32> {
    return in.color;
}
