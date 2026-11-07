//! Static meshes: indexed triangle geometry drawn with per-instance
//! transforms and user-written shaders.
//!
//! This module holds the *public contract* of the static mesh subrenderer:
//! the data you create resources from and put into a
//! [`RenderWorld`](crate::RenderWorld). The subrenderer itself is private.
//!
//! The shader side of the contract is [`PRELUDE`] (`prelude.wgsl`).

mod geometry;
mod renderer;

use glam::Vec4;

pub use geometry::{MeshData, Vertex};
pub(crate) use renderer::StaticMeshRenderer;

use crate::{MaterialId, MeshId, ShaderId, TextureId, Transform, world::MaterialParams};

/// The WGSL contract prepended to every static mesh shader.
pub const PRELUDE: &str = include_str!("prelude.wgsl");

/// Number of texture slots in a material (`texture0`..`texture3`).
pub const MAX_TEXTURES: usize = 4;

/// How a shader's output combines with what is already on screen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BlendMode {
    /// Overwrites; writes depth. Drawn first.
    #[default]
    Opaque,
    /// Standard transparency using output alpha. No depth write.
    Alpha,
    /// Adds light: glows, particles, trails. No depth write.
    Additive,
}

/// Everything needed to build a static mesh shader.
#[derive(Debug, Clone)]
pub struct ShaderDesc<'a> {
    pub label: &'a str,
    /// Your WGSL: must define `vs_main` and `fs_main` (see [`PRELUDE`]).
    pub source: &'a str,
    pub blend: BlendMode,
    /// Draw back faces too (disables culling).
    pub double_sided: bool,
}

impl<'a> ShaderDesc<'a> {
    pub fn new(label: &'a str, source: &'a str) -> Self {
        Self { label, source, blend: BlendMode::Opaque, double_sided: false }
    }
    pub fn blend(mut self, blend: BlendMode) -> Self {
        self.blend = blend;
        self
    }
    pub fn double_sided(mut self, yes: bool) -> Self {
        self.double_sided = yes;
        self
    }
}

/// A shader plus its inputs: up to [`MAX_TEXTURES`] textures and default
/// values for its four knobs.
#[derive(Debug, Clone)]
pub struct MaterialDesc<'a> {
    pub shader: ShaderId,
    /// Bound to `texture0`.. in order; missing slots are 1x1 white.
    pub textures: &'a [TextureId],
    /// Used whenever a `RenderWorld` does not set this material's params.
    pub params: MaterialParams,
}

impl<'a> MaterialDesc<'a> {
    pub fn new(shader: ShaderId) -> Self {
        Self { shader, textures: &[], params: [Vec4::ZERO; 4] }
    }
    pub fn with_textures(mut self, textures: &'a [TextureId]) -> Self {
        self.textures = textures;
        self
    }
    pub fn with_params(mut self, params: MaterialParams) -> Self {
        self.params = params;
        self
    }
}

/// One instance of a mesh in a [`RenderWorld`](crate::RenderWorld).
/// Its fields reach the shader as `InstanceIn` (see [`PRELUDE`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshDraw {
    /// Stable identity across ticks (e.g. an ECS entity id). Draws with the
    /// same key in the previous and current world are interpolated; draws
    /// without a key (or new this tick) render exactly as given.
    /// Keys should be unique within one world.
    pub key: Option<u64>,
    pub mesh: MeshId,
    pub material: MaterialId,
    pub transform: Transform,
    pub color: Vec4,
    /// Free per-instance knobs, e.g. note velocity or age.
    pub params: Vec4,
}

impl MeshDraw {
    pub fn new(mesh: MeshId, material: MaterialId, transform: impl Into<Transform>) -> Self {
        Self {
            key: None,
            mesh,
            material,
            transform: transform.into(),
            color: Vec4::ONE,
            params: Vec4::ZERO,
        }
    }
    pub fn with_key(mut self, key: u64) -> Self {
        self.key = Some(key);
        self
    }
    pub fn with_color(mut self, color: Vec4) -> Self {
        self.color = color;
        self
    }
    pub fn with_params(mut self, params: Vec4) -> Self {
        self.params = params;
        self
    }

    /// Blend the continuous properties of `prev` towards `curr`. Discrete
    /// ones (mesh, material) come from `curr`.
    pub(crate) fn interpolate(prev: &Self, curr: &Self, t: f32) -> Self {
        Self {
            transform: prev.transform.lerp(&curr.transform, t),
            color: prev.color.lerp(curr.color, t),
            params: prev.params.lerp(curr.params, t),
            ..*curr
        }
    }
}
