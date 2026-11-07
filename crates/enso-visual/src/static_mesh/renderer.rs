//! The static mesh subrenderer (private implementation).

use std::{collections::HashMap, ops::Range};

use wgpu::util::DeviceExt;

use super::{BlendMode, MAX_TEXTURES, MaterialDesc, MeshData, MeshDraw, PRELUDE, ShaderDesc, Vertex};
use crate::{
    MaterialId, MeshId, RenderError, MeshShaderId, TextureId,
    shader,
    subrenderer::{FrameData, RenderContext, SubRenderer, white_texture},
    texture::Texture,
    world::MaterialParams,
};

// ---- GPU-side layouts ------------------------------------------------------

/// Matches `InstanceIn` in `prelude.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct InstanceRaw {
    model: [[f32; 4]; 4],
    color: [f32; 4],
    params: [f32; 4],
}

impl From<&MeshDraw> for InstanceRaw {
    fn from(d: &MeshDraw) -> Self {
        Self {
            model: d.transform.to_matrix().to_cols_array_2d(),
            color: d.color.to_array(),
            params: d.params.to_array(),
        }
    }
}

const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];

const INSTANCE_ATTRIBUTES: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
    3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4, // model matrix columns
    7 => Float32x4, // color
    8 => Float32x4, // params
];

// ---- resources ---------------------------------------------------------------

struct ShaderEntry {
    blend: BlendMode,
    pipeline: wgpu::RenderPipeline,
}

struct MeshEntry {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

struct MaterialEntry {
    shader: MeshShaderId,
    bind_group: wgpu::BindGroup,
    params_buffer: wgpu::Buffer,
    default_params: MaterialParams,
    /// What is currently in `params_buffer`, to skip redundant uploads.
    uploaded_params: MaterialParams,
}

/// Draw order: opaque before blended (blended ones don't write depth), then
/// grouped by shader -> material -> mesh to minimise GPU state changes.
/// `derive(Ord)` compares fields top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SortKey {
    blended: bool,
    shader: MeshShaderId,
    material: MaterialId,
    mesh: MeshId,
}

struct Batch {
    key: SortKey,
    instances: Range<u32>,
}

pub(crate) struct StaticMeshRenderer {
    ctx: RenderContext,
    material_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    sampler: wgpu::Sampler,
    white: Texture,

    shaders: HashMap<MeshShaderId, ShaderEntry>,
    meshes: HashMap<MeshId, MeshEntry>,
    materials: HashMap<MaterialId, MaterialEntry>,

    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,

    // Per-frame scratch space, kept to reuse allocations.
    prev_by_key: HashMap<u64, usize>,
    sorted: Vec<(SortKey, InstanceRaw)>,
    staging: Vec<InstanceRaw>,
    batches: Vec<Batch>,
}

