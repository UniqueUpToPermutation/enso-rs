//! Sprites: textured quads, optionally billboarded (turned to face the
//! camera).
//!
//! * [`Billboard::Fixed`]: a flat quad oriented by its transform (decals,
//!   cards, panels).
//! * [`Billboard::Camera`]: always parallel to the screen (particles, glows).
//! * [`Billboard::Upright`]: turns only around the world's vertical axis, so
//!   it stays standing when seen from above (trees, posts, characters).
//!
//! The texture is the material's `texture0`; sprite shaders are written
//! against [`PRELUDE`], whose `sprite_corner()` does the billboard maths.

mod renderer;

use glam::{Vec2, Vec4};

pub(crate) use renderer::SpriteRenderer;

use crate::{MaterialId, Transform, world::Span};

/// The WGSL contract prepended to every sprite shader (after the globals
/// and material declarations).
pub const PRELUDE: &str = include_str!("prelude.wgsl");

/// How a sprite is oriented.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Billboard {
    /// Oriented by the transform: the quad lies in its local XY plane,
    /// facing +Z.
    #[default]
    Fixed,
    /// Faces the camera: parallel to the screen, upright on screen.
    Camera,
    /// Faces the camera around the world's Y axis only; stays vertical.
    Upright,
}

impl Billboard {
    fn index(self) -> u32 {
        match self {
            Self::Fixed => 0,
            Self::Camera => 1,
            Self::Upright => 2,
        }
    }
}

/// One sprite in a [`RenderWorld`](crate::RenderWorld). Its fields reach the
/// shader as `SpriteIn` (see [`PRELUDE`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpriteDraw {
    /// Stable identity across ticks; matched draws interpolate transform,
    /// size, spin, colour and parameters.
    pub key: Option<u64>,
    /// Must use a sprite shader ([`SpriteShaderId`](crate::SpriteShaderId)).
    pub material: MaterialId,
    /// Position and scale (and, for [`Billboard::Fixed`], rotation).
    pub transform: Transform,
    /// Width and height, in world units (before the transform's scale).
    pub size: Vec2,
    /// The point of the quad placed at the transform's position: (0, 0) =
    /// bottom-left, (0.5, 0.5) = centre, (0.5, 0) = bottom-centre (for
    /// things standing on the ground).
    pub anchor: Vec2,
    /// Rotation within the sprite's own plane, in radians (counter-clockwise).
    pub spin: f32,
    /// The part of the texture to show: `[u0, v0, u1, v1]`, (u0, v0) =
    /// top-left. Not interpolated, so sprite-sheet frames switch cleanly.
    pub uv_rect: [f32; 4],
    pub color: Vec4,
    pub billboard: Billboard,
    pub(crate) params: Span,
}

impl SpriteDraw {
    pub fn new(material: MaterialId, transform: impl Into<Transform>) -> Self {
        Self {
            key: None,
            material,
            transform: transform.into(),
            size: Vec2::ONE,
            anchor: Vec2::splat(0.5),
            spin: 0.0,
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            color: Vec4::ONE,
            billboard: Billboard::Fixed,
            params: Span::default(),
        }
    }
    pub fn with_key(mut self, key: u64) -> Self {
        self.key = Some(key);
        self
    }
    pub fn with_size(mut self, size: Vec2) -> Self {
        self.size = size;
        self
    }
    pub fn with_anchor(mut self, anchor: Vec2) -> Self {
        self.anchor = anchor;
        self
    }
    pub fn with_spin(mut self, spin: f32) -> Self {
        self.spin = spin;
        self
    }
    pub fn with_uv_rect(mut self, uv_rect: [f32; 4]) -> Self {
        self.uv_rect = uv_rect;
        self
    }
    pub fn with_color(mut self, color: Vec4) -> Self {
        self.color = color;
        self
    }
    pub fn with_billboard(mut self, billboard: Billboard) -> Self {
        self.billboard = billboard;
        self
    }

    /// Blend continuous properties from `prev` towards `curr`; discrete
    /// ones (material, uv_rect, anchor, billboard) come from `curr`.
    pub(crate) fn interpolate(prev: &Self, curr: &Self, t: f32) -> Self {
        Self {
            transform: prev.transform.lerp(&curr.transform, t),
            size: prev.size.lerp(curr.size, t),
            spin: prev.spin + (curr.spin - prev.spin) * t,
            color: prev.color.lerp(curr.color, t),
            ..*curr
        }
    }
}
