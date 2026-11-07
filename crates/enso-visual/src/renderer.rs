//! [`WgpuRenderer`]: the wgpu implementation of [`Renderer`](crate::Renderer).

use std::collections::HashMap;

use bytemuck::Zeroable as _;
use wgpu::util::DeviceExt;

use crate::{
    Gpu, MaterialId, MeshId, MeshShaderId, ParamInfo, RenderError, RenderWorld, Renderer, ShaderDesc, SkeletonId,
    SkinnedMeshId, SkinnedShaderId, SkyShaderId, TextureId,
    material::{MaterialDesc, MaterialShader, MaterialStore},
    skinned_mesh::{SkeletonDesc, SkinnedMeshData, SkinnedMeshRenderer},
    sky::{SkyRenderer, SkyShaderDesc},
    static_mesh::{MeshData, StaticMeshRenderer},
    subrenderer::{DrawData, FrameData, RenderContext, SubRenderer},
    texture::{Texture, TextureDesc},
};

/// Where a [`WgpuRenderer`] draws: usually the window's current surface
/// texture, or an [`OffscreenTarget`](crate::OffscreenTarget).
#[derive(Debug, Clone, Copy)]
pub struct RenderTarget<'a> {
    pub view: &'a wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

/// CPU mirror of `Globals` in [`crate::GLOBALS_WGSL`].
/// `#[repr(C)]` + `Pod` lets bytemuck hand it to the GPU as raw bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GlobalsUniform {
    view_proj: [[f32; 4]; 4],
    inv_view_proj: [[f32; 4]; 4],
    camera_position: [f32; 4],
    resolution: [f32; 2],
    time: f32,
    delta_time: f32,
}

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// The wgpu implementation of [`Renderer`].
///
/// Internally the work is split across subrenderers, one per kind of
/// drawable (sky, static meshes, skinned meshes); they are not exposed.
///
/// The trait's methods need it in scope: `use enso_visual::Renderer;`.
pub struct WgpuRenderer {
    ctx: RenderContext,
    globals_buffer: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    depth: Option<DepthTarget>,
    last_time: Option<f64>,

    textures: HashMap<TextureId, Texture>,
    materials: MaterialStore,

    // Subrenderers, in draw order.
    sky: SkyRenderer,
    static_mesh: StaticMeshRenderer,
    skinned_mesh: SkinnedMeshRenderer,
}

struct DepthTarget {
    size: (u32, u32),
    view: wgpu::TextureView,
}

/// What only the wgpu implementation has: construction and GPU access.
impl WgpuRenderer {
    /// `color_format` must match the surface (or texture) you will render to.
    pub fn new(gpu: Gpu, color_format: wgpu::TextureFormat) -> Self {
        let device = &gpu.device;

        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("enso globals layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(size_of::<GlobalsUniform>() as u64),
                },
                count: None,
            }],
        });

        let globals_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("enso globals"),
            contents: bytemuck::bytes_of(&GlobalsUniform::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("enso globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buffer.as_entire_binding(),
            }],
        });

        let materials = MaterialStore::new(&gpu);
        let ctx = RenderContext {
            material_layout: materials.layout().clone(),
            gpu,
            color_format,
            depth_format: DEPTH_FORMAT,
            globals_layout,
        };

        Self {
            sky: SkyRenderer::new(&ctx),
            static_mesh: StaticMeshRenderer::new(&ctx),
            skinned_mesh: SkinnedMeshRenderer::new(&ctx),
            ctx,
            globals_buffer,
            globals_bind_group,
            depth: None,
            last_time: None,
            textures: HashMap::new(),
            materials,
        }
    }

    pub fn gpu(&self) -> &Gpu {
        &self.ctx.gpu
    }

    pub fn color_format(&self) -> wgpu::TextureFormat {
        self.ctx.color_format
    }

    /// The shared depth buffer, (re)created whenever the target size changes.
    fn depth_view(&mut self, width: u32, height: u32) -> &wgpu::TextureView {
        let size = (width.max(1), height.max(1));
        if self.depth.as_ref().is_none_or(|d| d.size != size) {
            let texture = self.ctx.gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("enso depth"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            self.depth = Some(DepthTarget { size, view });
        }
        &self.depth.as_ref().unwrap().view
    }
}

