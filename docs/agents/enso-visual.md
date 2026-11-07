# enso-visual internals

## Public API shape

`Renderer` is a **trait** (in `lib.rs`); `WgpuRenderer` implements it.
Callers need `use enso_visual::Renderer;` to call the methods. Code that
only builds scenes should take `&mut impl Renderer`; it then runs against
the GPU-free fake in `tests/renderer_trait.rs` too. The trait has an
associated `Target<'a>` (`RenderTarget` for wgpu, `()` for the fake), so
`render` doesn't name wgpu types. `TextureDesc::format` still does (a
`wgpu::TextureFormat`); replace it with an own enum before moving the
trait to `enso-core`.

```rust
let mut renderer = WgpuRenderer::new(gpu, surface_format);   // inherent: new, gpu, color_format

// Resources (trait methods): created up front, referred to by Copy IDs (never reused)
renderer.create_texture(&TextureDesc) -> Result<TextureId>
renderer.create_mesh(&MeshData) -> MeshId
renderer.create_mesh_shader(&ShaderDesc) -> Result<MeshShaderId>
renderer.update_mesh_shader(id, &ShaderDesc) -> Result<()>       // hot reload, same id
renderer.create_skinned_mesh(&SkinnedMeshData) -> SkinnedMeshId
renderer.create_skeleton(&SkeletonDesc) -> Result<SkeletonId>
renderer.create_skinned_shader(&ShaderDesc) -> Result<SkinnedShaderId>
renderer.update_skinned_shader(id, &ShaderDesc) -> Result<()>
renderer.create_material(&MaterialDesc::new(mesh_or_skinned_shader)) -> Result<MaterialId>
    // .with_param("base_color", Vec3::ONE) sets a `struct Material` default
renderer.create_font(&FontDesc::new("Noto Sans", ttf_bytes)) -> Result<FontId>   // builds the MSDF atlas
renderer.create_text_shader(&ShaderDesc) -> Result<TextShaderId>
renderer.update_text_shader(id, &ShaderDesc) -> Result<()>
renderer.create_sky_shader(&SkyShaderDesc) -> Result<SkyShaderId>
renderer.update_sky_shader(id, &SkyShaderDesc) -> Result<()>
renderer.destroy_*(id)

// Named parameters (see "Named parameters" below)
renderer.material_params(material) -> Option<&[ParamInfo]>       // list fields (GUI)
renderer.instance_params(material) -> Option<&[ParamInfo]>
renderer.material_param::<f32>(material, "decay") -> Result<Param<f32>>   // checked handle
renderer.instance_param::<f32>(material, "velocity") -> Result<Param<f32>>

// Per simulation tick: plain data
world.camera / world.time / world.clear_color
world.sky = Some(sky_material)          // a material of a sky shader; kept by clear()
world.draw_mesh(MeshDraw::new(mesh, material, transform).with_key(k)).with(velocity, 0.8).with(age, 0.1)
world.draw_skinned(SkinnedDraw::new(mesh, skeleton, material, transform).with_key(k), &pose).with(..)
world.draw_text(TextDraw::new(font, material, transform).with_size(0.5).with_align(TextAlign::Center), "C#").with(..)
world.set_param(material, decay, 2.5)

// Per display frame
renderer.render(RenderTarget, &prev, &curr, alpha)
```

Crate-root types shared by subrenderers: `ShaderDesc`, `BlendMode`,
`MaterialDesc`, `MaterialShader`, `MAX_TEXTURES`, `Transform`, `Camera`,
`Param`, `ParamType`, `ParamKind`, `ParamInfo`, `InstanceParams`.
Per-subrenderer contract types live in `static_mesh::`, `skinned_mesh::`, `sky::`.

## Files

