//! The sky subrenderer (private implementation).

use std::collections::HashMap;

use glam::Vec4;
use wgpu::util::DeviceExt;

use super::{PRELUDE, Sky, SkyShaderDesc};
use crate::{
    RenderError, SkyShaderId,
    shader,
    subrenderer::{FrameData, RenderContext, SubRenderer},
    world::MaterialParams,
};

pub(crate) struct SkyRenderer {
    ctx: RenderContext,
    pipeline_layout: wgpu::PipelineLayout,
    params_buffer: wgpu::Buffer,
    params_bind_group: wgpu::BindGroup,
    shaders: HashMap<SkyShaderId, wgpu::RenderPipeline>,

    /// The shader to draw this frame, decided in `prepare`.
    current: Option<SkyShaderId>,
    /// What is currently in `params_buffer`, to skip redundant uploads.
    uploaded_params: MaterialParams,
}

impl SkyRenderer {
    pub(crate) fn new(ctx: &RenderContext) -> Self {
        let device = &ctx.gpu.device;

        let params_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sky params layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(size_of::<MaterialParams>() as u64),
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sky pipeline layout"),
            bind_group_layouts: &[Some(&ctx.globals_layout), Some(&params_layout)],
            immediate_size: 0,
        });

        let uploaded_params = [Vec4::ZERO; 4];
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sky params"),
            contents: bytemuck::cast_slice(&uploaded_params),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let params_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sky params"),
            layout: &params_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: params_buffer.as_entire_binding() }],
        });

        Self {
            ctx: ctx.clone(),
            pipeline_layout,
            params_buffer,
            params_bind_group,
            shaders: HashMap::new(),
            current: None,
            uploaded_params,
        }
    }

    pub(crate) fn create_shader(&mut self, desc: &SkyShaderDesc<'_>) -> Result<SkyShaderId, RenderError> {
        let pipeline = self.build_pipeline(desc)?;
        let id = SkyShaderId::next();
        self.shaders.insert(id, pipeline);
        Ok(id)
    }

    /// Recompile in place; on error the old version stays.
    pub(crate) fn update_shader(&mut self, id: SkyShaderId, desc: &SkyShaderDesc<'_>) -> Result<(), RenderError> {
        if !self.shaders.contains_key(&id) {
            return Err(RenderError::UnknownSkyShader(id));
        }
        let pipeline = self.build_pipeline(desc)?;
        self.shaders.insert(id, pipeline);
        Ok(())
    }

    pub(crate) fn destroy_shader(&mut self, id: SkyShaderId) {
        self.shaders.remove(&id);
    }

    fn build_pipeline(&self, desc: &SkyShaderDesc<'_>) -> Result<wgpu::RenderPipeline, RenderError> {
        let device = &self.ctx.gpu.device;
        let source = shader::compose(PRELUDE, desc.source);

        shader::capture_errors(device, desc.label, || {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(desc.label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
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
        })
    }
}

impl SubRenderer for SkyRenderer {
    fn prepare(&mut self, frame: &FrameData<'_>, _encoder: &mut wgpu::CommandEncoder) {
        self.current = None;
        let Some(curr) = &frame.curr.sky else { return };
        let sky = Sky::interpolate(frame.prev.sky.as_ref(), curr, frame.alpha);

        // A destroyed (or never created) shader just doesn't draw.
        if !self.shaders.contains_key(&sky.shader) {
            return;
        }
        self.current = Some(sky.shader);

        if sky.params != self.uploaded_params {
            self.ctx
                .gpu
                .queue
                .write_buffer(&self.params_buffer, 0, bytemuck::cast_slice(&sky.params));
            self.uploaded_params = sky.params;
        }
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, globals: &wgpu::BindGroup) {
        let Some(pipeline) = self.current.and_then(|id| self.shaders.get(&id)) else { return };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, globals, &[]);
        pass.set_bind_group(1, &self.params_bind_group, &[]);
        pass.draw(0..3, 0..1); // the fullscreen triangle
    }
}
