//! The sprite subrenderer (private implementation).

use std::{collections::HashMap, sync::Arc};

use super::{PRELUDE, SpriteDraw};
use crate::{
    BlendMode, RenderError, ShaderDesc, SpriteShaderId,
    instancing::{BatchKey, InstanceBatches, InstanceData},
    material::{MATERIAL_WGSL, MaterialShader},
    params::ParamLayout,
    shader,
    subrenderer::{DrawData, FrameData, RenderContext, SubRenderer},
};

const INSTANCE_PARAMS_GROUP: u32 = 2;

/// One sprite. Matches `SpriteIn` in `prelude.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SpriteRaw {
    model: [[f32; 4]; 4],
    color: [f32; 4],
    uv_rect: [f32; 4],
    size_anchor: [f32; 4],
    spin: f32,
    billboard: u32,
    param_slot: u32,
}

impl InstanceData for SpriteRaw {
    fn set_param_slot(&mut self, slot: u32) {
        self.param_slot = slot;
    }
}

impl From<&SpriteDraw> for SpriteRaw {
    fn from(d: &SpriteDraw) -> Self {
        Self {
            model: d.transform.to_matrix().to_cols_array_2d(),
            color: d.color.to_array(),
            uv_rect: d.uv_rect,
            size_anchor: [d.size.x, d.size.y, d.anchor.x, d.anchor.y],
            spin: d.spin,
            billboard: d.billboard.index(),
            param_slot: 0, // assigned in upload
        }
    }
}

const SPRITE_ATTRIBUTES: [wgpu::VertexAttribute; 10] = wgpu::vertex_attr_array![
    0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, // model matrix columns
    4 => Float32x4, // color
    5 => Float32x4, // uv_rect
    6 => Float32x4, // size_anchor
    7 => Float32,   // spin
    8 => Uint32,    // billboard
    9 => Uint32,    // param_slot
];

fn vertex_buffers() -> [Option<wgpu::VertexBufferLayout<'static>>; 1] {
    [Some(wgpu::VertexBufferLayout {
        array_stride: size_of::<SpriteRaw>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &SPRITE_ATTRIBUTES,
    })]
}

struct ShaderEntry {
    blend: BlendMode,
    pipeline: wgpu::RenderPipeline,
    material_layout: Arc<ParamLayout>,
    instance_layout: ParamLayout,
}

pub(crate) struct SpriteRenderer {
    ctx: RenderContext,
    pipeline_layout: wgpu::PipelineLayout,
    shaders: HashMap<SpriteShaderId, ShaderEntry>,
    /// Sprites have no mesh: the batch key's mesh slot is `()`.
    instances: InstanceBatches<SpriteShaderId, (), SpriteRaw>,
    prev_by_key: HashMap<u64, usize>,
}

