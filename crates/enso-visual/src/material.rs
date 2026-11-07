//! Materials: a shader plus its inputs (its `struct Material` values and up
//! to four textures), shared by the static mesh, skinned mesh and sky
//! subrenderers.
//!
//! All materials use the same bind group layout (`@group(1)`, declared in
//! `material.wgsl`), so one store serves every subrenderer. The *size* of
//! each material's uniform buffer follows its shader's `struct Material`,
//! whose layout the subrenderer reflects when compiling the shader.

use std::{collections::HashMap, sync::Arc};

use crate::{
    Gpu, MaterialId, MeshShaderId, RenderError, RenderWorld, SkinnedShaderId, SkyShaderId, TextureId,
    params::{Param, ParamLayout, ParamName, ParamType, ParamValue},
    texture::{Texture, TextureDesc},
};

/// The material WGSL contract, included in every mesh-like shader.
pub(crate) const MATERIAL_WGSL: &str = include_str!("material.wgsl");

/// Number of texture slots in a material (`texture0`..`texture3`).
pub const MAX_TEXTURES: usize = 4;

/// The shader a material uses. Its kind decides where the material can be
/// used: `Mesh` for [`MeshDraw`](crate::static_mesh::MeshDraw), `Skinned`
/// for [`SkinnedDraw`](crate::skinned_mesh::SkinnedDraw), `Sky` for
/// [`RenderWorld::sky`](crate::RenderWorld::sky).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MaterialShader {
    Mesh(MeshShaderId),
    Skinned(SkinnedShaderId),
    Sky(SkyShaderId),
}

impl From<SkyShaderId> for MaterialShader {
    fn from(id: SkyShaderId) -> Self {
        Self::Sky(id)
    }
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
    /// Values for `struct Material` fields when a tick doesn't set them.
    pub(crate) defaults: Vec<(String, ParamValue)>,
}

impl<'a> MaterialDesc<'a> {
    /// `shader` is a [`MeshShaderId`], [`SkinnedShaderId`] or [`SkyShaderId`].
    pub fn new(shader: impl Into<MaterialShader>) -> Self {
        Self { shader: shader.into(), textures: &[], defaults: Vec::new() }
    }

    pub fn with_textures(mut self, textures: &'a [TextureId]) -> Self {
        self.textures = textures;
        self
    }

    /// Default value of the shader's `struct Material` field `name`
    /// (otherwise 0). Checked when the material is created.
    pub fn with_param<T: ParamType>(mut self, name: &str, value: T) -> Self {
        let (_, value) = Param::<T>::new(name).value(value);
        self.defaults.push((name.to_owned(), value));
        self
    }
}

// ---- the store (internal) ---------------------------------------------------

pub(crate) struct MaterialEntry {
    pub shader: MaterialShader,
    pub bind_group: wgpu::BindGroup,
    pub layout: Arc<ParamLayout>,
    defaults: Vec<(ParamName, ParamValue)>,
    default_bytes: Vec<u8>,
    /// This frame's packed values (scratch).
    bytes: Vec<u8>,
    /// What is currently in `buffer`, to skip redundant uploads.
    uploaded: Vec<u8>,
    buffer: wgpu::Buffer,
    views: [wgpu::TextureView; MAX_TEXTURES],
}

pub(crate) struct MaterialStore {
    gpu: Gpu,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    white: Texture,
    entries: HashMap<MaterialId, MaterialEntry>,
}

