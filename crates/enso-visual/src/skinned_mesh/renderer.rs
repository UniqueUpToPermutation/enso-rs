//! The skinned mesh subrenderer (private implementation).

use std::{collections::HashMap, sync::Arc};

use glam::Mat4;
use wgpu::util::DeviceExt;

use super::{PRELUDE, SkeletonDesc, SkinnedMeshData, SkinnedVertex, skeleton::Skeleton};
use crate::{
    BlendMode, RenderError, ShaderDesc, SkeletonId, SkinnedMeshId, SkinnedShaderId, Transform,
    instancing::{BatchKey, InstanceBatches, InstanceData},
    material::{MATERIAL_WGSL, MaterialShader},
    params::ParamLayout,
    shader,
    subrenderer::{DrawData, FrameData, RenderContext, SubRenderer},
};

/// Bind group of the joint matrices.
const JOINTS_GROUP: u32 = 2;
/// Bind group of the per-instance parameter segment (`instances`).
const INSTANCE_PARAMS_GROUP: u32 = 3;

/// Matches the fixed part of `InstanceIn` in `prelude.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct InstanceRaw {
    model: [[f32; 4]; 4],
    color: [f32; 4],
    joint_offset: u32,
    param_slot: u32,
}

impl InstanceData for InstanceRaw {
    fn set_param_slot(&mut self, slot: u32) {
        self.param_slot = slot;
    }
}

const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    0 => Float32x3, // position
    1 => Float32x3, // normal
    2 => Float32x2, // uv
    3 => Uint16x4,  // joints (vec4<u32> in WGSL)
    4 => Float32x4, // weights
];

const INSTANCE_ATTRIBUTES: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![
    5 => Float32x4, 6 => Float32x4, 7 => Float32x4, 8 => Float32x4, // model matrix columns
    9 => Float32x4, // color
    10 => Uint32,   // joint_offset
    11 => Uint32,   // param_slot
];

fn vertex_buffers() -> [Option<wgpu::VertexBufferLayout<'static>>; 2] {
    [
        Some(wgpu::VertexBufferLayout {
            array_stride: size_of::<SkinnedVertex>() as u64,
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

/// All instances' skinning matrices for one frame, in a storage buffer.
struct JointBuffer {
    layout: wgpu::BindGroupLayout,
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    /// In matrices.
    capacity: usize,
}

impl JointBuffer {
    fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("skinned joints layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(size_of::<Mat4>() as u64),
                },
                count: None,
            }],
        });
        let capacity = 256;
        let (buffer, bind_group) = Self::create(device, &layout, capacity);
        Self { layout, buffer, bind_group, capacity }
    }

    fn create(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, capacity: usize) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("skinned joint matrices"),
            size: (capacity * size_of::<Mat4>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("skinned joint matrices"),
            layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
        });
        (buffer, bind_group)
    }

    fn upload(&mut self, gpu: &crate::Gpu, matrices: &[Mat4]) {
        if matrices.is_empty() {
            return;
        }
        if matrices.len() > self.capacity {
            // The bind group points at the old buffer, so it is rebuilt too.
            self.capacity = matrices.len().next_power_of_two();
            (self.buffer, self.bind_group) = Self::create(&gpu.device, &self.layout, self.capacity);
        }
        gpu.queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(matrices));
    }
}

pub(crate) struct SkinnedMeshRenderer {
    ctx: RenderContext,
    pipeline_layout: wgpu::PipelineLayout,
    shaders: HashMap<SkinnedShaderId, ShaderEntry>,
    meshes: HashMap<SkinnedMeshId, MeshEntry>,
    skeletons: HashMap<SkeletonId, Skeleton>,
    instances: InstanceBatches<SkinnedShaderId, SkinnedMeshId, InstanceRaw>,
    joints: JointBuffer,

    // Per-frame scratch space, kept to reuse allocations.
    prev_by_key: HashMap<u64, usize>,
    joint_matrices: Vec<Mat4>,
    pose: Vec<Transform>,
    model_space: Vec<Mat4>,
}

