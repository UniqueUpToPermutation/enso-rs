//! The static mesh subrenderer (private implementation).

use std::{collections::HashMap, sync::Arc};

use wgpu::util::DeviceExt;

use super::{MeshData, MeshDraw, PRELUDE, Vertex};
use crate::{
    BlendMode, MeshId, MeshShaderId, RenderError, ShaderDesc,
    instancing::{BatchKey, InstanceBatches, InstanceData},
    material::{MATERIAL_WGSL, MaterialShader},
    params::ParamLayout,
    shader,
    subrenderer::{DrawData, FrameData, RenderContext, SubRenderer},
};

/// Bind group of the per-instance parameter segment (`instances`).
const INSTANCE_PARAMS_GROUP: u32 = 2;

/// Matches the fixed part of `InstanceIn` in `prelude.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct InstanceRaw {
    model: [[f32; 4]; 4],
    color: [f32; 4],
    param_slot: u32,
}

impl InstanceData for InstanceRaw {
    fn set_param_slot(&mut self, slot: u32) {
        self.param_slot = slot;
    }
}

const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];

const INSTANCE_ATTRIBUTES: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
    3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4, // model matrix columns
    7 => Float32x4, // color
    8 => Uint32,    // param_slot
];

fn vertex_buffers() -> [Option<wgpu::VertexBufferLayout<'static>>; 2] {
    [
        Some(wgpu::VertexBufferLayout {
            array_stride: size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &VERTEX_ATTRIBUTES,
        }),
        Some(wgpu::VertexBufferLayout {
            array_stride: size_of::<InstanceRaw>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &INSTANCE_ATTRIBUTES,
        }),
    ]
}

struct ShaderEntry {
    blend: BlendMode,
    pipeline: wgpu::RenderPipeline,
    /// Its `struct Material`.
    material_layout: Arc<ParamLayout>,
    /// Its `struct Instance`.
    instance_layout: ParamLayout,
}

