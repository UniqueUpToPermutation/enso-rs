//! The internal subrenderer contract.
//!
//! A subrenderer owns one *kind* of drawable data (sky, static meshes,
//! skinned meshes, ...). For its kind it defines:
//!
//! * the **vertex contract**: which vertex/instance attributes reach the
//!   vertex shader, and in what `@location`s;
//! * its **bind groups** beyond group 0 (mesh-like subrenderers share the
//!   material layout at group 1);
//! * a **WGSL prelude** declaring all of the above, which is prepended to
//!   every user shader written for it;
//! * how to read its section of the two [`RenderWorld`]s, interpolate it,
//!   and draw it.
//!
//! Subrenderers are private to this crate. Users only see the
//! [`Renderer`](crate::Renderer) facade and the data types that go into a
//! `RenderWorld`.

use crate::{Gpu, RenderWorld, material::MaterialStore};

/// Shared GPU state a subrenderer needs to build its objects.
#[derive(Debug, Clone)]
pub(crate) struct RenderContext {
    pub gpu: Gpu,
    /// Format of the colour target (the window surface).
    pub color_format: wgpu::TextureFormat,
    /// Format of the shared depth buffer.
    pub depth_format: wgpu::TextureFormat,
    /// Layout of the frame globals bind group (always `@group(0)`).
    pub globals_layout: wgpu::BindGroupLayout,
    /// Layout of material bind groups (`@group(1)` for mesh-like subrenderers).
    pub material_layout: wgpu::BindGroupLayout,
}

/// The two simulation ticks being rendered between.
pub(crate) struct FrameData<'a> {
    pub prev: &'a RenderWorld,
    pub curr: &'a RenderWorld,
    /// 0 = `prev`, 1 = `curr`.
    pub alpha: f32,
    /// Material params for this frame are already uploaded.
    pub materials: &'a MaterialStore,
}

/// Shared state available while recording draws.
pub(crate) struct DrawData<'a> {
    /// Bind at `@group(0)`.
    pub globals: &'a wgpu::BindGroup,
    pub materials: &'a MaterialStore,
}

/// Frame lifecycle, called by the renderer for every subrenderer in order:
/// first `prepare` for all, then `draw` for all inside one render pass.
pub(crate) trait SubRenderer {
    /// Read this subrenderer's part of the frame, interpolate it, and upload
    /// the result (instance buffers, uniforms). Runs before the render pass.
    fn prepare(&mut self, frame: &FrameData<'_>, encoder: &mut wgpu::CommandEncoder);

    /// Record draw calls. Each subrenderer binds globals at group 0 itself.
    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, data: &DrawData<'_>);
}
