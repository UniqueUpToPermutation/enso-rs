// ---- enso skinned mesh contract ----
//
// Prepended to every skinned mesh shader (after the globals and the material
// declarations: `material`, `material_sampler`, `texture0`..`texture3`).
// Your shader must define:
//
//   @vertex   fn vs_main(v: SkinnedVertexIn, i: InstanceIn) -> <your output struct>
//   @fragment fn fs_main(<your output struct>) -> @location(0) vec4<f32>
//
// and may define `struct Material { .. }` / `struct Instance { .. }` exactly
// as for static meshes (read with `material.<field>` / `instance_data(i)`).
//
// Typical vertex shader:
//
//   let world = instance_model(i) * skin_matrix(v, i) * vec4<f32>(v.position, 1.0);

// Per-vertex data from the mesh (matches `SkinnedVertex` in Rust).
struct SkinnedVertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) joints: vec4<u32>,   // which joints move this vertex
    @location(4) weights: vec4<f32>,  // how much each one counts (sum to 1)
};

// Fixed per-instance data from each `SkinnedDraw`.
struct InstanceIn {
    @location(5) model_0: vec4<f32>,
    @location(6) model_1: vec4<f32>,
    @location(7) model_2: vec4<f32>,
    @location(8) model_3: vec4<f32>,
    @location(9) color: vec4<f32>,
    @location(10) joint_offset: u32,  // where this instance's matrices start in joint_matrices
    @location(11) param_slot: u32,    // index of this instance's `Instance` block
};

fn instance_model(i: InstanceIn) -> mat4x4<f32> {
    return mat4x4<f32>(i.model_0, i.model_1, i.model_2, i.model_3);
}

// Skinning matrices for every instance drawn this frame (group 2).
@group(2) @binding(0) var<storage, read> joint_matrices: array<mat4x4<f32>>;

// The blended joint transform for this vertex: model space (rest pose) ->
// model space (current pose). Use it for positions and (with w = 0) normals.
fn skin_matrix(v: SkinnedVertexIn, i: InstanceIn) -> mat4x4<f32> {
    let o = i.joint_offset;
    return joint_matrices[o + v.joints.x] * v.weights.x
         + joint_matrices[o + v.joints.y] * v.weights.y
         + joint_matrices[o + v.joints.z] * v.weights.z
         + joint_matrices[o + v.joints.w] * v.weights.w;
}

// Per-instance knobs for every instance of this shader drawn this frame (group 3).
@group(3) @binding(0) var<storage, read> instances: array<Instance>;

fn instance_data(i: InstanceIn) -> Instance {
    return instances[i.param_slot];
}
