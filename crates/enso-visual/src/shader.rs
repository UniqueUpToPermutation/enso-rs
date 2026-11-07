//! Shader composition and error-tolerant compilation.
//!
//! A user shader is never compiled on its own. It is glued together as:
//!
//! ```text
//!   GLOBALS_WGSL          (shared by all subrenderers: group 0)
//! + <subrenderer prelude> (that subrenderer's contract: vertex inputs, group 1, helpers)
//! + <user source>         (your vs_main / fs_main)
//! ```
//!
//! Compilation errors are *returned*, not panicked on, so a typo in a shader
//! during a live set leaves the previous shader running instead of crashing.

use crate::RenderError;

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

/// Concatenate globals + prelude + user source.
pub(crate) fn compose(prelude: &str, user_source: &str) -> String {
    let mut out = String::with_capacity(GLOBALS_WGSL.len() + prelude.len() + user_source.len() + 64);
    out.push_str(GLOBALS_WGSL);
    out.push_str(prelude);
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