| File | Role |
|---|---|
| `src/lib.rs` | Module list + `pub use` re-exports + **`pub trait Renderer`** (the interface, with its docs) |
| `src/renderer.rs` | `WgpuRenderer` (implements `Renderer`): globals, depth buffer, textures, materials, owns subrenderers, `render()`; inherent `new`/`gpu`/`color_format` |
| `src/subrenderer.rs` | `pub(crate)` `SubRenderer` trait (`prepare`, `draw`), `RenderContext`, `FrameData`, `DrawData` |
| `src/world.rs` | `RenderWorld` (material values, packed instance values + poses, `Span`), `InstanceParams` builder |
| `src/ids.rs` | `define_id!` macro: all resource ID types |
| `src/params.rs` | `Param<T>`/`ParamType`/`ParamKind`/`ParamInfo` (public); `ParamName` interning, `ParamValue`, `ParamLayout` (naga reflection, `pack`, `write`), `declares_struct` + tests |
| `src/shader.rs` | `GLOBALS_WGSL`, `ShaderDesc`, `BlendMode`, `compose()`, `compose_mesh()` (placeholder structs), `reflect_params()`, `capture_errors()`, `mesh_pipeline()` |
| `src/material.rs` + `material.wgsl` | `MaterialDesc` (`with_param` defaults)/`MaterialShader` (public), `MaterialStore` (per-material layout, packing, `relayout` on hot reload), group-1 WGSL |
| `src/instancing.rs` | `InstanceBatches`: sort by `BatchKey`, batch, instance vertex buffer + per-shader parameter segments (storage buffer, one bind group per segment) |
| `src/static_mesh/` | `MeshDraw`, `MeshData`/`Vertex`, `PRELUDE` (loc 0-2 vertex, 3-8 instance incl. `param_slot`, group 2 `instances`); private `StaticMeshRenderer` |
| `src/skinned_mesh/mod.rs` | `SkinnedDraw`, `PRELUDE` (loc 0-4 vertex, 5-11 instance, group 2 joints, group 3 `instances`) |
| `src/skinned_mesh/skeleton.rs` | `SkeletonDesc` (public, `chain()`), `Skeleton` (validation, inverse binds, skinning matrices) + tests |
| `src/skinned_mesh/geometry.rs` | `SkinnedVertex`, `SkinnedMeshData::tube` + tests |
| `src/skinned_mesh/renderer.rs` | Private `SkinnedMeshRenderer`: pose interpolation, joint storage buffer, batching |
| `src/sky/` | `SkyShaderDesc`, `PRELUDE` (fixed `sky_vs_main` outputting `SkyIn { view_dir, uv, ndc }`); private `SkyRenderer` (draws `world.sky`'s material) |
| `src/text/mod.rs` | `FontDesc`, `TextDraw`, `TextAlign`, `DEFAULT_CHARS`, `PRELUDE` |
| `src/text/font.rs` | `FontAtlas`: ttf-parser outline → fdsm MSDF (f32, sign + error correction, flipped to top-down) → shelf-packed RGBA8 atlas; `layout()` (lines, alignment, `?` fallback) + tests |
| `src/text/prelude.wgsl` | Text contract: `GlyphIn` (loc 0-8), `glyph_corner()`, `text_screen_distance/coverage/max_reach_px()`, groups 2 (font) and 3 (instances) |
| `src/text/renderer.rs` | Private `TextRenderer`: fonts (atlas texture + bind group), shaders, one instance per glyph, `draw(0..6, ..)` per batch |
| `src/offscreen.rs` | `OffscreenTarget`: render into a texture, `read_pixels` back to the CPU |
| `examples/common/mod.rs` | Example harness: `Example` trait, `Setup`, `Simulation`, shader hot reload, helpers |
| `examples/common/window.rs` | Interactive winit window + real-time loop (`common::run`) |
| `examples/common/golden.rs` | Headless golden-image test (`common::assert_golden`, test builds only) |
| `examples/golden/*.png` | Committed golden images, one per example |
| `examples/static_mesh.rs` | Cubes + wobbling sphere at a 20 Hz sim |
| `examples/sky.rs` | Gradient sky with a moving sun behind a ring of cubes |
| `examples/tentacles.rs` | Skinned tubes on joint chains, swaying with travelling waves |
| `examples/notes.rs` | Named parameters: keys struck by a melody (material + instance structs) and sparks (a second shader with a different `Instance` stride) |
| `examples/text.rs` | MSDF text: waving outlined title, subtitle, ring of note names lit by a playhead, over a sky |
| `examples/fonts/` | Noto Sans Regular + its OFL licence (used by the text example and font tests) |
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
user source (`shader::compose(parts, source)`; mesh-like subrenderers use
`compose_mesh`, which also appends `struct Material { _unused: vec4<f32> }`
/ `struct Instance { .. }` placeholders when the user source doesn't
declare them, since the preludes bind both):

| Subrenderer | Parts | User writes |
|---|---|---|
| static mesh | `MATERIAL_WGSL`, `static_mesh::PRELUDE` | `vs_main(VertexIn, InstanceIn)`, `fs_main`; optional `struct Material`, `struct Instance` |
| skinned mesh | `MATERIAL_WGSL`, `skinned_mesh::PRELUDE` | `vs_main(SkinnedVertexIn, InstanceIn)`, `fs_main`; optional structs |
| sky | `MATERIAL_WGSL`, `sky::PRELUDE` (contains `sky_vs_main`) | `fs_main(SkyIn)`; optional `struct Material` |
| text | `MATERIAL_WGSL`, `text::PRELUDE` | `vs_main(@builtin(vertex_index) u32, GlyphIn)`, `fs_main`; optional structs |

Bind groups:

| Group | Static mesh | Skinned mesh | Sky | Text |
|---|---|---|---|---|
| 0 | globals | globals | globals | globals |
| 1 | material (uniform `Material` + sampler + 4 textures) | material | material | material |
| 2 | `instances: array<Instance>` (storage) | joint matrices (storage) | | font atlas + sampler + `FontInfo { px_range }` |
| 3 | | `instances: array<Instance>` (storage) | | `instances` |

(Default `max_bind_groups` is 4: a new group needs care.)
Rust ↔ WGSL layouts must match byte for byte: `GlobalsUniform` ↔ `Globals`,
each `InstanceRaw` ↔ its `InstanceIn`, `Vertex`/`SkinnedVertex` ↔ vertex inputs.

Static and skinned shaders share `shader::mesh_pipeline` (triangle list,
CCW front faces, depth `Less`, blending per `BlendMode`, depth write only
when opaque).

Compile error line numbers refer to the *composed* source, offset by the
globals + parts length. Mapping them back to the user file is a known TODO.

## Frame flow (`WgpuRenderer::render`)

1. Interpolate camera, time, clear colour; compute `delta_time` from the
   last rendered time; write the globals uniform.
2. `MaterialStore::update_params`: per material, start from its default
   bytes, write each `curr` value (lerped with `prev`'s same field) at its
   reflected offset, upload if the bytes changed.
3. `prepare()` on each subrenderer, in draw order:
   - **sky**: pick `curr.sky`'s material if its shader is a sky shader
     (its knobs were already interpolated and uploaded in step 2).
   - **static mesh**: match draws by `key` between prev/curr; interpolate;
     skip draws whose material is missing or not a `Mesh` shader; pack the
     draw's `struct Instance` block (`ParamLayout::pack`, lerping by name
     with the matched prev draw's values); push into `InstanceBatches`.
     `upload` sorts by `(blended, shader, material, mesh)`, merges batches,
     assigns each instance a `param_slot` within its shader's segment,
     lays segments out at `min_storage_buffer_offset_alignment`, uploads
     both buffers and creates one bind group per segment.
   - **skinned mesh**: as static, plus for each draw: interpolate the pose
     joint by joint (if prev has the same skeleton and pose length), run
     `Skeleton::skinning_matrices` (walk parents → model space → times
     inverse bind), append to one frame-wide `Vec<Mat4>`, and store its
     start index as the instance's `joint_offset`. Upload to the storage
     buffer (grows to the next power of two; bind group rebuilt).
4. Colour load op: if `SkyRenderer::covers_target()` (a valid sky is set
   and its shader source has no `discard`), `LoadOp::DontCare` (unsafe
   token; sound because the sky REPLACE-writes every pixel first);
   otherwise `Clear(clear_color)`. Depth is always cleared to 1.0.
   One render pass; `draw()` on each subrenderer in
   order: **sky first** (depth `Always`, no depth write), then static,
   then skinned meshes, then **text** (blended, so last). Drawing the sky
   last at depth 1.0 would save overdraw but would paint over blended
   meshes (they don't write depth).
5. Submit. The caller presents.

## Interpolation semantics

- Draws match by `key` (unique per world, per draw type). Matched draws lerp
  translation and scale, slerp rotation, lerp colour. Mesh, material (and
  skeleton) come from `curr`.
- Instance parameters: per field *name*, lerp with the matched prev draw's
  value of the same kind; otherwise `curr`'s; unset fields are 0.
- Skinned poses: per-joint `Transform::lerp` of *local* transforms when
  both ticks used the same skeleton with the same pose length; otherwise
  `curr`'s pose. Missing trailing joints use the rest pose.
- Unkeyed draws, or draws new this tick, render as given. Draws missing
  from `curr` vanish.
- Material parameters: per field, both ticks set → lerp; only curr →
  curr; else the default from `MaterialDesc::with_param` (or 0). Values are
  per tick, not persistent: set them every tick you want them.
- The sky is a material, so its knobs interpolate like any material's.
  `world.sky` persists across `RenderWorld::clear()`; switching to another
  sky material is a cut (no crossfade).
- Sky view direction: computed per vertex (far-plane point via
  `inv_view_proj` minus the eye), *unnormalised*, and interpolated. This is
  exact for perspective cameras because the far-plane point is affine in
  screen position. Shaders must `normalize(in.view_dir)`.
- `alpha` is clamped to 0..1. Passing the same world twice = no interpolation.
- No "snap/teleport" flag yet (proposed).

## Named parameters

- A mesh-like shader may declare `struct Material` (per material,
  `material.<field>`) and `struct Instance` (per instance,
  `instance_data(i).<field>` in the vertex stage; pass to the fragment
  stage yourself). Fields: `f32`, `vec2/3/4<f32>`; names starting with `_`
  are padding (any type, never exposed). Anything else is a
  `RenderError::ShaderParams` at compile time.
- At shader creation the composed source is parsed again with
  `wgpu::naga` and the struct bound at group 1 (Material) and the
  instance group is read: names, kinds, byte offsets (naga applies WGSL's
  uniform/storage layout rules, e.g. `vec3` aligns to 16), and the struct
  size, which becomes the uniform buffer size or the storage array stride.
- `Param<T>` is an interned *name* + type marker, not an offset. Hot reload
  re-reflects; `MaterialStore::relayout` rebuilds buffers/bind groups if the
  size changed; handles keep working; values for vanished fields or a
  changed kind are ignored silently.
- `Renderer::material_param` / `instance_param` are provided trait methods
  built on `material_params` / `instance_params` (the field lists), so every
  implementation gets checked handles for free.
- The sky uses the same mechanism: `MaterialShader::Sky`, `struct Material`
  in the sky shader, `world.set_param(sky_material, ..)`.
  `instance_params` of a sky material is an empty list.

## Text (MSDF)

- `create_font` parses the font (ttf-parser) and, per character in
  `FontDesc::chars`, builds an fdsm shape from the outline (in font y-up
  orientation, offset into bitmap pixels), colours edges
  (`edge_coloring_simple(0.03)`), generates an f32 MSDF, applies sign and
  error correction, converts it to u8, and flips it top-down. Bitmaps are
  shelf-packed into a 1024-wide `Rgba8Unorm` atlas (linear, not sRGB).
  Glyph metrics are stored in ems; `TextDraw::size` = world units per em.
- Encoding: pixel = distance/`range_px` + 0.5 (so `px_range` = full range).
  `text_screen_px_range` converts that to screen pixels via `fwidth(uv)`.
  **Outlines/glows must clamp to `text_max_reach_px(uv)`**: beyond the
  stored range distances saturate, and small or edge-on text would turn
  into solid boxes (this happened in the first render of the example).
- Layout: per frame, per draw, `FontAtlas::layout` (lines on `\n`,
  alignment, `index` counts every char incl. spaces/newlines, missing → `?`).
  No kerning, no shaping, no wrapping. cosmic-text would be the upgrade.
- Each glyph is an instance (`GlyphRaw`) with no vertex buffer: 6 vertices
  from `vertex_index`. All glyphs of a draw get the same `Instance` block.
  Text pipelines are forced double-sided.
- **Fonts with overlapping contours** (e.g. Inter as shipped) produce
  spikes and specks at stroke joins: fdsm has no overlap support. The
  example uses Noto Sans (overlaps removed) for that reason. A real fix =
  boolean-union the outlines before generating (not implemented).
- Debugging an atlas: dump `FontAtlas::pixels` to a PNG (RGBA) and look.
  Clean MSDFs have smooth bands; artifacts show as specks at corners.

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
3. Add a field to `WgpuRenderer`, construct it in `WgpuRenderer::new`, and
   add it to **both** subrenderer arrays in `render()` (prepare and draw),
   in draw order.
4. Add its section to `RenderWorld` (plain data, cleared in `clear()` if it
   is per-tick). Declare its `create_*`/`update_*`/`destroy_*` methods (with
   docs) on the `Renderer` trait in `lib.rs`, implement them in
   `impl Renderer for WgpuRenderer`, and add stubs to the fake in
   `tests/renderer_trait.rs`.
5. Add ID types in `ids.rs` and error variants in `error.rs` as needed.
6. Add an example (with golden test) that exercises it.
