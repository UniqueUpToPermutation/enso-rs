//! Opening the GPU.
//!
//! Every wgpu handle (`Device`, `Queue`, ...) is a cheap, reference-counted
//! `Clone`, so [`Gpu`] can be cloned freely and handed to every subrenderer.

/// The opened GPU: instance, chosen adapter, logical device and its queue.
#[derive(Debug, Clone)]
pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    #[error("no suitable GPU adapter found: {0}")]
    Adapter(#[from] wgpu::RequestAdapterError),
    #[error("failed to open the GPU device: {0}")]
    Device(#[from] wgpu::RequestDeviceError),
}

impl Gpu {
    /// Create a wgpu instance with default backends (DX12/Vulkan on Windows).
    ///
    /// Create this first, use it to make your window's `Surface`, then pass
    /// both to [`Gpu::new`] so the adapter we pick can present to that window.
    pub fn create_instance() -> wgpu::Instance {
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle())
    }

    /// Pick an adapter (preferring a discrete GPU) and open a device on it.
    pub async fn new(
        instance: wgpu::Instance,
        compatible_surface: Option<&wgpu::Surface<'_>>,
    ) -> Result<Self, GpuError> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface,
                ..Default::default()
            })
            .await?;

        log::info!("using GPU adapter: {:?}", adapter.get_info());

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("enso device"),
                ..Default::default()
            })
            .await?;

        Ok(Self { instance, adapter, device, queue })
    }

    /// Blocking version of [`Gpu::new`], for use outside async code.
    pub fn new_blocking(
        instance: wgpu::Instance,
        compatible_surface: Option<&wgpu::Surface<'_>>,
    ) -> Result<Self, GpuError> {
        pollster::block_on(Self::new(instance, compatible_surface))
    }
}
