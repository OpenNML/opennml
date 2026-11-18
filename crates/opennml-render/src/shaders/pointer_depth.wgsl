// Depth pre-pass of the 3D pointer (O-457): rectangles with fixed depth, depth only, no color.
// The corners already arrive in NDC with depth (0 for the minimap, 1 for the cleared square around the mouse).

@vertex
fn vs_depth(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    return vec4<f32>(position, 1.0);
}

// The pipeline writes no color (empty write mask); the target still needs an output.
@fragment
fn fs_depth() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0);
}
