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

## Open proposals (not decided)

- Winit-free `WindowSurface` helper in enso-visual for surface boilerplate.
- `snap` flag on `MeshDraw` to skip interpolation for intentional jumps.
- Map shader error line numbers back to the user's file.
- Move `RenderWorld` + ID types into `enso-core`.
- Render-to-texture / full-screen effect subrenderer (feedback trails).
