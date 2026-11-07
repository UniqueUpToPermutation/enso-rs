//! Rendering without a window: into a texture whose pixels can be read back.
//! Used by golden-image tests; also the basis for recording or thumbnails.

use std::sync::mpsc;

use crate::{Gpu, RenderError, RenderTarget};

/// A texture to render into instead of a window surface.
///
/// ```ignore
/// let offscreen = OffscreenTarget::new(&gpu, 640, 360, wgpu::TextureFormat::Rgba8UnormSrgb);
/// let mut renderer = WgpuRenderer::new(gpu.clone(), offscreen.format());
/// renderer.render(offscreen.target(), &prev, &curr, alpha);
/// let rgba = offscreen.read_pixels(&gpu)?;
/// ```
#[derive(Debug)]
pub struct OffscreenTarget {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl OffscreenTarget {
    /// `format` must have a fixed pixel size (e.g. `Rgba8UnormSrgb`).
    pub fn new(gpu: &Gpu, width: u32, height: u32, format: wgpu::TextureFormat) -> Self {
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("enso offscreen target"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            // COPY_SRC so read_pixels can copy it into a buffer.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Self { texture, view }
    }

    pub fn target(&self) -> RenderTarget<'_> {
        RenderTarget { view: &self.view, width: self.texture.width(), height: self.texture.height() }
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.texture.format()
    }

    pub fn size(&self) -> (u32, u32) {
        (self.texture.width(), self.texture.height())
    }

    /// Copy the pixels back to the CPU as tightly packed rows. Blocks until
    /// the GPU has finished all submitted work.
    pub fn read_pixels(&self, gpu: &Gpu) -> Result<Vec<u8>, RenderError> {
        let (width, height) = self.size();
        let bytes_per_pixel = self
            .format()
            .block_copy_size(None)
            .expect("offscreen format has no fixed pixel size");
        // GPU copies need each row to start on a 256-byte boundary, so rows
        // in the buffer are padded; the padding is stripped below.
        let unpadded_row = width * bytes_per_pixel;
        let padded_row = unpadded_row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("enso offscreen readback"),
            size: u64::from(padded_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("enso readback") });
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(height),
                },
            },
            self.texture.size(),
        );
        gpu.queue.submit([encoder.finish()]);

        // Mapping is asynchronous: the callback fires during `poll`.
        let (tx, rx) = mpsc::channel();
        buffer.map_async(wgpu::MapMode::Read, .., move |result| {
            let _ = tx.send(result);
        });
        gpu.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| RenderError::Readback(e.to_string()))?;
        rx.recv()
            .map_err(|e| RenderError::Readback(e.to_string()))?
            .map_err(|e| RenderError::Readback(e.to_string()))?;

        let mapped = buffer
            .get_mapped_range(..)
            .map_err(|e| RenderError::Readback(e.to_string()))?;
        let mut pixels = Vec::with_capacity((unpadded_row * height) as usize);
        for row in mapped.chunks(padded_row as usize) {
            pixels.extend_from_slice(&row[..unpadded_row as usize]);
        }
        drop(mapped);
        buffer.unmap();
        Ok(pixels)
    }
}
