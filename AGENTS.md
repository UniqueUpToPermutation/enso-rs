# enso: agent guide

enso is a Rust toolkit for **live musical improvisation with visuals**: the
player improvises at a MIDI keyboard, often without a strict tempo, and enso
turns that into sound and graphics in real time. Low latency and never
crashing mid-performance matter more than anything else.

The owner is learning Rust and Cargo through this project. Explain the *why*
behind Rust/Cargo choices briefly when you make them.

## Status

| Crate | State |
|---|---|
| `crates/enso-visual` | Working: wgpu renderer with sky, static mesh, skinned mesh, sprite/billboard and MSDF text subrenderers, named shader parameters, interpolated `RenderWorld` API, examples with golden tests |
| `enso-core`, `enso-io`, `enso-audio`, `enso-engine`, `enso-ui`, `enso` (bin) | Planned, not created yet. See [architecture](docs/agents/architecture.md) |

## Commands

```sh
cargo check                                         # fast type-check, use constantly
cargo clippy --all-targets                          # keep it warning-free
cargo test -p enso-visual                           # unit tests + headless golden-image test per example
cargo xtask golden                                  # review golden-image failures, accept new images
cargo run -p enso-visual --example static_mesh      # windowed demo; Space toggles interpolation
cargo doc -p enso-visual --no-deps --open           # browse the public API
```

## Rules that are easy to break

- **Versions are pinned to match `egui-wgpu`** (wgpu 30, winit 0.30). Don't
  bump wgpu or winit independently, and don't use winit 0.31 (beta).
- **Every dependency version lives in the root `Cargo.toml`
  `[workspace.dependencies]`.** Member crates use `dep.workspace = true`.
- **`enso-visual` must not depend on winit** (dev-dependency for examples
  only). Windows and event loops belong to the application.
- **`Renderer` is a trait** (`lib.rs`) implemented by `WgpuRenderer`. New
  renderer methods go on the trait first (with docs), then the impl, then
  the fake in `tests/renderer_trait.rs`.
- **Subrenderers are private.** Users interact through `Renderer` (resource
  creation, `render`) and plain-data `RenderWorld`s. Don't re-expose them.
- **`RenderWorld` must stay plain data:** IDs, numbers, glam types. No wgpu
  handles, no `Arc`s to GPU objects. It is meant to be built on the
  simulation thread and may move to `enso-core`.
- **Every example is a golden-image test** (`test = true` in the crate's
  `Cargo.toml` + a `#[test]` calling `common::assert_golden`). A guard test
  fails if one is missing. Never accept a golden image you haven't looked at.
- **Nothing on the live path may panic on user input.** Shader compile
  errors return `RenderError`; missing resources are skipped silently.
- **The audio thread (future) must never allocate, lock, or block.**
- **Never work directly on `master`.** Before changing any file, create a
  branch for the task (`git switch -c <type>/<short-topic>`, e.g.
  `feat/particle-subrenderer`). Don't merge, rebase onto, or push `master`
  yourself; the owner merges. See [workflow.md](docs/agents/workflow.md#git).

## Wiki (read the relevant page before working in that area)

- [architecture.md](docs/agents/architecture.md): whole-system design, planned crates, threading
- [enso-visual.md](docs/agents/enso-visual.md): renderer internals, frame flow, adding a subrenderer
- [dependency-notes.md](docs/agents/dependency-notes.md): API gotchas in wgpu 30 / glam 0.34 / winit 0.30
- [workflow.md](docs/agents/workflow.md): verifying changes, Windows shell quirks, git conventions
- [decisions.md](docs/agents/decisions.md): decision log with reasons; append to it

Keep this wiki current: if you change a design or learn a non-obvious fact,
update the matching page in the same change.