/// The interface. Documentation lives on the trait (`lib.rs`).
impl Renderer for WgpuRenderer {
    type Target<'a> = RenderTarget<'a>;

    // ---- textures ----

    fn create_texture(&mut self, desc: &TextureDesc<'_>) -> Result<TextureId, RenderError> {
        let texture = Texture::new(&self.ctx.gpu.device, &self.ctx.gpu.queue, desc)?;
        let id = TextureId::next();
        self.textures.insert(id, texture);
        Ok(id)
    }

    fn write_texture(&mut self, id: TextureId, data: &[u8]) -> Result<(), RenderError> {
        self.textures
            .get(&id)
            .ok_or(RenderError::UnknownTexture(id))?
            .write(&self.ctx.gpu.queue, data)
    }

    fn destroy_texture(&mut self, id: TextureId) {
        self.textures.remove(&id);
    }

    // ---- static meshes ----

    fn create_mesh(&mut self, data: &MeshData) -> MeshId {
        self.static_mesh.create_mesh(data)
    }

    fn destroy_mesh(&mut self, id: MeshId) {
        self.static_mesh.destroy_mesh(id);
    }

    fn create_mesh_shader(&mut self, desc: &ShaderDesc<'_>) -> Result<MeshShaderId, RenderError> {
        self.static_mesh.create_shader(desc)
    }

    fn update_mesh_shader(&mut self, id: MeshShaderId, desc: &ShaderDesc<'_>) -> Result<(), RenderError> {
        let layout = self.static_mesh.update_shader(id, desc)?;
        self.materials.relayout(MaterialShader::Mesh(id), layout);
        Ok(())
    }

    fn destroy_mesh_shader(&mut self, id: MeshShaderId) {
        self.static_mesh.destroy_shader(id);
    }

    // ---- skinned meshes ----

    fn create_skinned_mesh(&mut self, data: &SkinnedMeshData) -> SkinnedMeshId {
        self.skinned_mesh.create_mesh(data)
    }

    fn destroy_skinned_mesh(&mut self, id: SkinnedMeshId) {
        self.skinned_mesh.destroy_mesh(id);
    }

    fn create_skeleton(&mut self, desc: &SkeletonDesc) -> Result<SkeletonId, RenderError> {
        self.skinned_mesh.create_skeleton(desc)
    }

    fn destroy_skeleton(&mut self, id: SkeletonId) {
        self.skinned_mesh.destroy_skeleton(id);
    }

    fn create_skinned_shader(&mut self, desc: &ShaderDesc<'_>) -> Result<SkinnedShaderId, RenderError> {
        self.skinned_mesh.create_shader(desc)
    }

    fn update_skinned_shader(&mut self, id: SkinnedShaderId, desc: &ShaderDesc<'_>) -> Result<(), RenderError> {
        let layout = self.skinned_mesh.update_shader(id, desc)?;
        self.materials.relayout(MaterialShader::Skinned(id), layout);
        Ok(())
    }

    fn destroy_skinned_shader(&mut self, id: SkinnedShaderId) {
        self.skinned_mesh.destroy_shader(id);
    }

    // ---- materials ----

    fn create_material(&mut self, desc: &MaterialDesc<'_>) -> Result<MaterialId, RenderError> {
        let layout = match desc.shader {
            MaterialShader::Mesh(id) => self.static_mesh.material_layout(id).ok_or(RenderError::UnknownShader(id))?,
            MaterialShader::Skinned(id) => {
                self.skinned_mesh.material_layout(id).ok_or(RenderError::UnknownSkinnedShader(id))?
            }
            MaterialShader::Sky(id) => self.sky.material_layout(id).ok_or(RenderError::UnknownSkyShader(id))?,
        };
        self.materials.create(desc, &self.textures, layout)
    }

    fn destroy_material(&mut self, id: MaterialId) {
        self.materials.destroy(id);
    }

    // ---- named parameters ----

