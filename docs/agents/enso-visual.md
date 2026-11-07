# enso-visual internals

## Public API shape

```rust
// Resources: created up front, referred to by Copy IDs (never reused)
renderer.create_texture(&TextureDesc) -> Result<TextureId>
renderer.create_mesh(&MeshData) -> MeshId
renderer.create_mesh_shader(&ShaderDesc) -> Result<MeshShaderId>
renderer.update_mesh_shader(id, &ShaderDesc) -> Result<()>       // hot reload, same id
renderer.create_skinned_mesh(&SkinnedMeshData) -> SkinnedMeshId
renderer.create_skeleton(&SkeletonDesc) -> Result<SkeletonId>
renderer.create_skinned_shader(&ShaderDesc) -> Result<SkinnedShaderId>
renderer.update_skinned_shader(id, &ShaderDesc) -> Result<()>
renderer.create_material(&MaterialDesc::new(mesh_or_skinned_shader)) -> Result<MaterialId>
renderer.create_sky_shader(&SkyShaderDesc) -> Result<SkyShaderId>
renderer.update_sky_shader(id, &SkyShaderDesc) -> Result<()>
renderer.destroy_*(id)

// Per simulation tick: plain data
world.camera / world.time / world.clear_color
world.sky = Some(Sky::new(sky_shader).with_params([Vec4; 4]))     // kept by clear()
world.draw_mesh(MeshDraw::new(mesh, material, transform).with_key(k))
world.draw_skinned(SkinnedDraw::new(mesh, skeleton, material, transform).with_key(k), &pose)
world.set_material_params(material, [Vec4; 4])

// Per display frame
renderer.render(RenderTarget, &prev, &curr, alpha)
```

Crate-root types shared by subrenderers: `ShaderDesc`, `BlendMode`,
`MaterialDesc`, `MaterialShader`, `MAX_TEXTURES`, `Transform`, `Camera`.
Per-subrenderer contract types live in `static_mesh::`, `skinned_mesh::`, `sky::`.

## Files

| File | Role |
|---|---|
| `src/lib.rs` | Module list + `pub use` re-exports = the public API |
| `src/renderer.rs` | `Renderer` facade: globals, depth buffer, textures, materials, owns subrenderers, `render()` |
| `src/subrenderer.rs` | `pub(crate)` `SubRenderer` trait (`prepare`, `draw`), `RenderContext`, `FrameData`, `DrawData` |
| `src/world.rs` | `RenderWorld` (incl. packed `poses` for skinned draws), `MaterialParams` |
| `src/ids.rs` | `define_id!` macro: all resource ID types |
| `src/shader.rs` | `GLOBALS_WGSL`, `ShaderDesc`, `BlendMode`, `compose()`, `capture_errors()`, `mesh_pipeline()` |
| `src/material.rs` + `material.wgsl` | `MaterialDesc`/`MaterialShader` (public), `MaterialStore` (internal), group-1 WGSL |
| `src/instancing.rs` | `InstanceBatches`: sort draws by `BatchKey`, merge into instanced batches, upload |
| `src/static_mesh/` | `MeshDraw`, `MeshData`/`Vertex`, `PRELUDE` (loc 0-2 vertex, 3-8 instance); private `StaticMeshRenderer` |
| `src/skinned_mesh/mod.rs` | `SkinnedDraw`, `PRELUDE` (loc 0-4 vertex, 5-11 instance, group 2 joints) |
| `src/skinned_mesh/skeleton.rs` | `SkeletonDesc` (public, `chain()`), `Skeleton` (validation, inverse binds, skinning matrices) + tests |
| `src/skinned_mesh/geometry.rs` | `SkinnedVertex`, `SkinnedMeshData::tube` + tests |
| `src/skinned_mesh/renderer.rs` | Private `SkinnedMeshRenderer`: pose interpolation, joint storage buffer, batching |
| `src/sky/` | `Sky`, `SkyShaderDesc`, `PRELUDE` (provides `sky_vs_main`, `sky_view_dir()`); private `SkyRenderer` |
| `src/offscreen.rs` | `OffscreenTarget`: render into a texture, `read_pixels` back to the CPU |
| `examples/common/mod.rs` | Example harness: `Example` trait, `Setup`, `Simulation`, shader hot reload, helpers |
| `examples/common/window.rs` | Interactive winit window + real-time loop (`common::run`) |
| `examples/common/golden.rs` | Headless golden-image test (`common::assert_golden`, test builds only) |
| `examples/golden/*.png` | Committed golden images, one per example |
| `examples/static_mesh.rs` | Cubes + wobbling sphere at a 20 Hz sim |
| `examples/sky.rs` | Gradient sky with a moving sun behind a ring of cubes |
| `examples/tentacles.rs` | Skinned tubes on joint chains, swaying with travelling waves |
| `tests/examples_have_golden_tests.rs` | Guard: every example is registered as a golden test |

