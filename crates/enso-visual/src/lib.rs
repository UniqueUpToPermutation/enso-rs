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
//!                                             ├─ interpolates camera, time, transforms, poses, knobs
//!                                             └─ subrenderers (private), drawn in order:
//!                                                  sky, static meshes, skinned meshes
//! ```
//!
//! * **Resources** are created on the [`Renderer`] and referred to by ID:
//!   textures, meshes, skeletons, shaders and materials.
//! * **Per tick**, the application describes what to draw in a
//!   [`RenderWorld`]: plain data, no GPU handles.
//! * **Shaders** are written against a subrenderer's contract, a WGSL
//!   prelude prepended to your code (see [`static_mesh::PRELUDE`],
//!   [`skinned_mesh::PRELUDE`], [`sky::PRELUDE`]).
//! * **Materials** ([`MaterialDesc`]) bind a mesh or skinned shader to
//!   textures and knobs; they are shared by both mesh subrenderers.

mod camera;
mod error;
mod gpu;
mod ids;
mod instancing;
mod material;
mod offscreen;
mod renderer;
mod shader;
pub mod skinned_mesh;
pub mod sky;
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
pub use ids::{MaterialId, MeshId, MeshShaderId, SkeletonId, SkinnedMeshId, SkinnedShaderId, SkyShaderId, TextureId};
pub use material::{MAX_TEXTURES, MaterialDesc, MaterialShader};
pub use offscreen::OffscreenTarget;
pub use renderer::{DEPTH_FORMAT, RenderTarget, Renderer};
pub use shader::{BlendMode, GLOBALS_WGSL, ShaderDesc};
pub use texture::TextureDesc;
pub use transform::Transform;
pub use world::{MaterialParams, RenderWorld};

// Let downstream crates use exactly our versions of these without adding
// their own dependency (avoids version mismatches).
pub use {glam, wgpu};
