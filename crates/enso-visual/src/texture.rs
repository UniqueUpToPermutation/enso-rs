//! Textures: shared by every subrenderer, referred to by [`TextureId`](crate::TextureId).

use crate::RenderError;

/// Description of a texture to create.
#[derive(Debug, Clone)]
pub struct TextureDesc<'a> {
    pub label: Option<&'a str>,
    pub width: u32,
    pub height: u32,
    pub format: wgpu::TextureFormat,
    /// Initial pixel data, tightly packed rows. `None` leaves it zeroed.
    pub data: Option<&'a [u8]>,
}

impl<'a> TextureDesc<'a> {
    /// An 8-bit sRGB RGBA texture: the usual choice for images and colours.
    pub fn rgba8(width: u32, height: u32, data: Option<&'a [u8]>) -> Self {
        Self {
            label: None,
            width,
            height,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            data,
        }
    }

    pub fn with_label(mut self, label: &'a str) -> Self {
        self.label = Some(label);
        self
    }
}

/// GPU side of a texture. Internal: users hold a `TextureId`.
#[derive(Debug)]
pub(crate) struct Texture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl Texture {
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        desc: &TextureDesc<'_>,
    ) -> Result<Self, RenderError> {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: desc.label,
            size: wgpu::Extent3d {
                width: desc.width,
                height: desc.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: desc.format,
            // TEXTURE_BINDING: sample it in shaders. COPY_DST: upload pixels.
            // RENDER_ATTACHMENT: render *into* it later (feedback effects).
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let this = Self { texture, view };
        if let Some(data) = desc.data {
            this.write(queue, data)?;
        }
        Ok(this)
    }

    /// Replace all pixels.
    pub(crate) fn write(&self, queue: &wgpu::Queue, data: &[u8]) -> Result<(), RenderError> {
        let tex = &self.texture;
        let bytes_per_pixel = tex
            .format()
            .block_copy_size(None)
            .expect("texture format has no fixed pixel size");
        let expected = (bytes_per_pixel * tex.width() * tex.height()) as usize;
        if data.len() != expected {
            return Err(RenderError::TextureDataSize { expected, got: data.len() });
        }
        queue.write_texture(
            tex.as_image_copy(),
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_pixel * tex.width()),
                rows_per_image: Some(tex.height()),
            },
            tex.size(),
        );
        Ok(())
    }

    pub(crate) fn view(&self) -> &wgpu::TextureView {
        &self.view
    }
}
