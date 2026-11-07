# enso-visual internals

## Public API shape

```rust
// Resources: created up front, referred to by Copy IDs (never reused)
renderer.create_texture(&TextureDesc) -> Result<TextureId>
renderer.create_mesh(&MeshData) -> MeshId
renderer.create_mesh_shader(&ShaderDesc) -> Result<MeshShaderId>
renderer.update_mesh_shader(id, &ShaderDesc) -> Result<()>   // hot reload, same id
renderer.create_material(&MaterialDesc) -> Result<MaterialId>
renderer.create_sky_shader(&SkyShaderDesc) -> Result<SkyShaderId>
renderer.update_sky_shader(id, &SkyShaderDesc) -> Result<()>
renderer.destroy_*(id)

// Per simulation tick: plain data
world.camera / world.time / world.clear_color
world.sky = Some(Sky::new(sky_shader).with_params([Vec4; 4]))   // kept by clear()
world.draw_mesh(MeshDraw::new(mesh, material, transform).with_key(k))
world.set_material_params(material, [Vec4; 4])

// Per display frame
renderer.render(RenderTarget, &prev, &curr, alpha)
```

## Files

| File | Role |
|---|---|
| `src/lib.rs` | Module list + `pub use` re-exports = the public API |
| `src/renderer.rs` | `Renderer` facade: globals uniform, depth buffer, texture store, owns subrenderers, `render()` |
| `src/subrenderer.rs` | `pub(crate)` `SubRenderer` trait (`prepare`, `draw`), `RenderContext`, `FrameData` |
| `src/world.rs` | `RenderWorld`, `MaterialParams` |
| `src/ids.rs` | `define_id!` macro: `TextureId`, `MeshId`, `MeshShaderId`, `SkyShaderId`, `MaterialId` |
| `src/shader.rs` | `GLOBALS_WGSL`, `compose()`, `capture_errors()` (wgpu error scope → `RenderError`) |
| `src/static_mesh/mod.rs` | Public contract types: `MeshDraw`, `MaterialDesc`, `ShaderDesc`, `BlendMode`, `PRELUDE` |
| `src/static_mesh/renderer.rs` | Private `StaticMeshRenderer`: resource maps, interpolation, sort/batch, draw |
| `src/static_mesh/prelude.wgsl` | Shader contract: `VertexIn` (loc 0-2), `InstanceIn` (loc 3-8), group 1 material |
| `src/static_mesh/geometry.rs` | `Vertex`, `MeshData::{quad, cube, uv_sphere}` + winding tests |
| `src/sky/mod.rs` | Public contract types: `Sky`, `SkyShaderDesc`, `PRELUDE` |
| `src/sky/renderer.rs` | Private `SkyRenderer`: shader map, params uniform, one fullscreen-triangle draw |
| `src/sky/prelude.wgsl` | Sky contract: provides `sky_vs_main` + `SkyIn` + `sky_view_dir()`; user writes `fs_main` |
| `src/offscreen.rs` | `OffscreenTarget`: render into a texture, `read_pixels` back to the CPU |
| `examples/common/mod.rs` | Example harness: `Example` trait, `Setup`, `Simulation`, shader hot reload, helpers |
| `examples/common/window.rs` | Interactive winit window + real-time loop (`common::run`) |
| `examples/common/golden.rs` | Headless golden-image test (`common::assert_golden`, test builds only) |
| `examples/golden/*.png` | Committed golden images, one per example |
| `examples/static_mesh.rs` | Cubes + wobbling sphere at a 20 Hz sim; only scene code + its test |
| `examples/sky.rs` | Gradient sky with a moving sun behind a ring of cubes |
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
   shaders with `setup.mesh_shader("file.wgsl", blend)` or
   `setup.sky_shader("file.wgsl")` (from `examples/shaders/`) to get hot
   reload. Helpers: `checkerboard`, `hue`.
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
* `xtask/` is a workspace binary crate; `.cargo/config.toml` defines the
  `cargo xtask` alias. It has no dependencies.

## Shader composition

The final WGSL is `GLOBALS_WGSL` + subrenderer `PRELUDE` + user source. For
static meshes the user defines `vs_main` and `fs_main`; for the sky the
prelude provides the vertex stage (`sky_vs_main`) and the user defines only
`fs_main`. Group 0 = globals (shared by all subrenderers, same layout
object; includes `view_proj`, `inv_view_proj`, camera, resolution, time);
group 1 = subrenderer-specific.
`GlobalsUniform` (Rust) must match `Globals` (WGSL) byte for byte, and the
same goes for `InstanceRaw` ↔ `InstanceIn` and `Vertex` ↔ `VertexIn`.

Compile error line numbers refer to the *composed* source, offset by the
globals + prelude length. Mapping them back to the user file is a known TODO.

## Frame flow (`Renderer::render`)

1. Interpolate camera, time, clear colour; compute `delta_time` from the last
   rendered time; write the globals uniform.
2. `prepare()` on each subrenderer, in draw order:
   - **sky**: pick `curr.sky`; lerp its params with `prev.sky` if the shader
     matches; upload if changed. No sky, or unknown shader → draws nothing.
   - **static mesh**: interpolate material params and upload changed ones;
     match draws by `key` between prev/curr; interpolate; skip draws with
     missing resources; sort by `(blended, shader, material, mesh)`; build
     instanced batches; upload the instance buffer, growing it to the next
     power of two.
3. One render pass (clear colour + depth); `draw()` on each subrenderer in
   order: **sky first** (depth compare `Always`, no depth write, so it
   covers the clear colour and everything after lands in front), then
   static meshes. Drawing the sky last at depth 1.0 would save overdraw but
   would paint over blended meshes (they don't write depth).
4. Submit. The caller presents.

## Interpolation semantics

- Draws match by `key` (unique per world). Matched draws lerp translation
  and scale, slerp rotation, lerp colour and params. Mesh and material come
  from `curr`.
- Unkeyed draws, or draws new this tick, render as given. Draws missing
  from `curr` vanish.
- Material params: both ticks set → lerp; only curr → curr; else the
  defaults from `MaterialDesc`.
- Sky params: lerp when both ticks have a sky with the same shader, else
  `curr`. The sky persists across `RenderWorld::clear()`.
- `alpha` is clamped to 0..1. Passing the same world twice = no interpolation.
- No "snap/teleport" flag yet (proposed).

## Adding a subrenderer

1. Create `src/<kind>/` with public contract types in `mod.rs`, a private
   implementation, and `prelude.wgsl`.
2. Implement `SubRenderer` for it; bind globals at group 0 in `draw`.
3. Add a field to `Renderer`, construct it in `Renderer::new`, and add it to
   **both** subrenderer arrays in `render()` (prepare and draw), in draw order.
4. Add its section to `RenderWorld` (plain data) and its `create_*`/`destroy_*`
   methods to `Renderer`.
5. Add an ID type in `ids.rs` if it has new resource kinds.
