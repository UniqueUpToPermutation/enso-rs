//! The sky: one fullscreen triangle behind everything else, shaded by a
//! user-written fragment shader.
//!
//! Public contract only; the subrenderer itself is private. The shader side
//! of the contract is [`PRELUDE`] (`prelude.wgsl`).

mod renderer;

use glam::Vec4;

pub(crate) use renderer::SkyRenderer;

use crate::{SkyShaderId, world::MaterialParams};

/// The WGSL contract prepended to every sky shader. It provides the vertex
/// shader; you write `fs_main`.
pub const PRELUDE: &str = include_str!("prelude.wgsl");

/// Everything needed to build a sky shader.
#[derive(Debug, Clone)]
pub struct SkyShaderDesc<'a> {
    pub label: &'a str,
    /// Your WGSL: must define `fs_main(in: SkyIn) -> @location(0) vec4<f32>`.
    pub source: &'a str,
}

impl<'a> SkyShaderDesc<'a> {
    pub fn new(label: &'a str, source: &'a str) -> Self {
        Self { label, source }
    }
}

/// The sky of a [`RenderWorld`](crate::RenderWorld).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sky {
    pub shader: SkyShaderId,
    /// Free knobs, `sky.params[0..4]` in WGSL. Interpolated between ticks
    /// when both ticks use the same shader.
    pub params: MaterialParams,
}

impl Sky {
    pub fn new(shader: SkyShaderId) -> Self {
        Self { shader, params: [Vec4::ZERO; 4] }
    }

    pub fn with_params(mut self, params: MaterialParams) -> Self {
        self.params = params;
        self
    }

    /// The sky to draw between two ticks.
    pub(crate) fn interpolate(prev: Option<&Self>, curr: &Self, t: f32) -> Self {
        match prev {
            Some(prev) if prev.shader == curr.shader => Self {
                shader: curr.shader,
                params: std::array::from_fn(|i| prev.params[i].lerp(curr.params[i], t)),
            },
            _ => *curr,
        }
    }
}
