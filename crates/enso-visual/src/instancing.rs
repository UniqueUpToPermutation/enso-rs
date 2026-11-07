//! Sorting draws into instanced batches, shared by the mesh-like
//! subrenderers.
//!
//! Each frame a subrenderer pushes one `(key, instance)` per draw. `upload`
//! sorts them so that draws with the same shader, material and mesh are
//! adjacent, merges each run into one batch (one instanced draw call), and
//! writes all instance data into a single GPU vertex buffer.

use std::ops::Range;

use crate::{Gpu, MaterialId};

/// Draw order: opaque before blended (blended ones don't write depth), then
/// grouped by shader -> material -> mesh to minimise GPU state changes.
/// `derive(Ord)` compares fields top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct BatchKey<S, M> {
    pub blended: bool,
    pub shader: S,
    pub material: MaterialId,
    pub mesh: M,
}

pub(crate) struct Batch<S, M> {
    pub key: BatchKey<S, M>,
    pub instances: Range<u32>,
}

/// `S` = shader ID type, `M` = mesh ID type, `I` = GPU instance struct.
pub(crate) struct InstanceBatches<S, M, I> {
    label: &'static str,
    sorted: Vec<(BatchKey<S, M>, I)>,
    staging: Vec<I>,
    batches: Vec<Batch<S, M>>,
    buffer: wgpu::Buffer,
    capacity: usize,
}

impl<S: Ord + Copy, M: Ord + Copy, I: bytemuck::Pod> InstanceBatches<S, M, I> {
    pub(crate) fn new(device: &wgpu::Device, label: &'static str) -> Self {
        let capacity = 256;
        Self {
            label,
            sorted: Vec::new(),
            staging: Vec::new(),
            batches: Vec::new(),
            buffer: create_buffer::<I>(device, label, capacity),
            capacity,
        }
    }

    /// Start a new frame.
    pub(crate) fn clear(&mut self) {
        self.sorted.clear();
        self.staging.clear();
        self.batches.clear();
    }

    pub(crate) fn push(&mut self, key: BatchKey<S, M>, instance: I) {
        self.sorted.push((key, instance));
    }

    /// Sort, merge into batches and upload the instance data.
    pub(crate) fn upload(&mut self, gpu: &Gpu) {
        self.sorted.sort_by_key(|(key, _)| *key);

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
        if self.staging.len() > self.capacity {
            self.capacity = self.staging.len().next_power_of_two();
            self.buffer = create_buffer::<I>(&gpu.device, self.label, self.capacity);
        }
        gpu.queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&self.staging));
    }

    pub(crate) fn batches(&self) -> &[Batch<S, M>] {
        &self.batches
    }

    pub(crate) fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }
}

fn create_buffer<I>(device: &wgpu::Device, label: &str, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: (capacity * size_of::<I>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