    fn material_params(&self, material: MaterialId) -> Option<&[ParamInfo]> {
        Some(self.materials.get(material)?.layout.infos())
    }

    fn instance_params(&self, material: MaterialId) -> Option<&[ParamInfo]> {
        let layout = match self.materials.get(material)?.shader {
            MaterialShader::Mesh(id) => self.static_mesh.instance_layout(id)?,
            MaterialShader::Skinned(id) => self.skinned_mesh.instance_layout(id)?,
            MaterialShader::Sky(_) => return Some(&[]), // the sky has no instances
        };
        Some(layout.infos())
    }

    // ---- sky ----

    fn create_sky_shader(&mut self, desc: &SkyShaderDesc<'_>) -> Result<SkyShaderId, RenderError> {
        self.sky.create_shader(desc)
    }

    fn update_sky_shader(&mut self, id: SkyShaderId, desc: &SkyShaderDesc<'_>) -> Result<(), RenderError> {
        let layout = self.sky.update_shader(id, desc)?;
        self.materials.relayout(MaterialShader::Sky(id), layout);
        Ok(())
    }

    fn destroy_sky_shader(&mut self, id: SkyShaderId) {
        self.sky.destroy_shader(id);
    }

    // ---- the frame ----

    fn render(&mut self, target: RenderTarget<'_>, prev: &RenderWorld, curr: &RenderWorld, alpha: f32) {
        let alpha = alpha.clamp(0.0, 1.0);
        // Needs `&mut self`, so do it before borrowing fields below.
        let depth_view = self.depth_view(target.width, target.height).clone();

        // Frame-wide values, interpolated.
        let camera = prev.camera.lerp(&curr.camera, alpha);
        let time = prev.time + (curr.time - prev.time) * f64::from(alpha);
        let delta_time = self.last_time.map_or(0.0, |last| (time - last).max(0.0));
        self.last_time = Some(time);
        let clear = prev.clear_color.lerp(curr.clear_color, alpha).as_dvec4();
        let aspect = target.width as f32 / target.height.max(1) as f32;

        let view_proj = camera.view_proj(aspect);
        let globals = GlobalsUniform {
            view_proj: view_proj.to_cols_array_2d(),
            inv_view_proj: view_proj.inverse().to_cols_array_2d(),
            camera_position: camera.eye.extend(1.0).to_array(),
            resolution: [target.width as f32, target.height as f32],
            time: time as f32,
            delta_time: delta_time as f32,
        };
        let gpu = &self.ctx.gpu;
        gpu.queue.write_buffer(&self.globals_buffer, 0, bytemuck::bytes_of(&globals));

        self.materials.update_params(prev, curr, alpha);

        let frame = FrameData { prev, curr, alpha, materials: &self.materials };
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("enso frame") });

        // Add new subrenderers to both arrays, in draw order.
        let subrenderers: [&mut dyn SubRenderer; 3] =
            [&mut self.sky, &mut self.static_mesh, &mut self.skinned_mesh];
        for sub in subrenderers {
            sub.prepare(&frame, &mut encoder);
        }

        // When the sky covers the screen, its pixels replace whatever was
        // there, so clearing first would be wasted work: tell the GPU the old
        // contents don't matter (the fastest load op on every GPU).
        let color_load = if self.sky.covers_target() {
            // SAFETY: the sky draws first, as a fullscreen triangle with
            // REPLACE blending, from a shader without `discard`, so every
            // pixel is written before anything reads, blends with or stores it.
            wgpu::LoadOp::DontCare(unsafe { wgpu::LoadOpDontCare::enabled() })
        } else {
            wgpu::LoadOp::Clear(wgpu::Color { r: clear.x, g: clear.y, b: clear.z, a: clear.w })
        };

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("enso main pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: color_load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            let data = DrawData { globals: &self.globals_bind_group, materials: &self.materials };
            let subrenderers: [&dyn SubRenderer; 3] = [&self.sky, &self.static_mesh, &self.skinned_mesh];
            for sub in subrenderers {
                sub.draw(&mut pass, &data);
            }
        }

        gpu.queue.submit([encoder.finish()]);
    }
}
