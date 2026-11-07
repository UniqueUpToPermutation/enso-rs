// ---- enso static mesh contract ----
//
// Prepended to every static mesh shader (after the globals and the material
// declarations: `material`, `material_sampler`, `texture0`..`texture3`).
// Your shader must define:
//
//   @vertex   fn vs_main(v: VertexIn, i: InstanceIn) -> <your output struct>
//   @fragment fn fs_main(<your output struct>) -> @location(0) vec4<f32>
//
// and may define (both optional; empty placeholders are used otherwise):
//
//   struct Material { .. }   per-material knobs, read as `material.<field>`
//   struct Instance { .. }   per-instance knobs, read as `instance_data(i).<field>`
//
// Fields must be f32 or vec2/3/4<f32> (or start with `_` for padding).

// Per-vertex data from the mesh (matches `Vertex` in Rust).
struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

// Fixed per-instance data from each `MeshDraw`.
struct InstanceIn {
    @location(3) model_0: vec4<f32>,
    @location(4) model_1: vec4<f32>,
    @location(5) model_2: vec4<f32>,
    @location(6) model_3: vec4<f32>,
    @location(7) color: vec4<f32>,
    @location(8) param_slot: u32,  // index of this instance's `Instance` block
};

fn instance_model(i: InstanceIn) -> mat4x4<f32> {
    return mat4x4<f32>(i.model_0, i.model_1, i.model_2, i.model_3);
}

// Per-instance knobs for every instance of this shader drawn this frame.
@group(2) @binding(0) var<storage, read> instances: array<Instance>;

// This instance's `struct Instance` values. Fragment shaders get them by
// passing them through from the vertex shader.
fn instance_data(i: InstanceIn) -> Instance {
    return instances[i.param_slot];
}
