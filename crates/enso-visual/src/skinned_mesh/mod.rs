//! Skinned meshes: geometry deformed by a skeleton of joints, posed every
//! tick.
//!
//! * Geometry ([`SkinnedMeshData`]) stores, per vertex, up to four joints
//!   and weights.
//! * A skeleton ([`SkeletonDesc`]) is a joint hierarchy plus its rest pose.
//! * Each tick, a [`SkinnedDraw`] supplies the *local* transform of every
//!   joint ([`RenderWorld::draw_skinned`](crate::RenderWorld::draw_skinned)).
//!   The renderer interpolates those per joint between ticks, walks the
//!   hierarchy, and uploads one skinning matrix per joint.
//!
//! Materials and shaders are shared with static meshes; a skinned draw
//! needs a material whose shader is a [`SkinnedShaderId`](crate::SkinnedShaderId).
//! The shader side of the contract is [`PRELUDE`] (`prelude.wgsl`).

mod geometry;
mod renderer;
mod skeleton;

use glam::Vec4;

pub use geometry::{SkinnedMeshData, SkinnedVertex};
pub(crate) use renderer::SkinnedMeshRenderer;
pub use skeleton::SkeletonDesc;

use crate::{MaterialId, SkeletonId, SkinnedMeshId, Transform, world::Span};

/// The WGSL contract prepended to every skinned mesh shader (after the
/// globals and material declarations).
pub const PRELUDE: &str = include_str!("prelude.wgsl");

/// One posed instance of a skinned mesh in a
/// [`RenderWorld`](crate::RenderWorld). Add it with
/// [`RenderWorld::draw_skinned`](crate::RenderWorld::draw_skinned), which
/// also takes the pose.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkinnedDraw {
    /// Stable identity across ticks; see [`MeshDraw::key`](crate::static_mesh::MeshDraw::key).
    /// Matched draws interpolate their transform, colour, parameters and,
    /// if they use the same skeleton, every joint of the pose.
    pub key: Option<u64>,
    pub mesh: SkinnedMeshId,
    pub skeleton: SkeletonId,
    /// Must use a skinned shader ([`SkinnedShaderId`](crate::SkinnedShaderId)).
    pub material: MaterialId,
    pub transform: Transform,
    pub color: Vec4,
    /// Where this draw's pose sits in the world's packed poses.
    pub(crate) pose: Span,
    /// This draw's `struct Instance` values in the world's packed array.
    pub(crate) params: Span,
}

impl SkinnedDraw {
    pub fn new(
        mesh: SkinnedMeshId,
        skeleton: SkeletonId,
        material: MaterialId,
        transform: impl Into<Transform>,
    ) -> Self {
        Self {
            key: None,
            mesh,
            skeleton,
            material,
            transform: transform.into(),
            color: Vec4::ONE,
            pose: Span::default(),
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
}
