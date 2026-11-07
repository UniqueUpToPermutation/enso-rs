//! Materials: a shader plus its inputs (four `vec4` knobs and up to four
//! textures), shared by the mesh-like subrenderers.
//!
//! All materials use the same bind group layout (`@group(1)`, declared in
//! `material.wgsl`), so one store serves every subrenderer. Which
//! subrenderer can use a material follows from its shader's kind.

use std::collections::HashMap;

use glam::Vec4;
use wgpu::util::DeviceExt;

use crate::{
    Gpu, MaterialId, MeshShaderId, RenderError, RenderWorld, SkinnedShaderId, TextureId,
    texture::{Texture, TextureDesc},
    world::MaterialParams,
};

/// The material WGSL contract, included in every mesh-like shader.
pub(crate) const MATERIAL_WGSL: &str = include_str!("material.wgsl");

/// Number of texture slots in a material (`texture0`..`texture3`).
pub const MAX_TEXTURES: usize = 4;

/// The shader a material uses. Its kind decides which draws can use the
/// material: `Mesh` for [`MeshDraw`](crate::static_mesh::MeshDraw),
/// `Skinned` for [`SkinnedDraw`](crate::skinned_mesh::SkinnedDraw).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MaterialShader {
    Mesh(MeshShaderId),
    Skinned(SkinnedShaderId),
}

impl From<MeshShaderId> for MaterialShader {
    fn from(id: MeshShaderId) -> Self {
        Self::Mesh(id)
    }
}

impl From<SkinnedShaderId> for MaterialShader {
    fn from(id: SkinnedShaderId) -> Self {
        Self::Skinned(id)
    }
}

/// A shader plus its inputs.
#[derive(Debug, Clone)]
pub struct MaterialDesc<'a> {
    pub shader: MaterialShader,
    /// Bound to `texture0`.. in order; missing slots are 1x1 white.
    pub textures: &'a [TextureId],
    /// Used whenever a `RenderWorld` does not set this material's params.
    pub params: MaterialParams,
}

impl<'a> MaterialDesc<'a> {
    /// `shader` is a [`MeshShaderId`] or a [`SkinnedShaderId`].
    pub fn new(shader: impl Into<MaterialShader>) -> Self {
        Self { shader: shader.into(), textures: &[], params: [Vec4::ZERO; 4] }
    }
    pub fn with_textures(mut self, textures: &'a [TextureId]) -> Self {
        self.textures = textures;
        self
    }
    pub fn with_params(mut self, params: MaterialParams) -> Self {
        self.params = params;
        self
    }
}

// ---- the store (internal) ---------------------------------------------------

pub(crate) struct MaterialEntry {
    pub shader: MaterialShader,
    pub bind_group: wgpu::BindGroup,
    params_buffer: wgpu::Buffer,
    default_params: MaterialParams,
    /// What is currently in `params_buffer`, to skip redundant uploads.
    uploaded_params: MaterialParams,
}

pub(crate) struct MaterialStore {
    gpu: Gpu,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    white: Texture,
    entries: HashMap<MaterialId, MaterialEntry>,
}

impl MaterialStore {
    pub(crate) fn new(gpu: &Gpu) -> Self {
        let device = &gpu.device;

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

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material layout"),
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

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("material sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let white = Texture::new(
            device,
            &gpu.queue,
            &TextureDesc::rgba8(1, 1, Some(&[255, 255, 255, 255])).with_label("white"),
        )
        .expect("1x1 RGBA data is the right size");

        Self { gpu: gpu.clone(), layout, sampler, white, entries: HashMap::new() }
    }

    pub(crate) fn layout(&self) -> &wgpu::BindGroupLayout {
        &self.layout
    }

    pub(crate) fn get(&self, id: MaterialId) -> Option<&MaterialEntry> {
        self.entries.get(&id)
    }

    /// The caller checks that `desc.shader` exists.
    pub(crate) fn create(
        &mut self,
        desc: &MaterialDesc<'_>,
        textures: &HashMap<TextureId, Texture>,
    ) -> Result<MaterialId, RenderError> {
        if desc.textures.len() > MAX_TEXTURES {
            return Err(RenderError::TooManyTextures { max: MAX_TEXTURES, got: desc.textures.len() });
        }
        let mut views = [self.white.view(); MAX_TEXTURES];
        for (slot, id) in views.iter_mut().zip(desc.textures) {
            *slot = textures.get(id).ok_or(RenderError::UnknownTexture(*id))?.view();
        }

        let device = &self.gpu.device;
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("material params"),
            contents: bytemuck::cast_slice(&desc.params),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("material"),
            layout: &self.layout,
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
        self.entries.insert(
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

    pub(crate) fn destroy(&mut self, id: MaterialId) {
        self.entries.remove(&id);
    }

    /// Interpolate each material's knobs between two ticks and upload the
    /// ones that changed. Both ticks set -> lerp; only `curr` -> `curr`;
    /// neither -> the defaults from creation.
    pub(crate) fn update_params(&mut self, prev: &RenderWorld, curr: &RenderWorld, alpha: f32) {
        for (id, material) in &mut self.entries {
            let target = match (prev.material_params.get(id), curr.material_params.get(id)) {
                (Some(p), Some(c)) => std::array::from_fn(|i| p[i].lerp(c[i], alpha)),
                (None, Some(c)) => *c,
                _ => material.default_params,
            };
            if target != material.uploaded_params {
                self.gpu
                    .queue
                    .write_buffer(&material.params_buffer, 0, bytemuck::cast_slice(&target));
                material.uploaded_params = target;
            }
        }
    }
}
