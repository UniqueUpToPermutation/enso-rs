//! The text subrenderer (private implementation).

use std::{collections::HashMap, sync::Arc};

use wgpu::util::DeviceExt;

use super::{FontAtlas, FontDesc, PRELUDE, TextDraw, font::PlacedGlyph};
use crate::{
    BlendMode, FontId, RenderError, ShaderDesc, TextShaderId,
    instancing::{BatchKey, InstanceBatches, InstanceData},
    material::{MATERIAL_WGSL, MaterialShader},
    params::ParamLayout,
    shader,
    subrenderer::{DrawData, FrameData, RenderContext, SubRenderer},
    texture::{Texture, TextureDesc},
};

const FONT_GROUP: u32 = 2;
const INSTANCE_PARAMS_GROUP: u32 = 3;

/// One glyph. Matches `GlyphIn` in `prelude.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GlyphRaw {
    model: [[f32; 4]; 4],
    color: [f32; 4],
    rect: [f32; 4],
    uv: [f32; 4],
    index: u32,
    param_slot: u32,
}

impl InstanceData for GlyphRaw {
    fn set_param_slot(&mut self, slot: u32) {
        self.param_slot = slot;
    }
}

const GLYPH_ATTRIBUTES: [wgpu::VertexAttribute; 9] = wgpu::vertex_attr_array![
    0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, // model matrix columns
    4 => Float32x4, // color
    5 => Float32x4, // rect
    6 => Float32x4, // uv_rect
    7 => Uint32,    // index
    8 => Uint32,    // param_slot
];

fn vertex_buffers() -> [Option<wgpu::VertexBufferLayout<'static>>; 1] {
    [Some(wgpu::VertexBufferLayout {
        array_stride: size_of::<GlyphRaw>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &GLYPH_ATTRIBUTES,
    })]
}

struct ShaderEntry {
    blend: BlendMode,
    pipeline: wgpu::RenderPipeline,
    material_layout: Arc<ParamLayout>,
    instance_layout: ParamLayout,
}

struct FontEntry {
    atlas: FontAtlas,
    bind_group: wgpu::BindGroup,
    // Kept alive by the bind group too; held for clarity.
    _texture: Texture,
}

pub(crate) struct TextRenderer {
    ctx: RenderContext,
    pipeline_layout: wgpu::PipelineLayout,
    font_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    shaders: HashMap<TextShaderId, ShaderEntry>,
    fonts: HashMap<FontId, FontEntry>,
    instances: InstanceBatches<TextShaderId, FontId, GlyphRaw>,

    // Per-frame scratch space.
    prev_by_key: HashMap<u64, usize>,
    glyphs: Vec<PlacedGlyph>,
}

impl TextRenderer {
    pub(crate) fn new(ctx: &RenderContext) -> Self {
        let device = &ctx.gpu.device;
        let font_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("font layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let instances = InstanceBatches::new(device, "text glyphs");
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("text pipeline layout"),
            bind_group_layouts: &[
                Some(&ctx.globals_layout),
                Some(&ctx.material_layout),
                Some(&font_layout),
                Some(instances.params_layout()),
            ],
            immediate_size: 0,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("font sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default() // clamp to edge
        });
        Self {
            ctx: ctx.clone(),
            pipeline_layout,
            font_layout,
            sampler,
            shaders: HashMap::new(),
            fonts: HashMap::new(),
            instances,
            prev_by_key: HashMap::new(),
            glyphs: Vec::new(),
        }
    }

    // ---- fonts ----

