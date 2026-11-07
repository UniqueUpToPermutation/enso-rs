
// ---- enso material (group 1) ----
// Shared by static and skinned meshes: four free knobs + up to four
// textures. Unused texture slots are bound to 1x1 white.
struct Material {
    params: array<vec4<f32>, 4>,
};
@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var material_sampler: sampler;
@group(1) @binding(2) var texture0: texture_2d<f32>;
@group(1) @binding(3) var texture1: texture_2d<f32>;
@group(1) @binding(4) var texture2: texture_2d<f32>;
@group(1) @binding(5) var texture3: texture_2d<f32>;
