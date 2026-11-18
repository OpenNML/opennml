// Weather like Terrain_RenderWeatherPass (0x00557210), state block Weather: one quad per particle
// (strips 0, 1, 2 / 1, 3, 2), additive, no fog and no lighting.
// Snow: 8 x 8 in view space (view matrix = identity), white.
// Rain: cylindrical billboard 24 x 160, rotated horizontally toward the camera, standing upright;
// color 0x00000000 at the top, 0xDCFFFFFF at the bottom.

struct WeatherIn {
    @builtin(vertex_index) index: u32,
    // Center of the particle in world coordinates (instance).
    @location(0) center: vec3<f32>,
};

struct WeatherOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
};

// Corner of the quad: (sign x, sign y) and UV of vertices 0 ... 3.
fn corner(index: u32) -> vec4<f32> {
    var strip = array<u32, 6>(0u, 1u, 2u, 1u, 3u, 2u);
    var corners = array<vec4<f32>, 4>(
        vec4<f32>(-1.0, 1.0, 0.0, 0.0),
        vec4<f32>(-1.0, -1.0, 0.0, 1.0),
        vec4<f32>(1.0, 1.0, 1.0, 0.0),
        vec4<f32>(1.0, -1.0, 1.0, 1.0),
    );
    return corners[strip[index % 6u]];
}

@vertex
fn vs_snow(in: WeatherIn) -> WeatherOut {
    let c = corner(in.index);
    var view = frame.view * vec4<f32>(in.center, 1.0);
    view = vec4<f32>(view.xy + c.xy * 4.0, view.zw);
    var out: WeatherOut;
    out.clip = frame.projection * view;
    out.color = vec4<f32>(1.0);
    out.uv = c.zw;
    return out;
}

@vertex
fn vs_rain(in: WeatherIn) -> WeatherOut {
    let c = corner(in.index);
    // Camera right in world coordinates: first row of the view rotation (Camera+0x1C, row 0).
    let right = vec3<f32>(frame.view[0].x, frame.view[1].x, frame.view[2].x);
    let position = in.center + right * (c.x * 12.0) + vec3<f32>(0.0, c.y * 80.0, 0.0);
    var out: WeatherOut;
    out.clip = frame.view_proj * vec4<f32>(position, 1.0);
    if (c.y > 0.0) {
        out.color = vec4<f32>(0.0);
    } else {
        out.color = vec4<f32>(1.0, 1.0, 1.0, 220.0 / 255.0);
    }
    out.uv = c.zw;
    return out;
}

@fragment
fn fs_weather(in: WeatherOut) -> @location(0) vec4<f32> {
    let color = textureSample(tex_a, wrap_sampler, in.uv) * in.color;
    // ALPHAREF 0 with GREATER: alpha 0 is discarded.
    if (color.a <= 0.0) {
        discard;
    }
    return color;
}
