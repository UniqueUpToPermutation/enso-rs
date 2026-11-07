//! The sky: one fullscreen triangle behind everything else, shaded by a
//! user-written fragment shader.
//!
//! The vertex stage is fixed (it lives in [`PRELUDE`]) and hands the
//! fragment shader the view direction for each pixel. Skies use ordinary
//! materials: create one from a [`SkyShaderId`](crate::SkyShaderId), set its
//! `struct Material` knobs with
//! [`RenderWorld::set_param`](crate::RenderWorld::set_param), and show it
//! with `world.sky = Some(material)`.

mod renderer;

pub(crate) use renderer::SkyRenderer;

/// The WGSL contract prepended to every sky shader (after the globals and
/// material declarations). It provides the vertex shader; you write `fs_main`.
pub const PRELUDE: &str = include_str!("prelude.wgsl");

/// Everything needed to build a sky shader.
#[derive(Debug, Clone)]
pub struct SkyShaderDesc<'a> {
    pub label: &'a str,
    /// Your WGSL: must define `fs_main(in: SkyIn) -> @location(0) vec4<f32>`,
    /// and may declare `struct Material { .. }`.
    pub source: &'a str,
}

impl<'a> SkyShaderDesc<'a> {
    pub fn new(label: &'a str, source: &'a str) -> Self {
        Self { label, source }
    }
}
