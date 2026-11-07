//! Resource identifiers.
//!
//! IDs are plain `Copy` values with no GPU handles inside, so a
//! [`RenderWorld`](crate::RenderWorld) that refers to them can be built on
//! any thread, stored in ECS components, or (later) serialised in a preset.
//!
//! IDs are never reused, so an ID whose resource was destroyed simply stops
//! drawing; it can never accidentally point at a newer resource.

use std::sync::atomic::{AtomicU64, Ordering};

fn next_raw() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u64);

        impl $name {
            /// A fresh ID, never returned before (across all ID types).
            /// [`Renderer`](crate::Renderer) implementations use this to
            /// name the resources they create.
            pub fn next() -> Self {
                Self(next_raw())
            }
        }
    };
}

define_id!(
    /// A texture, from [`Renderer::create_texture`](crate::Renderer::create_texture).
    TextureId
);
define_id!(
    /// Static mesh geometry, from [`Renderer::create_mesh`](crate::Renderer::create_mesh).
    MeshId
);
define_id!(
    /// A static mesh shader, from
    /// [`Renderer::create_mesh_shader`](crate::Renderer::create_mesh_shader).
    MeshShaderId
);
define_id!(
    /// A sky shader, from
    /// [`Renderer::create_sky_shader`](crate::Renderer::create_sky_shader).
    SkyShaderId
);
define_id!(
    /// Skinned mesh geometry, from
    /// [`Renderer::create_skinned_mesh`](crate::Renderer::create_skinned_mesh).
    SkinnedMeshId
);
define_id!(
    /// A skinned mesh shader, from
    /// [`Renderer::create_skinned_shader`](crate::Renderer::create_skinned_shader).
    SkinnedShaderId
);
define_id!(
    /// A joint hierarchy, from
    /// [`Renderer::create_skeleton`](crate::Renderer::create_skeleton).
    SkeletonId
);
define_id!(
    /// A material (shader + textures + knobs), from
    /// [`Renderer::create_material`](crate::Renderer::create_material).
    MaterialId
);
