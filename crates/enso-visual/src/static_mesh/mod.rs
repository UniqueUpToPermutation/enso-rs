//! Static meshes: indexed triangle geometry drawn with per-instance
//! transforms and user-written shaders.
//!
//! This module holds the *public contract* of the static mesh subrenderer:
//! the data you create resources from and put into a
//! [`RenderWorld`](crate::RenderWorld). The subrenderer itself is private.
//! Shaders ([`ShaderDesc`](crate::ShaderDesc)) and materials
//! ([`MaterialDesc`](crate::MaterialDesc)) are shared with skinned meshes.
//!
//! The shader side of the contract is [`PRELUDE`] (`prelude.wgsl`).

mod geometry;
mod renderer;

use glam::Vec4;

pub use geometry::{MeshData, Vertex};
pub(crate) use renderer::StaticMeshRenderer;

use crate::{MaterialId, MeshId, Transform, world::Span};

/// The WGSL contract prepended to every static mesh shader (after the
/// globals and material declarations).
pub const PRELUDE: &str = include_str!("prelude.wgsl");

/// One instance of a mesh in a [`RenderWorld`](crate::RenderWorld).
/// Its fields reach the shader as `InstanceIn` (see [`PRELUDE`]); its
/// shader's `struct Instance` values are set with
/// [`RenderWorld::draw_mesh`](crate::RenderWorld::draw_mesh)`(..).with(..)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshDraw {
    /// Stable identity across ticks (e.g. an ECS entity id). Draws with the
    /// same key in the previous and current world are interpolated; draws
    /// without a key (or new this tick) render exactly as given.
    /// Keys should be unique within one world.
    pub key: Option<u64>,
    pub mesh: MeshId,
    /// Must use a mesh shader ([`MeshShaderId`](crate::MeshShaderId)).
    pub material: MaterialId,
    pub transform: Transform,
    pub color: Vec4,
    /// This draw's `struct Instance` values in the world's packed array.
    pub(crate) params: Span,
}

impl MeshDraw {
    pub fn new(mesh: MeshId, material: MaterialId, transform: impl Into<Transform>) -> Self {
        Self {
            key: None,
            mesh,
            material,
            transform: transform.into(),
            color: Vec4::ONE,
            params: Span::default(),
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

    /// Blend the continuous properties of `prev` towards `curr`. Discrete
    /// ones (mesh, material) come from `curr`; parameter values are
    /// interpolated separately, by name.
    pub(crate) fn interpolate(prev: &Self, curr: &Self, t: f32) -> Self {
        Self {
            transform: prev.transform.lerp(&curr.transform, t),
            color: prev.color.lerp(curr.color, t),
            ..*curr
        }
    }
}