/// Uniform buffers are sized in multiples of 16 bytes.
fn uniform_size(layout: &ParamLayout) -> usize {
    (layout.size as usize).next_multiple_of(16).max(16)
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
                        min_binding_size: None, // follows each shader's struct
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

    /// The caller checks that `desc.shader` exists and passes its
    /// `struct Material` layout.
    pub(crate) fn create(
        &mut self,
        desc: &MaterialDesc<'_>,
        textures: &HashMap<TextureId, Texture>,
        layout: Arc<ParamLayout>,
    ) -> Result<MaterialId, RenderError> {
        if desc.textures.len() > MAX_TEXTURES {
            return Err(RenderError::TooManyTextures { max: MAX_TEXTURES, got: desc.textures.len() });
        }
        let mut views: [wgpu::TextureView; MAX_TEXTURES] = std::array::from_fn(|_| self.white.view().clone());
        for (slot, id) in views.iter_mut().zip(desc.textures) {
            *slot = textures.get(id).ok_or(RenderError::UnknownTexture(*id))?.view().clone();
        }

        let mut defaults = Vec::with_capacity(desc.defaults.len());
        for (name, value) in &desc.defaults {
            let param = ParamName::intern(name);
            match layout.kind_of(param) {
                None => {
                    let available: Vec<&str> = layout.infos().iter().map(|i| i.name.as_str()).collect();
                    return Err(RenderError::UnknownParam { name: name.clone(), available: available.join(", ") });
                }
                Some(found) if found != value.kind => {
                    return Err(RenderError::ParamKindMismatch { name: name.clone(), expected: value.kind, found });
                }
                Some(_) => defaults.push((param, *value)),
            }
        }

        let (buffer, bind_group) = self.create_binding(&layout, &views);
        let mut entry = MaterialEntry {
            shader: desc.shader,
            bind_group,
            layout,
            defaults,
            default_bytes: Vec::new(),
            bytes: Vec::new(),
            uploaded: Vec::new(), // forces an upload on the first frame
            buffer,
            views,
        };
        entry.default_bytes = default_bytes(&entry.layout, &entry.defaults);

        let id = MaterialId::next();
        self.entries.insert(id, entry);
        Ok(id)
    }

    fn create_binding(&self, layout: &ParamLayout, views: &[wgpu::TextureView; MAX_TEXTURES]) -> (wgpu::Buffer, wgpu::BindGroup) {
        let device = &self.gpu.device;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("material params"),
            size: uniform_size(layout) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("material"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&views[0]) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&views[1]) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&views[2]) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&views[3]) },
            ],
        });
        (buffer, bind_group)
    }

    pub(crate) fn destroy(&mut self, id: MaterialId) {
        self.entries.remove(&id);
    }

    /// A shader was recompiled and its `struct Material` may have changed:
    /// re-lay out every material that uses it. Defaults are kept by name;
    /// ones whose field disappeared are dropped silently.
    pub(crate) fn relayout(&mut self, shader: MaterialShader, layout: Arc<ParamLayout>) {
        let ids: Vec<MaterialId> = self.entries.iter().filter(|(_, e)| e.shader == shader).map(|(id, _)| *id).collect();
        for id in ids {
            let size_changed = uniform_size(&self.entries[&id].layout) != uniform_size(&layout);
            if size_changed {
                let (buffer, bind_group) = self.create_binding(&layout, &self.entries[&id].views);
                let entry = self.entries.get_mut(&id).unwrap();
                entry.buffer = buffer;
                entry.bind_group = bind_group;
            }
            let entry = self.entries.get_mut(&id).unwrap();
            entry.layout = layout.clone();
            entry.default_bytes = default_bytes(&entry.layout, &entry.defaults);
            entry.uploaded.clear(); // force an upload
        }
    }

    /// Pack each material's `struct Material` for this frame: defaults,
    /// overwritten by values set in `curr` (interpolated with `prev` where
    /// both set the same field). Upload the ones that changed.
    pub(crate) fn update_params(&mut self, prev: &RenderWorld, curr: &RenderWorld, alpha: f32) {
        for entry in self.entries.values_mut() {
            entry.bytes.clone_from(&entry.default_bytes);
        }
        for (&(material, name), value) in &curr.material_values {
            let Some(entry) = self.entries.get_mut(&material) else { continue };
            let value = match prev.material_values.get(&(material, name)) {
                Some(prev) if prev.kind == value.kind => prev.lerp(value, alpha),
                _ => *value,
            };
            entry.layout.write(&mut entry.bytes, name, &value);
        }
        for entry in self.entries.values_mut() {
            if entry.bytes != entry.uploaded {
                self.gpu.queue.write_buffer(&entry.buffer, 0, &entry.bytes);
                entry.uploaded.clone_from(&entry.bytes);
            }
        }
    }
}

fn default_bytes(layout: &ParamLayout, defaults: &[(ParamName, ParamValue)]) -> Vec<u8> {
    let mut bytes = vec![0; uniform_size(layout)];
    for (name, value) in defaults {
        layout.write(&mut bytes, *name, value);
    }
    bytes
}