## Example harness

`examples/common/` is included by each example with `mod common;`.
Cargo doesn't build it as an example because it is neither `examples/*.rs`
nor `examples/*/main.rs`. It owns logging, the winit window/surface, the
fixed-timestep loop (`Simulation`: `prev.clone_from(&curr); curr.clear();
simulate`), interpolation toggle (Space), quit (Esc), and shader hot reload.

## Adding an example

1. `examples/<name>.rs`: implement `common::Example` (`TITLE`, optional
   `TICK_HZ` and `GOLDEN_TIME`, `init(&mut Setup)`, `simulate(time, &mut
   RenderWorld)`), and call `common::run::<MyExample>()` from `main`. Load
   shaders from `examples/shaders/` with `setup.mesh_shader(file, blend)`,
   `setup.skinned_shader(file, blend)` or `setup.sky_shader(file)` to get
   hot reload. Helpers: `checkerboard`, `hue`.
2. At the end of the file:
   ```rust
   #[cfg(test)]
   mod tests {
       #[test]
       fn matches_golden_image() {
           crate::common::assert_golden::<crate::MyExample>();
       }
   }
   ```
3. In `crates/enso-visual/Cargo.toml`: `[[example]] name = "<name>" test = true`.
   This makes `cargo test` build the example as a test binary too.
4. `cargo test -p enso-visual` fails ("no golden image yet") and writes
   `examples/golden/<name>.actual.png`. **Look at it** (agents: read the PNG),
   then `cargo xtask golden accept <name>` and commit the `.png`.

Note: `cargo test` builds the *test* version of an example; to launch the
window, build it with `cargo build -p enso-visual --example <name>` first.

## Golden-image tests

* `assert_golden` initialises the example exactly as the window does,
  steps the simulation `round(GOLDEN_TIME * TICK_HZ)` ticks (time =
  `ticks / TICK_HZ`, exact), renders 640x360 at `alpha = 0.5` into an
  `Rgba8UnormSrgb` `OffscreenTarget`, and compares with
  `examples/golden/<name>.png`.
* Tolerance: a pixel differs if any channel is off by more than 3; the
  test fails if more than 0.1% of pixels differ. Constants are at the top
  of `golden.rs`. Goldens were made on an RTX 5080 / Vulkan; another GPU
  may need a looser tolerance or its own goldens.
* On failure it writes `<name>.actual.png` and `<name>.diff.png` (red =
  differs over a dimmed golden; gitignored) and panics with stats. Stale
  outputs are deleted at the start of each run.
* Needs a GPU adapter; there is no skip if none is found.
* Review: `cargo xtask golden` writes `target/golden-report.html` (expected
  / actual / diff / hover-to-compare), opens it, and asks `[y/N/q]` per
  failure when run in a terminal. Non-interactive:
  `cargo xtask golden accept <name>|--all`, `cargo xtask golden clean`.
* Refactors that shouldn't change pixels: run the golden tests *before*
  accepting anything; all existing goldens must still pass.
* `xtask/` is a workspace binary crate; `.cargo/config.toml` defines the
  `cargo xtask` alias. It has no dependencies.

## Shader composition

The final WGSL is `GLOBALS_WGSL` + shared parts + subrenderer `PRELUDE` +
user source (`shader::compose(parts, source)`):

| Subrenderer | Parts | User writes |
|---|---|---|
| static mesh | `MATERIAL_WGSL`, `static_mesh::PRELUDE` | `vs_main(VertexIn, InstanceIn)`, `fs_main` |
| skinned mesh | `MATERIAL_WGSL`, `skinned_mesh::PRELUDE` | `vs_main(SkinnedVertexIn, InstanceIn)`, `fs_main` |
| sky | `sky::PRELUDE` (contains `sky_vs_main`) | `fs_main(SkyIn)` |