impl StaticMeshRenderer {
    pub(crate) fn new(ctx: &RenderContext) -> Self {
        let device = &ctx.gpu.device;

        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };

        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("static mesh material layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(size_of::<MaterialParams>() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                texture_entry(2),
                texture_entry(3),
                texture_entry(4),
                texture_entry(5),
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("static mesh pipeline layout"),
            bind_group_layouts: &[Some(&ctx.globals_layout), Some(&material_layout)],
            immediate_size: 0,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("static mesh sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let instance_capacity = 256;
        Self {
            ctx: ctx.clone(),
            material_layout,
            pipeline_layout,
            sampler,
            white: white_texture(&ctx.gpu),
            shaders: HashMap::new(),
            meshes: HashMap::new(),
            materials: HashMap::new(),
            instance_buffer: create_instance_buffer(device, instance_capacity),
            instance_capacity,
            prev_by_key: HashMap::new(),
            sorted: Vec::new(),
            staging: Vec::new(),
            batches: Vec::new(),
        }
    }

    // ---- shaders ----

    pub(crate) fn create_shader(&mut self, desc: &ShaderDesc<'_>) -> Result<MeshShaderId, RenderError> {
        let entry = self.build_shader(desc)?;
        let id = MeshShaderId::next();
        self.shaders.insert(id, entry);
        Ok(id)
    }

    /// Recompile in place. Materials using `id` pick up the new version
    /// immediately. On error the old version stays.
    pub(crate) fn update_shader(&mut self, id: MeshShaderId, desc: &ShaderDesc<'_>) -> Result<(), RenderError> {
        if !self.shaders.contains_key(&id) {
            return Err(RenderError::UnknownShader(id));
        }
        let entry = self.build_shader(desc)?;
        self.shaders.insert(id, entry);
        Ok(())
    }

    pub(crate) fn destroy_shader(&mut self, id: MeshShaderId) {
        self.shaders.remove(&id);
    }

    fn build_shader(&self, desc: &ShaderDesc<'_>) -> Result<ShaderEntry, RenderError> {
        let device = &self.ctx.gpu.device;
        let source = shader::compose(PRELUDE, desc.source);

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

        let pipeline = shader::capture_errors(device, desc.label, || {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(desc.label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });

            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(desc.label),
                layout: Some(&self.pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[
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
                    ],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: (!desc.double_sided).then_some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: self.ctx.depth_format,
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
                        format: self.ctx.color_format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        })?;

        Ok(ShaderEntry { blend: desc.blend, pipeline })
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

    // ---- materials ----

    pub(crate) fn create_material(
        &mut self,
        desc: &MaterialDesc<'_>,
        textures: &HashMap<TextureId, Texture>,
    ) -> Result<MaterialId, RenderError> {
        if !self.shaders.contains_key(&desc.shader) {
            return Err(RenderError::UnknownShader(desc.shader));
        }
        if desc.textures.len() > MAX_TEXTURES {
            return Err(RenderError::TooManyTextures { max: MAX_TEXTURES, got: desc.textures.len() });
        }
        let mut views = [self.white.view(); MAX_TEXTURES];
        for (slot, id) in views.iter_mut().zip(desc.textures) {
            *slot = textures.get(id).ok_or(RenderError::UnknownTexture(*id))?.view();
        }

        let device = &self.ctx.gpu.device;
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("static mesh material params"),
            contents: bytemuck::cast_slice(&desc.params),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("static mesh material"),
            layout: &self.material_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: params_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(views[0]) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(views[1]) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(views[2]) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(views[3]) },
            ],
        });

        let id = MaterialId::next();
        self.materials.insert(
            id,
            MaterialEntry {
                shader: desc.shader,
                bind_group,
                params_buffer,
                default_params: desc.params,
                uploaded_params: desc.params,
            },
        );
        Ok(id)
    }

    pub(crate) fn destroy_material(&mut self, id: MaterialId) {
        self.materials.remove(&id);
    }

    // ---- per-frame helpers ----

    /// Interpolate each material's knobs and upload the ones that changed.
    fn update_material_params(&mut self, frame: &FrameData<'_>) {
        for (id, material) in &mut self.materials {
            let target = match (frame.prev.material_params.get(id), frame.curr.material_params.get(id)) {
                (Some(prev), Some(curr)) => {
                    std::array::from_fn(|i| prev[i].lerp(curr[i], frame.alpha))
                }
                (None, Some(curr)) => *curr,
                _ => material.default_params,
            };
            if target != material.uploaded_params {
                self.ctx
                    .gpu
                    .queue
                    .write_buffer(&material.params_buffer, 0, bytemuck::cast_slice(&target));
                material.uploaded_params = target;
            }
        }
    }

    /// Interpolate every draw in `curr` against its keyed match in `prev`,
    /// drop draws referring to missing resources, and sort for batching.
    fn collect_draws(&mut self, frame: &FrameData<'_>) {
        self.prev_by_key.clear();
        for (index, draw) in frame.prev.meshes.iter().enumerate() {
            if let Some(key) = draw.key {
                self.prev_by_key.insert(key, index);
            }
        }

        self.sorted.clear();
        for curr in &frame.curr.meshes {
            let draw = match curr.key.and_then(|k| self.prev_by_key.get(&k)) {
                Some(&i) => MeshDraw::interpolate(&frame.prev.meshes[i], curr, frame.alpha),
                None => *curr,
            };

            // A destroyed (or never created) resource just doesn't draw.
            let Some(material) = self.materials.get(&draw.material) else { continue };
            let Some(shader) = self.shaders.get(&material.shader) else { continue };
            if !self.meshes.contains_key(&draw.mesh) {
                continue;
            }

            let key = SortKey {
                blended: shader.blend != BlendMode::Opaque,
                shader: material.shader,
                material: draw.material,
                mesh: draw.mesh,
            };
            self.sorted.push((key, InstanceRaw::from(&draw)));
        }
        self.sorted.sort_by_key(|(key, _)| *key);
    }
}

impl SubRenderer for StaticMeshRenderer {
    fn prepare(&mut self, frame: &FrameData<'_>, _encoder: &mut wgpu::CommandEncoder) {
        self.update_material_params(frame);
        self.collect_draws(frame);

        // Consecutive draws with the same key become one instanced batch.
        self.staging.clear();
        self.batches.clear();
        for (key, instance) in &self.sorted {
            let index = self.staging.len() as u32;
            self.staging.push(*instance);
            match self.batches.last_mut() {
                Some(batch) if batch.key == *key => batch.instances.end = index + 1,
                _ => self.batches.push(Batch { key: *key, instances: index..index + 1 }),
            }
        }

        if self.staging.is_empty() {
            return;
        }
        if self.staging.len() > self.instance_capacity {
            self.instance_capacity = self.staging.len().next_power_of_two();
            self.instance_buffer = create_instance_buffer(&self.ctx.gpu.device, self.instance_capacity);
        }
        self.ctx
            .gpu
            .queue
            .write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&self.staging));
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, globals: &wgpu::BindGroup) {
        if self.batches.is_empty() {
            return;
        }
        pass.set_bind_group(0, globals, &[]);
        pass.set_vertex_buffer(1, self.instance_buffer.slice(..));

        let mut current: Option<SortKey> = None;
        for batch in &self.batches {
            let key = batch.key;
            // Lookups can't fail: collect_draws filtered out missing resources
            // and nothing can be destroyed between prepare and draw.
            let mesh = &self.meshes[&key.mesh];

            if current.is_none_or(|c| c.shader != key.shader) {
                pass.set_pipeline(&self.shaders[&key.shader].pipeline);
            }
            if current.is_none_or(|c| c.material != key.material) {
                pass.set_bind_group(1, &self.materials[&key.material].bind_group, &[]);
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

fn create_instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("static mesh instances"),
        size: (capacity * size_of::<InstanceRaw>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
