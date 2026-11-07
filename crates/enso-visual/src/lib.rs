//! # enso-visual
//!
//! The GPU renderer.
//!
//! ```text
//!   simulation (fixed tick)                 display (every frame)
//!   ───────────────────────                 ─────────────────────
//!   tick N-1 ──▶ RenderWorld (prev) ─┐
//!   tick N   ──▶ RenderWorld (curr) ─┼──▶ Renderer::render(target, prev, curr, alpha)
//!                          alpha ────┘        │
//!                                             ├─ interpolates camera, time, transforms, knobs
//!                                             └─ subrenderers (private), drawn in order:
//!                                                  static meshes, ...
//! ```
//!
//! * **Resources** are created on the [`Renderer`] and referred to by ID:
//!   [`TextureId`], [`MeshId`], [`ShaderId`], [`MaterialId`].
//! * **Per tick**, the application describes what to draw in a
//!   [`RenderWorld`]: plain data, no GPU handles.
//! * **Shaders** are written against a subrenderer's contract, a WGSL
//!   prelude prepended to your code (see [`static_mesh::PRELUDE`]).

mod camera;
mod error;
mod gpu;
mod ids;
mod renderer;
mod shader;
pub mod static_mesh;
mod subrenderer;
mod texture;
mod transform;
mod world;

// Re-exports: the crate's public API, flattened so users write
// `enso_visual::Renderer` instead of `enso_visual::renderer::Renderer`.
pub use camera::Camera;
pub use error::RenderError;
pub use gpu::{Gpu, GpuError};
pub use ids::{MaterialId, MeshId, ShaderId, TextureId};
pub use renderer::{DEPTH_FORMAT, RenderTarget, Renderer};
pub use shader::GLOBALS_WGSL;
pub use texture::TextureDesc;
pub use transform::Transform;
pub use world::{MaterialParams, RenderWorld};

// Let downstream crates use exactly our versions of these without adding
// their own dependency (avoids version mismatches).
pub use {glam, wgpu};