    pub(crate) fn create_font(&mut self, desc: &FontDesc<'_>) -> Result<FontId, RenderError> {
        let atlas = FontAtlas::build(desc)?;
        let gpu = &self.ctx.gpu;
        let texture = Texture::new(
            &gpu.device,
            &gpu.queue,
            &TextureDesc {
                label: Some(desc.label),
                width: atlas.width,
                height: atlas.height,
                // Distances, not colours: no sRGB conversion.
                format: wgpu::TextureFormat::Rgba8Unorm,
                data: Some(&atlas.pixels),
            },
        )?;
        let info = gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("font info"),
            contents: bytemuck::cast_slice(&[atlas.px_range, 0.0, 0.0, 0.0]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(desc.label),
            layout: &self.font_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(texture.view()) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: info.as_entire_binding() },
            ],
        });
        let id = FontId::next();
        self.fonts.insert(id, FontEntry { atlas, bind_group, _texture: texture });
        Ok(id)
    }

    pub(crate) fn destroy_font(&mut self, id: FontId) {
        self.fonts.remove(&id);
    }

    // ---- shaders ----

    pub(crate) fn material_layout(&self, id: TextShaderId) -> Option<Arc<ParamLayout>> {
        self.shaders.get(&id).map(|s| s.material_layout.clone())
    }

    pub(crate) fn instance_layout(&self, id: TextShaderId) -> Option<&ParamLayout> {
        self.shaders.get(&id).map(|s| &s.instance_layout)
    }

    pub(crate) fn create_shader(&mut self, desc: &ShaderDesc<'_>) -> Result<TextShaderId, RenderError> {
        let entry = self.build_shader(desc)?;
        let id = TextShaderId::next();
        self.shaders.insert(id, entry);
        Ok(id)
    }

    pub(crate) fn update_shader(
        &mut self,
        id: TextShaderId,
        desc: &ShaderDesc<'_>,
    ) -> Result<Arc<ParamLayout>, RenderError> {
        if !self.shaders.contains_key(&id) {
            return Err(RenderError::UnknownTextShader(id));
        }
        let entry = self.build_shader(desc)?;
        let layout = entry.material_layout.clone();
        self.shaders.insert(id, entry);
        Ok(layout)
    }

    pub(crate) fn destroy_shader(&mut self, id: TextShaderId) {
        self.shaders.remove(&id);
    }

    fn build_shader(&self, desc: &ShaderDesc<'_>) -> Result<ShaderEntry, RenderError> {
        // Glyph quads must be visible from behind too (text spinning in 3D).
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

impl SubRenderer for TextRenderer {
    fn prepare(&mut self, frame: &FrameData<'_>, _encoder: &mut wgpu::CommandEncoder) {
        self.prev_by_key.clear();
        for (index, draw) in frame.prev.texts.iter().enumerate() {
            if let Some(key) = draw.key {
                self.prev_by_key.insert(key, index);
            }
        }

        self.instances.clear();
        for curr in &frame.curr.texts {
            // Missing resources (or a material for another subrenderer)
            // just don't draw.
            let Some(material) = frame.materials.get(curr.material) else { continue };
            let MaterialShader::Text(shader_id) = material.shader else { continue };
            let Some(shader) = self.shaders.get(&shader_id) else { continue };
            let Some(font) = self.fonts.get(&curr.font) else { continue };

            let prev: Option<&TextDraw> = curr.key.and_then(|k| self.prev_by_key.get(&k)).map(|&i| &frame.prev.texts[i]);
            let (transform, color, size) = match prev {
                Some(prev) => (
                    prev.transform.lerp(&curr.transform, frame.alpha),
                    prev.color.lerp(curr.color, frame.alpha),
                    prev.size + (curr.size - prev.size) * frame.alpha,
                ),
                None => (curr.transform, curr.color, curr.size),
            };
            let prev_values = prev.map_or(&[][..], |prev| frame.prev.instance_values(prev.params));
            let curr_values = frame.curr.instance_values(curr.params);

            let key = BatchKey {
                blended: shader.blend != BlendMode::Opaque,
                shader: shader_id,
                material: curr.material,
                mesh: curr.font,
            };
            let model = transform.to_matrix().to_cols_array_2d();

            // The string is laid out fresh each frame (cheap for short text).
            font.atlas.layout(frame.curr.text_of(curr), curr.align, &mut self.glyphs);
            for glyph in &self.glyphs {
                let instance = GlyphRaw {
                    model,
                    color: color.to_array(),
                    rect: glyph.rect.map(|v| v * size),
                    uv: glyph.uv,
                    index: glyph.index,
                    param_slot: 0, // assigned in upload
                };
                // Every glyph of this draw gets the same Instance values.
                self.instances.push(key, instance, |out| {
                    shader.instance_layout.pack(prev_values, curr_values, frame.alpha, out);
                });
            }
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

        let mut current: Option<BatchKey<TextShaderId, FontId>> = None;
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
            if current.is_none_or(|c| c.mesh != key.mesh) {
                pass.set_bind_group(FONT_GROUP, &self.fonts[&key.mesh].bind_group, &[]);
            }
            current = Some(key);

            pass.draw(0..6, batch.instances.clone()); // one quad per glyph
        }
    }
}