struct MeshEntry {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

pub(crate) struct StaticMeshRenderer {
    ctx: RenderContext,
    pipeline_layout: wgpu::PipelineLayout,
    shaders: HashMap<MeshShaderId, ShaderEntry>,
    meshes: HashMap<MeshId, MeshEntry>,
    instances: InstanceBatches<MeshShaderId, MeshId, InstanceRaw>,
    /// Scratch: key -> index into the previous world's draws.
    prev_by_key: HashMap<u64, usize>,
}

impl StaticMeshRenderer {
    pub(crate) fn new(ctx: &RenderContext) -> Self {
        let instances = InstanceBatches::new(&ctx.gpu.device, "static mesh instances");
        let pipeline_layout = ctx.gpu.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("static mesh pipeline layout"),
            bind_group_layouts: &[
                Some(&ctx.globals_layout),
                Some(&ctx.material_layout),
                Some(instances.params_layout()),
            ],
            immediate_size: 0,
        });
        Self {
            ctx: ctx.clone(),
            pipeline_layout,
            shaders: HashMap::new(),
            meshes: HashMap::new(),
            instances,
            prev_by_key: HashMap::new(),
        }
    }

    // ---- shaders ----

    pub(crate) fn material_layout(&self, id: MeshShaderId) -> Option<Arc<ParamLayout>> {
        self.shaders.get(&id).map(|s| s.material_layout.clone())
    }

    pub(crate) fn instance_layout(&self, id: MeshShaderId) -> Option<&ParamLayout> {
        self.shaders.get(&id).map(|s| &s.instance_layout)
    }

    pub(crate) fn create_shader(&mut self, desc: &ShaderDesc<'_>) -> Result<MeshShaderId, RenderError> {
        let entry = self.build_shader(desc)?;
        let id = MeshShaderId::next();
        self.shaders.insert(id, entry);
        Ok(id)
    }

    /// Recompile in place and return the (possibly changed) `struct
    /// Material` layout, so materials can follow. On error the old version stays.
    pub(crate) fn update_shader(
        &mut self,
        id: MeshShaderId,
        desc: &ShaderDesc<'_>,
    ) -> Result<Arc<ParamLayout>, RenderError> {
        if !self.shaders.contains_key(&id) {
            return Err(RenderError::UnknownShader(id));
        }
        let entry = self.build_shader(desc)?;
        let layout = entry.material_layout.clone();
        self.shaders.insert(id, entry);
        Ok(layout)
    }

    pub(crate) fn destroy_shader(&mut self, id: MeshShaderId) {
        self.shaders.remove(&id);
    }

    fn build_shader(&self, desc: &ShaderDesc<'_>) -> Result<ShaderEntry, RenderError> {
        let source = shader::compose_mesh(&[MATERIAL_WGSL, PRELUDE], desc.source);
        let pipeline = shader::mesh_pipeline(&self.ctx, &self.pipeline_layout, &source, desc, &vertex_buffers())?;
        Ok(ShaderEntry {
            blend: desc.blend,
            pipeline,
            material_layout: Arc::new(shader::reflect_params(desc.label, &source, 1)?),
            instance_layout: shader::reflect_params(desc.label, &source, INSTANCE_PARAMS_GROUP)?,
        })
    }

    // ---- meshes ----

    pub(crate) fn create_mesh(&mut self, data: &MeshData) -> MeshId {
        let device = &self.ctx.gpu.device;
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("static mesh vertices"),
            contents: bytemuck::cast_slice(&data.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("static mesh indices"),
            contents: bytemuck::cast_slice(&data.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let id = MeshId::next();
        self.meshes.insert(
            id,
            MeshEntry { vertex_buffer, index_buffer, index_count: data.indices.len() as u32 },
        );
        id
    }

    pub(crate) fn destroy_mesh(&mut self, id: MeshId) {
        self.meshes.remove(&id);
    }
}

impl SubRenderer for StaticMeshRenderer {
    fn prepare(&mut self, frame: &FrameData<'_>, _encoder: &mut wgpu::CommandEncoder) {
        self.prev_by_key.clear();
        for (index, draw) in frame.prev.meshes.iter().enumerate() {
            if let Some(key) = draw.key {
                self.prev_by_key.insert(key, index);
            }
        }

        self.instances.clear();
        for curr in &frame.curr.meshes {
            // Missing resources (or a material for another subrenderer)
            // just don't draw.
            let Some(material) = frame.materials.get(curr.material) else { continue };
            let MaterialShader::Mesh(shader_id) = material.shader else { continue };
            let Some(shader) = self.shaders.get(&shader_id) else { continue };
            if !self.meshes.contains_key(&curr.mesh) {
                continue;
            }

            let prev = curr.key.and_then(|k| self.prev_by_key.get(&k)).map(|&i| &frame.prev.meshes[i]);
            let draw = prev.map_or(*curr, |prev| MeshDraw::interpolate(prev, curr, frame.alpha));
            let prev_values = prev.map_or(&[][..], |prev| frame.prev.instance_values(prev.params));
            let curr_values = frame.curr.instance_values(curr.params);

            let key = BatchKey {
                blended: shader.blend != BlendMode::Opaque,
                shader: shader_id,
                material: curr.material,
                mesh: curr.mesh,
            };
            let instance = InstanceRaw {
                model: draw.transform.to_matrix().to_cols_array_2d(),
                color: draw.color.to_array(),
                param_slot: 0, // assigned in upload
            };
            self.instances.push(key, instance, |out| {
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
        pass.set_vertex_buffer(1, self.instances.instance_buffer().slice(..));

        let mut current: Option<BatchKey<MeshShaderId, MeshId>> = None;
        for batch in batches {
            let key = batch.key;
            // Lookups can't fail: prepare filtered out missing resources
            // and nothing can be destroyed between prepare and draw.
            let mesh = &self.meshes[&key.mesh];

            if current.is_none_or(|c| c.shader != key.shader) {
                pass.set_pipeline(&self.shaders[&key.shader].pipeline);
                pass.set_bind_group(INSTANCE_PARAMS_GROUP, self.instances.segment(batch.segment), &[]);
            }
            if current.is_none_or(|c| c.material != key.material) {
                let material = data.materials.get(key.material).expect("checked in prepare");
                pass.set_bind_group(1, &material.bind_group, &[]);
            }
            if current.is_none_or(|c| c.mesh != key.mesh) {
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            }
            current = Some(key);

            pass.draw_indexed(0..mesh.index_count, 0, batch.instances.clone());
        }
    }
}