Bind groups: 0 = globals (`view_proj`, `inv_view_proj`, camera, resolution,
time; one shared layout); 1 = material (mesh and skinned) or sky params;
2 = skinned joint matrices (storage buffer, vertex stage).
Rust ↔ WGSL layouts must match byte for byte: `GlobalsUniform` ↔ `Globals`,
each `InstanceRaw` ↔ its `InstanceIn`, `Vertex`/`SkinnedVertex` ↔ vertex inputs.

Static and skinned shaders share `shader::mesh_pipeline` (triangle list,
CCW front faces, depth `Less`, blending per `BlendMode`, depth write only
when opaque).

Compile error line numbers refer to the *composed* source, offset by the
globals + parts length. Mapping them back to the user file is a known TODO.

## Frame flow (`Renderer::render`)

1. Interpolate camera, time, clear colour; compute `delta_time` from the
   last rendered time; write the globals uniform.
2. `MaterialStore::update_params`: interpolate each material's knobs and
   upload the changed ones (shared by both mesh subrenderers).
3. `prepare()` on each subrenderer, in draw order:
   - **sky**: pick `curr.sky`; lerp params with `prev.sky` if the shader
     matches; upload if changed.
   - **static mesh**: match draws by `key` between prev/curr; interpolate;
     skip draws whose material is missing or not a `Mesh` shader; push
     into `InstanceBatches` (sorted by `(blended, shader, material, mesh)`,
     merged into instanced batches, uploaded).
   - **skinned mesh**: as static, plus for each draw: interpolate the pose
     joint by joint (if prev has the same skeleton and pose length), run
     `Skeleton::skinning_matrices` (walk parents → model space → times
     inverse bind), append to one frame-wide `Vec<Mat4>`, and store its
     start index as the instance's `joint_offset`. Upload to the storage
     buffer (grows to the next power of two; bind group rebuilt).
4. One render pass (clear colour + depth); `draw()` on each subrenderer in
   order: **sky first** (depth `Always`, no depth write), then static,
   then skinned meshes. Drawing the sky last at depth 1.0 would save
   overdraw but would paint over blended meshes (they don't write depth).
5. Submit. The caller presents.

## Interpolation semantics

- Draws match by `key` (unique per world, per draw type). Matched draws lerp
  translation and scale, slerp rotation, lerp colour and params. Mesh,
  material (and skeleton) come from `curr`.
- Skinned poses: per-joint `Transform::lerp` of *local* transforms when
  both ticks used the same skeleton with the same pose length; otherwise
  `curr`'s pose. Missing trailing joints use the rest pose.
- Unkeyed draws, or draws new this tick, render as given. Draws missing
  from `curr` vanish.
- Material params: both ticks set → lerp; only curr → curr; else the
  defaults from `MaterialDesc`.
- Sky params: lerp when both ticks have a sky with the same shader, else
  `curr`. The sky persists across `RenderWorld::clear()`.
- `alpha` is clamped to 0..1. Passing the same world twice = no interpolation.
- No "snap/teleport" flag yet (proposed).

## Skinning model

- `SkeletonDesc { parents, rest_pose }`: parents must precede children;
  the rest pose is local transforms. Inverse bind matrices are computed
  from it (`inverse(model_space(rest_pose))`), so drawing the rest pose
  yields identity skinning matrices (tested).
- Vertices: 4 joints (`u16`) + 4 weights (should sum to 1).
- Shader: `instance_model(i) * skin_matrix(v, i) * position`; normals the
  same with w = 0 (fine for rotations and uniform scale).
- A future glTF loader (enso-io) supplies explicit inverse bind matrices.
  `SkeletonDesc` may need an optional override field for them then.

## Adding a subrenderer

1. Create `src/<kind>/` with public contract types in `mod.rs`, a private
   `renderer.rs`, and `prelude.wgsl`.
2. Implement `SubRenderer` for it; bind globals at group 0 in `draw`.
   Mesh-like? Reuse `MATERIAL_WGSL` + `ctx.material_layout` (group 1),
   `shader::mesh_pipeline`, and `InstanceBatches`, and add a variant to
   `MaterialShader` (plus a check in `Renderer::create_material`).
3. Add a field to `Renderer`, construct it in `Renderer::new`, and add it to
   **both** subrenderer arrays in `render()` (prepare and draw), in draw order.
4. Add its section to `RenderWorld` (plain data, cleared in `clear()` if it
   is per-tick) and its `create_*`/`update_*`/`destroy_*` methods to `Renderer`.
5. Add ID types in `ids.rs` and error variants in `error.rs` as needed.
6. Add an example (with golden test) that exercises it.
