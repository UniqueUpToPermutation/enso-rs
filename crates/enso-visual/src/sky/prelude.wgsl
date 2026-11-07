// ---- enso sky contract ----
//
// Prepended to every sky shader (after the globals and the material
// declarations: `material`, `material_sampler`, `texture0`..`texture3`).
// The vertex stage is provided: one triangle covering the whole screen,
// which outputs the view direction. Your shader must define only:
//
//   @fragment fn fs_main(in: SkyIn) -> @location(0) vec4<f32>
//
// and may declare `struct Material { .. }` for its knobs (`material.<field>`)
// and sample `texture0`.. (e.g. a panorama).
//
// The sky is drawn before everything else, without depth, so anything drawn
// after it appears in front.

struct SkyIn {
    @builtin(position) position: vec4<f32>,  // fragment: pixel coordinates
    // World-space direction from the camera through this pixel, NOT
    // normalised: an unnormalised direction interpolates exactly across the
    // screen, a normalised one would not. Use `normalize(in.view_dir)`.
    @location(0) view_dir: vec3<f32>,
    @location(1) uv: vec2<f32>,   // 0..1 across the screen, (0, 0) = top-left
    @location(2) ndc: vec2<f32>,  // -1..1, y up
};

// The fullscreen triangle: vertices at uv (0,0), (2,0), (0,2). The parts
// outside the screen are clipped away, so every pixel is covered exactly
// once (two triangles would shade the pixels along their shared diagonal twice).
@vertex
fn sky_vs_main(@builtin(vertex_index) index: u32) -> SkyIn {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);

    // The far-plane point behind this vertex, back in world space. With a
    // perspective camera it varies affinely across the screen, so the
    // direction to it interpolates exactly.
    let far = globals.inv_view_proj * vec4<f32>(ndc, 1.0, 1.0);

    var out: SkyIn;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.view_dir = far.xyz / far.w - globals.camera_position.xyz;
    out.uv = uv;
    out.ndc = ndc;
    return out;
}
