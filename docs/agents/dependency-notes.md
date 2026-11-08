# Dependency notes

These versions are newer than many agents' training data. When unsure, read
the source in `~/.cargo/registry/src/index.crates.io-*/<crate>-<version>/`
instead of guessing.

## Pins and why

| Crate | Version | Why |
|---|---|---|
| wgpu | 30.0 | Required by egui-wgpu 0.37 |
| winit | 0.30 | Required by egui-wgpu 0.37 (0.31 is beta) |
| glam | 0.34 (`bytemuck` feature) | |
| bytemuck | 1 (`derive`) | Pod structs for GPU buffers |
| egui / eframe | 0.37 | Must match wgpu 30 / winit 0.30. eframe's default features use the wgpu backend |
| egui-snarl / egui-scale | 0.12 / 0.6 (vendored in `third_party/`) | Upstream is on egui 0.36; see `third_party/README.md` |
| cpal | 0.18 | |
| midir | 0.11 | 0.10 linked an older ALSA than cpal 0.18 (`links` conflict) |

## egui 0.37 / eframe 0.37 API facts (verified)

- `eframe::App` has `fn ui(&mut self, ui: &mut egui::Ui, frame)` (plus an
  optional `logic`), not `update(ctx, frame)`.
- Panels: `egui::Panel::top(id)`, `Panel::bottom(id)` and
  `CentralPanel::default()`, each with `.show(ui, |ui| ..)` (`show_inside`
  is deprecated).
- `ui.close()` closes the enclosing menu/popup.
- egui-snarl's `SnarlViewer::show_input`/`show_output` must return
  `impl SnarlPin + 'static`. Returning `PinInfo` directly triggers the
  `refining_impl_trait` warning.

## wgpu 30 API facts (verified)

- `Instance::new(InstanceDescriptor::new_without_display_handle())`; `Instance::default()` also works.
- `request_adapter` / `request_device` return `Result`. `RequestAdapterOptions`
  has `apply_limit_buckets`, so use `..Default::default()`.
- `push_error_scope(filter)` returns a guard; `guard.pop()` is a future
  resolving to `Option<Error>` (block on it with pollster).
- `surface.get_current_texture()` returns `CurrentSurfaceTexture`:
  `Success(t) | Suboptimal(t) | Timeout | Occluded | Outdated | Lost | Validation`.
- Present with **`queue.present(surface_texture)`** (not `surface_texture.present()`).
- `PipelineLayoutDescriptor { bind_group_layouts: &[Option<&BindGroupLayout>], immediate_size }`
  (push constants are now "immediates").
- `VertexState.buffers` / `FragmentState.targets` are slices of `Option<_>`.
- `RenderPipelineDescriptor` and `RenderPassDescriptor` have `multiview_mask: Option<NonZeroU32>`.
- `DepthStencilState.depth_write_enabled: Option<bool>`, `depth_compare: Option<CompareFunction>`.
- `RenderPassColorAttachment` has `depth_slice: Option<u32>`.
- `SurfaceConfiguration` has `color_space` (default `Auto`).
- Device, Queue, Instance, Adapter, Buffer, Texture, TextureView, BindGroup,
  RenderPipeline etc. are all cheap `Clone` (Arc inside).
- `RenderPass<'_>` doesn't borrow resources set on it, so a `&self`
  method can record draws.

## glam 0.34

`Mat4::look_at_rh` / `perspective_rh` were **removed**. Use:

```rust
glam::camera::rh::view::look_at_mat4(eye, target, up)
glam::camera::rh::proj::directx::perspective(fov_y, aspect, near, far) // wgpu: depth 0..1, Y up
```

## winit 0.30

`ApplicationHandler` trait (`resumed`, `window_event`); create the window in
`resumed`. Use `Arc<Window>` so `instance.create_surface(window.clone())`
gives `Surface<'static>`.