impl SkinnedMeshRenderer {
    pub(crate) fn new(ctx: &RenderContext) -> Self {
        let device = &ctx.gpu.device;
        let joints = JointBuffer::new(device);
        let instances = InstanceBatches::new(device, "skinned mesh instances");
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("skinned mesh pipeline layout"),
            bind_group_layouts: &[
                Some(&ctx.globals_layout),
                Some(&ctx.material_layout),
                Some(&joints.layout),
                Some(instances.params_layout()),
            ],
            immediate_size: 0,
        });
        Self {
            ctx: ctx.clone(),
            pipeline_layout,
            shaders: HashMap::new(),
            meshes: HashMap::new(),
            skeletons: HashMap::new(),
            instances,
            joints,
            prev_by_key: HashMap::new(),
            joint_matrices: Vec::new(),
            pose: Vec::new(),
            model_space: Vec::new(),
        }
    }

    // ---- shaders ----

    pub(crate) fn material_layout(&self, id: SkinnedShaderId) -> Option<Arc<ParamLayout>> {
        self.shaders.get(&id).map(|s| s.material_layout.clone())
    }

    pub(crate) fn instance_layout(&self, id: SkinnedShaderId) -> Option<&ParamLayout> {
        self.shaders.get(&id).map(|s| &s.instance_layout)
    }

    pub(crate) fn create_shader(&mut self, desc: &ShaderDesc<'_>) -> Result<SkinnedShaderId, RenderError> {
        let entry = self.build_shader(desc)?;
        let id = SkinnedShaderId::next();
        self.shaders.insert(id, entry);
        Ok(id)
    }

    /// Recompile in place and return the (possibly changed) `struct
    /// Material` layout. On error the old version stays.
    pub(crate) fn update_shader(
        &mut self,
        id: SkinnedShaderId,
        desc: &ShaderDesc<'_>,
    ) -> Result<Arc<ParamLayout>, RenderError> {
        if !self.shaders.contains_key(&id) {
            return Err(RenderError::UnknownSkinnedShader(id));
        }
        let entry = self.build_shader(desc)?;
        let layout = entry.material_layout.clone();
        self.shaders.insert(id, entry);
        Ok(layout)
    }

    pub(crate) fn destroy_shader(&mut self, id: SkinnedShaderId) {
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

    // ---- meshes and skeletons ----

    pub(crate) fn create_mesh(&mut self, data: &SkinnedMeshData) -> SkinnedMeshId {
        let device = &self.ctx.gpu.device;
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("skinned mesh vertices"),
            contents: bytemuck::cast_slice(&data.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("skinned mesh indices"),
            contents: bytemuck::cast_slice(&data.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let id = SkinnedMeshId::next();
        self.meshes.insert(
            id,
            MeshEntry { vertex_buffer, index_buffer, index_count: data.indices.len() as u32 },
        );
        id
    }

    pub(crate) fn destroy_mesh(&mut self, id: SkinnedMeshId) {
        self.meshes.remove(&id);
    }

    pub(crate) fn create_skeleton(&mut self, desc: &SkeletonDesc) -> Result<SkeletonId, RenderError> {
        let skeleton = Skeleton::new(desc)?;
        let id = SkeletonId::next();
        self.skeletons.insert(id, skeleton);
        Ok(id)
    }

    pub(crate) fn destroy_skeleton(&mut self, id: SkeletonId) {
        self.skeletons.remove(&id);
    }
}

impl SubRenderer for SkinnedMeshRenderer {
    fn prepare(&mut self, frame: &FrameData<'_>, _encoder: &mut wgpu::CommandEncoder) {
        self.prev_by_key.clear();
        for (index, draw) in frame.prev.skinned.iter().enumerate() {
            if let Some(key) = draw.key {
                self.prev_by_key.insert(key, index);
            }
        }

        self.instances.clear();
        self.joint_matrices.clear();

        for curr in &frame.curr.skinned {
            // Missing resources (or a material for another subrenderer)
            // just don't draw.
            let Some(material) = frame.materials.get(curr.material) else { continue };
            let MaterialShader::Skinned(shader_id) = material.shader else { continue };
            let Some(shader) = self.shaders.get(&shader_id) else { continue };
            let Some(skeleton) = self.skeletons.get(&curr.skeleton) else { continue };
            if !self.meshes.contains_key(&curr.mesh) {
                continue;
            }

            let prev = curr
                .key
                .and_then(|k| self.prev_by_key.get(&k))
                .map(|&i| &frame.prev.skinned[i]);

            // Interpolate the pose joint by joint when both ticks posed the
            // same skeleton the same way; otherwise use this tick's pose.
            let curr_pose = frame.curr.pose_of(curr);
            let pose: &[Transform] = match prev {
                Some(prev) if prev.skeleton == curr.skeleton && prev.pose.len == curr.pose.len => {
                    let prev_pose = frame.prev.pose_of(prev);
                    self.pose.clear();
                    self.pose
                        .extend(prev_pose.iter().zip(curr_pose).map(|(p, c)| p.lerp(c, frame.alpha)));
                    &self.pose
                }
                _ => curr_pose,
            };

            let joint_offset = self.joint_matrices.len() as u32;
            skeleton.skinning_matrices(pose, &mut self.model_space, &mut self.joint_matrices);
            debug_assert_eq!(self.joint_matrices.len(), joint_offset as usize + skeleton.joint_count());

            let (transform, color) = match prev {
                Some(prev) => (
                    prev.transform.lerp(&curr.transform, frame.alpha),
                    prev.color.lerp(curr.color, frame.alpha),
                ),
                None => (curr.transform, curr.color),
            };
            let prev_values = prev.map_or(&[][..], |prev| frame.prev.instance_values(prev.params));
            let curr_values = frame.curr.instance_values(curr.params);

            let key = BatchKey {
                blended: shader.blend != BlendMode::Opaque,
                shader: shader_id,
                material: curr.material,
                mesh: curr.mesh,
            };
            let instance = InstanceRaw {
                model: transform.to_matrix().to_cols_array_2d(),
                color: color.to_array(),
                joint_offset,
                param_slot: 0, // assigned in upload
            };
            self.instances.push(key, instance, |out| {
                shader.instance_layout.pack(prev_values, curr_values, frame.alpha, out);
            });
        }

        self.instances.upload(&self.ctx.gpu);
        self.joints.upload(&self.ctx.gpu, &self.joint_matrices);
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, data: &DrawData<'_>) {
        let batches = self.instances.batches();
        if batches.is_empty() {
            return;
        }
        pass.set_bind_group(0, data.globals, &[]);
        pass.set_bind_group(JOINTS_GROUP, &self.joints.bind_group, &[]);
        pass.set_vertex_buffer(1, self.instances.instance_buffer().slice(..));

        let mut current: Option<BatchKey<SkinnedShaderId, SkinnedMeshId>> = None;
        for batch in batches {
            let key = batch.key;
            // Lookups can't fail: prepare filtered out missing resources.
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
