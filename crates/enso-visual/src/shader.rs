//! Shader descriptions, composition and error-tolerant compilation.
//!
//! A user shader is never compiled on its own. It is glued together as:
//!
//! ```text
//!   GLOBALS_WGSL           (shared by all subrenderers: group 0)
//! + <shared parts>         (e.g. MATERIAL_WGSL for mesh-like subrenderers: group 1)
//! + <subrenderer prelude>  (that subrenderer's contract: vertex inputs, helpers)
//! + <user source>          (your vs_main / fs_main)
//! ```
//!
//! Compilation errors are *returned*, not panicked on, so a typo in a shader
//! during a live set leaves the previous shader running instead of crashing.

use crate::{RenderError, subrenderer::RenderContext};

/// Frame-wide uniforms, bound at `@group(0)` for every subrenderer.
/// Must match `GlobalsUniform` in `renderer.rs` byte for byte.
pub const GLOBALS_WGSL: &str = r#"
// ---- enso globals (group 0) ----
struct Globals {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,  // clip space -> world space
    camera_position: vec4<f32>,  // xyz = eye position
    resolution: vec2<f32>,       // render target size in pixels
    time: f32,                   // interpolated simulation time, seconds
    delta_time: f32,             // time since the previous rendered frame
};
@group(0) @binding(0) var<uniform> globals: Globals;
"#;

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

/// A user shader for a mesh-like subrenderer (static or skinned meshes).
#[derive(Debug, Clone)]
pub struct ShaderDesc<'a> {
    pub label: &'a str,
    /// Your WGSL: must define `vs_main` and `fs_main` (see the
    /// subrenderer's `PRELUDE`).
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

/// `GLOBALS_WGSL` + each of `parts` + the user's source.
pub(crate) fn compose(parts: &[&str], user_source: &str) -> String {
    let mut out = String::from(GLOBALS_WGSL);
    for part in parts {
        out.push_str(part);
    }
    out.push_str("\n// ---- user shader ----\n");
    out.push_str(user_source);
    out
}

/// Run `build` while capturing wgpu validation errors, returning them as a
/// [`RenderError::ShaderCompile`] instead of letting wgpu's default handler
/// panic. Use around `create_shader_module` + `create_render_pipeline`.
pub(crate) fn capture_errors<T>(
    device: &wgpu::Device,
    label: &str,
    build: impl FnOnce() -> T,
) -> Result<T, RenderError> {
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let value = build();
    match pollster::block_on(scope.pop()) {
        None => Ok(value),
        Some(err) => Err(RenderError::ShaderCompile {
            label: label.to_owned(),
            message: err.to_string(),
        }),
    }
}

/// Build the pipeline for a mesh-like shader: user `vs_main`/`fs_main`,
/// triangle list, depth test, blending per [`BlendMode`].
pub(crate) fn mesh_pipeline(
    ctx: &RenderContext,
    layout: &wgpu::PipelineLayout,
    parts: &[&str],
    desc: &ShaderDesc<'_>,
    vertex_buffers: &[Option<wgpu::VertexBufferLayout<'_>>],
) -> Result<wgpu::RenderPipeline, RenderError> {
    let device = &ctx.gpu.device;
    let source = compose(parts, desc.source);

    let blend = match desc.blend {
        BlendMode::Opaque => wgpu::BlendState::REPLACE,
        BlendMode::Alpha => wgpu::BlendState::ALPHA_BLENDING,
        BlendMode::Additive => {
            let add = wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            };
            wgpu::BlendState { color: add, alpha: add }
        }
    };

    capture_errors(device, desc.label, || {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(desc.label),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });

        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(desc.label),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: vertex_buffers,
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: (!desc.double_sided).then_some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: ctx.depth_format,
                depth_write_enabled: Some(desc.blend == BlendMode::Opaque),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: ctx.color_format,
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    })
}
