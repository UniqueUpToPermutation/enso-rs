//! The sky subrenderer (private implementation).

use std::{collections::HashMap, sync::Arc};

use super::{PRELUDE, SkyShaderDesc};
use crate::{
    MaterialId, RenderError, SkyShaderId,
    material::{MATERIAL_WGSL, MaterialShader},
    params::ParamLayout,
    shader,
    subrenderer::{DrawData, FrameData, RenderContext, SubRenderer},
};

struct ShaderEntry {
    pipeline: wgpu::RenderPipeline,
    /// Its `struct Material`.
    material_layout: Arc<ParamLayout>,
}

pub(crate) struct SkyRenderer {
    ctx: RenderContext,
    pipeline_layout: wgpu::PipelineLayout,
    shaders: HashMap<SkyShaderId, ShaderEntry>,
    /// What to draw this frame, decided in `prepare`.
    current: Option<(SkyShaderId, MaterialId)>,
}

impl SkyRenderer {
    pub(crate) fn new(ctx: &RenderContext) -> Self {
        let pipeline_layout = ctx.gpu.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sky pipeline layout"),
            bind_group_layouts: &[Some(&ctx.globals_layout), Some(&ctx.material_layout)],
            immediate_size: 0,
        });
        Self { ctx: ctx.clone(), pipeline_layout, shaders: HashMap::new(), current: None }
    }

    pub(crate) fn material_layout(&self, id: SkyShaderId) -> Option<Arc<ParamLayout>> {
        self.shaders.get(&id).map(|s| s.material_layout.clone())
    }

    pub(crate) fn create_shader(&mut self, desc: &SkyShaderDesc<'_>) -> Result<SkyShaderId, RenderError> {
        let entry = self.build_shader(desc)?;
        let id = SkyShaderId::next();
        self.shaders.insert(id, entry);
        Ok(id)
    }

    /// Recompile in place and return the (possibly changed) `struct
    /// Material` layout. On error the old version stays.
    pub(crate) fn update_shader(
        &mut self,
        id: SkyShaderId,
        desc: &SkyShaderDesc<'_>,
    ) -> Result<Arc<ParamLayout>, RenderError> {
        if !self.shaders.contains_key(&id) {
            return Err(RenderError::UnknownSkyShader(id));
        }
        let entry = self.build_shader(desc)?;
        let layout = entry.material_layout.clone();
        self.shaders.insert(id, entry);
        Ok(layout)
    }

    pub(crate) fn destroy_shader(&mut self, id: SkyShaderId) {
        self.shaders.remove(&id);
    }

    fn build_shader(&self, desc: &SkyShaderDesc<'_>) -> Result<ShaderEntry, RenderError> {
        let device = &self.ctx.gpu.device;
        let source = shader::compose_with_structs(&[MATERIAL_WGSL, PRELUDE], desc.source, &["Material"]);

        let pipeline = shader::capture_errors(device, desc.label, || {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(desc.label),
                source: wgpu::ShaderSource::Wgsl(source.as_str().into()),
            });

            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(desc.label),
                layout: Some(&self.pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("sky_vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[], // positions come from @builtin(vertex_index)
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                // The pass has a depth buffer, so the pipeline must declare
                // it, but the sky neither tests against nor writes depth:
                // it is drawn first and everything else lands on top.
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: self.ctx.depth_format,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: self.ctx.color_format,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        })?;

        Ok(ShaderEntry {
            pipeline,
            material_layout: Arc::new(shader::reflect_params(desc.label, &source, 1)?),
        })
    }
}

impl SubRenderer for SkyRenderer {
    fn prepare(&mut self, frame: &FrameData<'_>, _encoder: &mut wgpu::CommandEncoder) {
        // The material's knobs were already interpolated and uploaded by the
        // material store; all that's left is to pick the pipeline.
        self.current = frame.curr.sky.and_then(|material_id| {
            let material = frame.materials.get(material_id)?;
            // A material for another subrenderer, or a destroyed shader,
            // just doesn't draw.
            let MaterialShader::Sky(shader_id) = material.shader else { return None };
            self.shaders.contains_key(&shader_id).then_some((shader_id, material_id))
        });
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, data: &DrawData<'_>) {
        let Some((shader_id, material_id)) = self.current else { return };
        let Some(material) = data.materials.get(material_id) else { return };
        pass.set_pipeline(&self.shaders[&shader_id].pipeline);
        pass.set_bind_group(0, data.globals, &[]);
        pass.set_bind_group(1, &material.bind_group, &[]);
        pass.draw(0..3, 0..1); // the fullscreen triangle
    }
}
