
// ---- enso material (group 1) ----
// Shared by static meshes, skinned meshes and the sky. `Material` is YOUR struct: declare
// `struct Material { .. }` in your shader with the knobs you want (f32 and
// vec2/3/4<f32> fields); if you don't, an empty placeholder is used. Set
// fields by name from Rust. Unused texture slots are bound to 1x1 white.
@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var material_sampler: sampler;
@group(1) @binding(2) var texture0: texture_2d<f32>;
@group(1) @binding(3) var texture1: texture_2d<f32>;
@group(1) @binding(4) var texture2: texture_2d<f32>;
@group(1) @binding(5) var texture3: texture_2d<f32>;
