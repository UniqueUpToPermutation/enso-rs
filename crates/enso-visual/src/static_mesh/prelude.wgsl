// ---- enso static mesh contract ----
//
// Prepended to every StaticMeshRenderer shader (after the globals).
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

// Per-instance data from each `draw()` call (matches `MeshInstance` in Rust).
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

// Material (group 1): free knobs + up to four textures. Unused texture
// slots are bound to 1x1 white.
struct Material {
    params: array<vec4<f32>, 4>,
};
@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var material_sampler: sampler;
@group(1) @binding(2) var texture0: texture_2d<f32>;
@group(1) @binding(3) var texture1: texture_2d<f32>;
@group(1) @binding(4) var texture2: texture_2d<f32>;
@group(1) @binding(5) var texture3: texture_2d<f32>;
