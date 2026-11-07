// ---- enso sky contract ----
//
// Prepended to every sky shader (after the globals). The vertex shader is
// provided: it draws one triangle that covers the whole screen. Your shader
// must define only:
//
//   @fragment fn fs_main(in: SkyIn) -> @location(0) vec4<f32>
//
// The sky is drawn before everything else, without depth, so anything drawn
// after it appears in front.

struct SkyIn {
    @builtin(position) position: vec4<f32>,  // fragment: pixel coordinates
    @location(0) uv: vec2<f32>,              // 0..1 across the screen, (0, 0) = top-left
    @location(1) ndc: vec2<f32>,             // -1..1, y up
};

// Four free knobs (matches `Sky::params` in Rust), interpolated between ticks.
struct Sky {
    params: array<vec4<f32>, 4>,
};
@group(1) @binding(0) var<uniform> sky: Sky;

// World-space direction from the camera through this pixel (normalised).
fn sky_view_dir(in: SkyIn) -> vec3<f32> {
    let far = globals.inv_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    return normalize(far.xyz / far.w - globals.camera_position.xyz);
}

// The fullscreen triangle: vertices at uv (0,0), (2,0), (0,2). The parts
// outside the screen are clipped away, so every pixel is covered exactly
// once (two triangles would shade the pixels along their shared diagonal twice).
@vertex
fn sky_vs_main(@builtin(vertex_index) index: u32) -> SkyIn {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    var out: SkyIn;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = uv;
    out.ndc = ndc;
    return out;
}
