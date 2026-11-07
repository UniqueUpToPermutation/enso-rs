// ---- enso static mesh contract ----
//
// Prepended to every static mesh shader (after the globals and the material
// declarations: `material`, `material_sampler`, `texture0`..`texture3`).
// Your shader must define:
//
//   @vertex   fn vs_main(v: VertexIn, i: InstanceIn) -> <your output struct>
//   @fragment fn fs_main(<your output struct>) -> @location(0) vec4<f32>
//
// Everything else (varyings, lighting, maths) is up to you.

// Per-vertex data from the mesh (matches `Vertex` in Rust).
struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

// Per-instance data from each `MeshDraw` (matches `InstanceRaw` in Rust).
struct InstanceIn {
    @location(3) model_0: vec4<f32>,
    @location(4) model_1: vec4<f32>,
    @location(5) model_2: vec4<f32>,
    @location(6) model_3: vec4<f32>,
    @location(7) color: vec4<f32>,
    @location(8) params: vec4<f32>,  // free per-instance knobs (e.g. note velocity)
};

fn instance_model(i: InstanceIn) -> mat4x4<f32> {
    return mat4x4<f32>(i.model_0, i.model_1, i.model_2, i.model_3);
}