impl SpriteRenderer {
    pub(crate) fn new(ctx: &RenderContext) -> Self {
        let instances = InstanceBatches::new(&ctx.gpu.device, "sprites");
        let pipeline_layout = ctx.gpu.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sprite pipeline layout"),
            bind_group_layouts: &[
                Some(&ctx.globals_layout),
                Some(&ctx.material_layout),
                Some(instances.params_layout()),
            ],
            immediate_size: 0,
        });
        Self { ctx: ctx.clone(), pipeline_layout, shaders: HashMap::new(), instances, prev_by_key: HashMap::new() }
    }

    pub(crate) fn material_layout(&self, id: SpriteShaderId) -> Option<Arc<ParamLayout>> {
        self.shaders.get(&id).map(|s| s.material_layout.clone())
    }

    pub(crate) fn instance_layout(&self, id: SpriteShaderId) -> Option<&ParamLayout> {
        self.shaders.get(&id).map(|s| &s.instance_layout)
    }

    pub(crate) fn create_shader(&mut self, desc: &ShaderDesc<'_>) -> Result<SpriteShaderId, RenderError> {
        let entry = self.build_shader(desc)?;
        let id = SpriteShaderId::next();
        self.shaders.insert(id, entry);
        Ok(id)
    }

    pub(crate) fn update_shader(
        &mut self,
        id: SpriteShaderId,
        desc: &ShaderDesc<'_>,
    ) -> Result<Arc<ParamLayout>, RenderError> {
        if !self.shaders.contains_key(&id) {
            return Err(RenderError::UnknownSpriteShader(id));
        }
        let entry = self.build_shader(desc)?;
        let layout = entry.material_layout.clone();
        self.shaders.insert(id, entry);
        Ok(layout)
    }

    pub(crate) fn destroy_shader(&mut self, id: SpriteShaderId) {
        self.shaders.remove(&id);
    }

    fn build_shader(&self, desc: &ShaderDesc<'_>) -> Result<ShaderEntry, RenderError> {
        // Fixed sprites can be seen from behind; billboards never need culling.
        let desc = ShaderDesc { double_sided: true, ..desc.clone() };
        let source = shader::compose_mesh(&[MATERIAL_WGSL, PRELUDE], desc.source);
        let pipeline = shader::mesh_pipeline(&self.ctx, &self.pipeline_layout, &source, &desc, &vertex_buffers())?;
        Ok(ShaderEntry {
            blend: desc.blend,
            pipeline,
            material_layout: Arc::new(shader::reflect_params(desc.label, &source, 1)?),
            instance_layout: shader::reflect_params(desc.label, &source, INSTANCE_PARAMS_GROUP)?,
        })
    }
}

impl SubRenderer for SpriteRenderer {
    fn prepare(&mut self, frame: &FrameData<'_>, _encoder: &mut wgpu::CommandEncoder) {
        self.prev_by_key.clear();
        for (index, draw) in frame.prev.sprites.iter().enumerate() {
            if let Some(key) = draw.key {
                self.prev_by_key.insert(key, index);
            }
        }

        self.instances.clear();
        for curr in &frame.curr.sprites {
            // Missing resources (or a material for another subrenderer)
            // just don't draw.
            let Some(material) = frame.materials.get(curr.material) else { continue };
            let MaterialShader::Sprite(shader_id) = material.shader else { continue };
            let Some(shader) = self.shaders.get(&shader_id) else { continue };

            let prev = curr.key.and_then(|k| self.prev_by_key.get(&k)).map(|&i| &frame.prev.sprites[i]);
            let draw = prev.map_or(*curr, |prev| SpriteDraw::interpolate(prev, curr, frame.alpha));
            let prev_values = prev.map_or(&[][..], |prev| frame.prev.instance_values(prev.params));
            let curr_values = frame.curr.instance_values(curr.params);

            let key = BatchKey {
                blended: shader.blend != BlendMode::Opaque,
                shader: shader_id,
                material: curr.material,
                mesh: (),
            };
            self.instances.push(key, SpriteRaw::from(&draw), |out| {
                shader.instance_layout.pack(prev_values, curr_values, frame.alpha, out);
            });
        }
        self.instances.upload(&self.ctx.gpu);
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, data: &DrawData<'_>) {
        let batches = self.instances.batches();
        if batches.is_empty() {
            return;
        }
        pass.set_bind_group(0, data.globals, &[]);
        pass.set_vertex_buffer(0, self.instances.instance_buffer().slice(..));

        let mut current: Option<BatchKey<SpriteShaderId, ()>> = None;
        for batch in batches {
            let key = batch.key;
            if current.is_none_or(|c| c.shader != key.shader) {
                pass.set_pipeline(&self.shaders[&key.shader].pipeline);
                pass.set_bind_group(INSTANCE_PARAMS_GROUP, self.instances.segment(batch.segment), &[]);
            }
            if current.is_none_or(|c| c.material != key.material) {
                let material = data.materials.get(key.material).expect("checked in prepare");
                pass.set_bind_group(1, &material.bind_group, &[]);
            }
            current = Some(key);
            pass.draw(0..6, batch.instances.clone()); // one quad per sprite
        }
    }
}
