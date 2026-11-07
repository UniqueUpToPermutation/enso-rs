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
//! * [`Renderer`] is the interface: resource creation (returning IDs) and
//!   `render`. [`WgpuRenderer`] is the implementation.
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
// `enso_visual::WgpuRenderer` instead of `enso_visual::renderer::WgpuRenderer`.
pub use camera::Camera;
pub use error::RenderError;
pub use gpu::{Gpu, GpuError};
pub use ids::{MaterialId, MeshId, MeshShaderId, SkeletonId, SkinnedMeshId, SkinnedShaderId, SkyShaderId, TextureId};
pub use material::{MAX_TEXTURES, MaterialDesc, MaterialShader};
pub use offscreen::OffscreenTarget;
pub use renderer::{DEPTH_FORMAT, RenderTarget, WgpuRenderer};
pub use shader::{BlendMode, GLOBALS_WGSL, ShaderDesc};
pub use texture::TextureDesc;
pub use transform::Transform;
pub use world::{MaterialParams, RenderWorld};

// Let downstream crates use exactly our versions of these without adding
// their own dependency (avoids version mismatches).
pub use {glam, wgpu};

use skinned_mesh::{SkeletonDesc, SkinnedMeshData};
use sky::SkyShaderDesc;
use static_mesh::MeshData;

/// The renderer interface.
///
/// * **Resources** (textures, meshes, skeletons, shaders, materials) are
///   created up front and referred to by `Copy` IDs. IDs are never reused:
///   draws referring to a destroyed resource are skipped.
/// * **Each frame**, [`render`](Self::render) is given the previous and
///   current [`RenderWorld`] and an interpolation factor.
///
/// Code that only describes scenes (an ECS, tests) can be written against
/// this trait (`fn setup(r: &mut impl Renderer)`) instead of a concrete
/// renderer. Bring it into scope to call its methods: `use enso_visual::Renderer;`.
pub trait Renderer {
    /// What [`render`](Self::render) draws into. For [`WgpuRenderer`] this
    /// is a [`RenderTarget`] (a texture view and its size).
    type Target<'a>;

    // ---- textures ----

    fn create_texture(&mut self, desc: &TextureDesc<'_>) -> Result<TextureId, RenderError>;

    /// Replace a texture's pixels. Cheap enough to call every frame.
    fn write_texture(&mut self, id: TextureId, data: &[u8]) -> Result<(), RenderError>;

    /// Materials already using this texture keep it alive on the GPU.
    fn destroy_texture(&mut self, id: TextureId);

    // ---- static meshes ----

    fn create_mesh(&mut self, data: &MeshData) -> MeshId;

    fn destroy_mesh(&mut self, id: MeshId);

    /// Compile a shader against the static mesh contract
    /// ([`static_mesh::PRELUDE`]). Compile errors are returned, never panicked on.
    fn create_mesh_shader(&mut self, desc: &ShaderDesc<'_>) -> Result<MeshShaderId, RenderError>;

    /// Recompile a shader in place, keeping its ID, so every material using
    /// it switches over. On error the previous version keeps running.
    fn update_mesh_shader(&mut self, id: MeshShaderId, desc: &ShaderDesc<'_>) -> Result<(), RenderError>;

    fn destroy_mesh_shader(&mut self, id: MeshShaderId);

    // ---- skinned meshes ----

    fn create_skinned_mesh(&mut self, data: &SkinnedMeshData) -> SkinnedMeshId;

    fn destroy_skinned_mesh(&mut self, id: SkinnedMeshId);

    /// Validate a joint hierarchy and precompute its inverse bind matrices.
    fn create_skeleton(&mut self, desc: &SkeletonDesc) -> Result<SkeletonId, RenderError>;

    fn destroy_skeleton(&mut self, id: SkeletonId);

    /// Compile a shader against the skinned mesh contract
    /// ([`skinned_mesh::PRELUDE`]).
    fn create_skinned_shader(&mut self, desc: &ShaderDesc<'_>) -> Result<SkinnedShaderId, RenderError>;

    /// Recompile a skinned shader in place, keeping its ID. On error the
    /// previous version keeps running.
    fn update_skinned_shader(&mut self, id: SkinnedShaderId, desc: &ShaderDesc<'_>) -> Result<(), RenderError>;

    fn destroy_skinned_shader(&mut self, id: SkinnedShaderId);

    // ---- materials ----

    /// A material for static meshes (shader is a [`MeshShaderId`]) or
    /// skinned meshes (shader is a [`SkinnedShaderId`]).
    fn create_material(&mut self, desc: &MaterialDesc<'_>) -> Result<MaterialId, RenderError>;

    fn destroy_material(&mut self, id: MaterialId);

    // ---- sky ----

    /// Compile a sky shader against the sky contract ([`sky::PRELUDE`]):
    /// you write `fs_main` only.
    fn create_sky_shader(&mut self, desc: &SkyShaderDesc<'_>) -> Result<SkyShaderId, RenderError>;

    /// Recompile a sky shader in place, keeping its ID. On error the
    /// previous version keeps running.
    fn update_sky_shader(&mut self, id: SkyShaderId, desc: &SkyShaderDesc<'_>) -> Result<(), RenderError>;

    fn destroy_sky_shader(&mut self, id: SkyShaderId);

    // ---- the frame ----

    /// Render the moment `alpha` of the way from `prev` to `curr`.
    ///
    /// `alpha = 0` renders `prev`, `alpha = 1` renders `curr`. With a
    /// fixed-rate simulation, `alpha` is how far the wall clock has moved
    /// into the next (not yet simulated) tick. To render a single world
    /// without interpolation, pass it as both `prev` and `curr`.
    fn render(&mut self, target: Self::Target<'_>, prev: &RenderWorld, curr: &RenderWorld, alpha: f32);
}
