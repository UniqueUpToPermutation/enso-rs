# Decision log

Append new entries at the bottom: date, decision, why.

**2026-11-07: Cargo workspace, one crate per layer.** Shared lockfile and
versions, enforced dependency direction, per-layer examples and tests.

**2026-11-07: Pin wgpu 30 + winit 0.30.** Exactly what egui-wgpu 0.37
needs, so the GUI can share the renderer's device without version conflicts.

**2026-11-07: Renderer = shared state + collection of subrenderers.** Each
subrenderer owns one kind of drawable and defines its shader contract as a
WGSL prelude; users write only `vs_main`/`fs_main`. Lets users customise
shaders freely while the data layout stays fixed.

**2026-11-07: Shader errors are returned, never panic; shaders update in
place.** A typo during a live set must not crash the show.
`update_mesh_shader` keeps the `ShaderId`, so materials switch over automatically.

**2026-11-07: winit stays out of enso-visual.** The app owns windows and the
frame loop (egui needs the events, multiple windows share one renderer,
offscreen rendering stays possible).

**2026-11-07: Users never touch subrenderers; they hand over a
`RenderWorld`.** (Owner's request.) Replaced the earlier design where users
got subrenderers via `renderer.get_mut::<T>()` and queued draws on them.

**2026-11-07: Render between two simulation ticks:
`render(target, prev, curr, alpha)`.** (Owner's request.) The simulation runs
at a fixed tick; rendering interpolates. Consequences:
- Resources are plain `Copy` IDs instead of `Arc` handles, so `RenderWorld`
  is pure, `Send` data that the sim thread can build.
- Instances need a stable `key` to interpolate.
- `Transform` is stored as translation/rotation/scale (lerp/slerp), not as a matrix.
- Camera, time, clear colour and material params live in `RenderWorld`.
- Rendering lags the simulation by up to one tick, so run the sim fast (120-240 Hz).

**2026-11-07: Agents work on branches, never on `master`.** (Owner's
request.) Each task gets a `<type>/<short-topic>` branch; the owner reviews
and merges.

**2026-11-07: Every example is a headless golden-image test.** (Owner's
request.) Examples are built as tests with `[[example]] test = true`, so
the scene code isn't duplicated. Goldens live next to the examples, and
`cargo xtask golden` reviews and accepts them. Tolerance: >3/255 per channel
on >0.1% of pixels fails. Rendering at `alpha = 0.5` also exercises
interpolation.

**2026-11-07: Sky subrenderer: fullscreen triangle, drawn first.** (Owner's
request.) The prelude provides the vertex stage, and users write `fs_main`
using `sky_view_dir()`, which needed `inv_view_proj` in the globals. It's
drawn first with depth `Always` and no depth write. Drawing it last
(depth = 1.0, `LessEqual`) would avoid overdraw but paint over blended
meshes. The sky persists across `clear()` like the camera.
`ShaderId` was renamed `MeshShaderId` next to `SkyShaderId` so mixing them
up is a type error.

**2026-11-07: Skinned mesh subrenderer; the renderer owns skeletons and
interpolates local joint poses.** (Owner asked for a skinned mesh
renderer; pose ownership was the agent's call.) `RenderWorld` carries
*local* joint transforms per draw, packed in one `poses` buffer. The renderer
lerps/slerps them per joint, walks the hierarchy, and applies inverse binds
derived from the rest pose. Interpolating final matrices instead would
shear and shrink the mesh. Joint matrices for all instances go in one
storage buffer (group 2) indexed by a per-instance `joint_offset`.

**2026-11-07: Materials, `ShaderDesc`, `BlendMode` and instance batching
are shared at crate level.** Static and skinned meshes use the same
group-1 material layout, so one `MaterialStore` serves both;
`MaterialShader::{Mesh, Skinned}` records which subrenderer a material
belongs to. These types moved from `static_mesh::` to the crate root, a
breaking path change for users. Existing goldens were verified unchanged.

**2026-11-07: `Renderer` is a trait; `WgpuRenderer` implements it.**
(Owner's choice.) The interface and its docs live in `lib.rs`; the
implementation lives in `renderer.rs`. `render` takes an associated
`Target<'a>` type so the trait doesn't name wgpu types there. ID
constructors (`XxxId::next()`) became public so other implementations can
mint IDs. A GPU-free fake (`tests/renderer_trait.rs`) proves scene code
can be written against `impl Renderer` and tested without a GPU. Next
step when `enso-core` exists: move the trait, `RenderWorld`, IDs and
descriptors there (needs an own texture-format enum first).

**2026-11-07: Named, typed shader parameters via naga reflection.**
(Owner asked to implement the proposal.) Shaders declare `struct Material`
and `struct Instance`; their layouts are reflected at compile time, and
values are set by name (`Param<T>`) per tick and interpolated. This replaced
the anonymous `[Vec4; 4]` material knobs and `MeshDraw::params: Vec4`.
Instance data for different shaders has different strides, so it goes in
one storage buffer per frame, *segmented per shader* (aligned offsets, a
bind group per segment, `param_slot` per instance). Values are **per
tick** (owner didn't choose; matches `RenderWorld` and interpolation). A
persistent layer, e.g. for MIDI controller state, can live above the
renderer. Unset fields fall back to defaults.

**2026-11-07: The sky uses materials; its vertex stage is fixed and outputs
the view direction.** (Owner's request.) `MaterialShader::Sky`;
`world.sky: Option<MaterialId>`; sky shaders get `struct Material` knobs
and texture slots (panoramas). The prelude's `sky_vs_main` outputs an
*unnormalised* `view_dir` per vertex (exact under interpolation; normalise
in the fragment shader), replacing the per-pixel `sky_view_dir()`. This
replaced `Sky`/`SkyParams`/`with_params`. The sky and tentacles goldens were
unchanged within tolerance.

**2026-11-07: Skip the colour clear when the sky covers the screen.**
(Owner's request.) Uses `LoadOp::DontCare`, which wgpu documents as the
fastest load op when every pixel is overwritten. It's sound only because the
sky draws first with REPLACE over a fullscreen triangle. A sky shader whose
source mentions `discard` falls back to `Clear`, so pixels are never left
undefined. Keep this invariant if the sky's draw order or blending changes.

## Open proposals (not decided)

- Winit-free `WindowSurface` helper in enso-visual for surface boilerplate.
- `snap` flag on `MeshDraw` to skip interpolation for intentional jumps.
- Map shader error line numbers back to the user's file.
- Move `RenderWorld` + ID types into `enso-core`.
- Render-to-texture / full-screen effect subrenderer (feedback trails).
- Scene level: `world.scene` (ambient, fog), keyed lights in a storage
  buffer, optional `lighting.wgsl` helpers; a `struct Scene` for global knobs.
- `// @range(a, b) @default(x)` comment annotations on parameter fields for
  GUI sliders and defaults.
- egui control panel generated from `material_params` / `instance_params`.