//! Sorting draws into instanced batches, shared by the mesh-like
//! subrenderers.
//!
//! Each frame a subrenderer pushes one `(key, instance, parameter block)`
//! per draw. `upload` sorts them so that draws with the same shader,
//! material and mesh are adjacent, merges each run into one batch (one
//! instanced draw call), and writes two GPU buffers:
//!
//! * the **instance vertex buffer**: fixed per-instance data (transform,
//!   colour, ...) with the same layout for every shader;
//! * the **parameter storage buffer**: each instance's `struct Instance`
//!   block. Shaders declare different structs (different strides), so the
//!   buffer is split into one *segment* per shader, each starting at an
//!   aligned offset with its own bind group. An instance's `param_slot` is
//!   its index within its shader's segment.

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
    /// Index into [`InstanceBatches::segments`]: bind it before drawing.
    pub segment: usize,
}

/// GPU instance data that carries a slot into its shader's parameter segment.
pub(crate) trait InstanceData: bytemuck::Pod {
    fn set_param_slot(&mut self, slot: u32);
}

struct Entry<S, M, I> {
    key: BatchKey<S, M>,
    instance: I,
    /// This instance's parameter block, in `InstanceBatches::blocks`.
    block: Range<usize>,
}

/// `S` = shader ID type, `M` = mesh ID type, `I` = GPU instance struct.
pub(crate) struct InstanceBatches<S, M, I> {
    label: &'static str,
    entries: Vec<Entry<S, M, I>>,
    blocks: Vec<u8>,

    staging: Vec<I>,
    params: Vec<u8>,
    batches: Vec<Batch<S, M>>,
    segment_ranges: Vec<Range<usize>>,
    segments: Vec<wgpu::BindGroup>,

    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
    param_buffer: wgpu::Buffer,
    param_capacity: usize,
    params_layout: wgpu::BindGroupLayout,
    /// `min_storage_buffer_offset_alignment`: where segments may start.
    alignment: usize,
}

impl<S: Ord + Copy, M: Ord + Copy, I: InstanceData> InstanceBatches<S, M, I> {
    pub(crate) fn new(device: &wgpu::Device, label: &'static str) -> Self {
        let params_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(label),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None, // differs per shader
                },
                count: None,
            }],
        });
        let instance_capacity = 256;
        let param_capacity = 64 * 1024;
        Self {
            label,
            entries: Vec::new(),
            blocks: Vec::new(),
            staging: Vec::new(),
            params: Vec::new(),
            batches: Vec::new(),
            segment_ranges: Vec::new(),
            segments: Vec::new(),
            instance_buffer: create_buffer(device, label, instance_capacity * size_of::<I>(), wgpu::BufferUsages::VERTEX),
            instance_capacity,
            param_buffer: create_buffer(device, label, param_capacity, wgpu::BufferUsages::STORAGE),
            param_capacity,
            params_layout,
            alignment: device.limits().min_storage_buffer_offset_alignment as usize,
        }
    }

    /// The layout of the per-shader parameter bind group.
    pub(crate) fn params_layout(&self) -> &wgpu::BindGroupLayout {
        &self.params_layout
    }

    /// Start a new frame.
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.blocks.clear();
    }

    /// Queue one instance. `write_block` appends its parameter block (the
    /// shader's `struct Instance`, `size` bytes) to the given buffer.
    pub(crate) fn push(&mut self, key: BatchKey<S, M>, instance: I, write_block: impl FnOnce(&mut Vec<u8>)) {
        let start = self.blocks.len();
        write_block(&mut self.blocks);
        self.entries.push(Entry { key, instance, block: start..self.blocks.len() });
    }

    /// Sort, merge into batches, lay out parameter segments, and upload.
    pub(crate) fn upload(&mut self, gpu: &Gpu) {
        self.entries.sort_by_key(|e| e.key);

        self.staging.clear();
        self.params.clear();
        self.batches.clear();
        self.segment_ranges.clear();

        let mut segment_shader = None;
        let mut slot = 0;
        for entry in &self.entries {
            // Entries are sorted by shader, so each shader's are contiguous.
            if segment_shader != Some(entry.key.shader) {
                segment_shader = Some(entry.key.shader);
                slot = 0;
                let start = self.params.len().next_multiple_of(self.alignment);
                self.params.resize(start, 0);
                self.segment_ranges.push(start..start);
            }
            self.params.extend_from_slice(&self.blocks[entry.block.clone()]);
            self.segment_ranges.last_mut().unwrap().end = self.params.len();

            let mut instance = entry.instance;
            instance.set_param_slot(slot);
            slot += 1;

            let index = self.staging.len() as u32;
            self.staging.push(instance);
            let segment = self.segment_ranges.len() - 1;
            match self.batches.last_mut() {
                Some(batch) if batch.key == entry.key => batch.instances.end = index + 1,
                _ => self.batches.push(Batch { key: entry.key, instances: index..index + 1, segment }),
            }
        }

        self.segments.clear();
        if self.staging.is_empty() {
            return;
        }

        let device = &gpu.device;
        if self.staging.len() > self.instance_capacity {
            self.instance_capacity = self.staging.len().next_power_of_two();
            self.instance_buffer = create_buffer(
                device,
                self.label,
                self.instance_capacity * size_of::<I>(),
                wgpu::BufferUsages::VERTEX,
            );
        }
        if self.params.len() > self.param_capacity {
            self.param_capacity = self.params.len().next_power_of_two();
            self.param_buffer = create_buffer(device, self.label, self.param_capacity, wgpu::BufferUsages::STORAGE);
        }
        gpu.queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&self.staging));
        gpu.queue.write_buffer(&self.param_buffer, 0, &self.params);

        // One small bind group per shader segment per frame.
        for range in &self.segment_ranges {
            self.segments.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(self.label),
                layout: &self.params_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.param_buffer,
                        offset: range.start as u64,
                        size: wgpu::BufferSize::new(range.len() as u64),
                    }),
                }],
            }));
        }
    }

    pub(crate) fn batches(&self) -> &[Batch<S, M>] {
        &self.batches
    }

    pub(crate) fn segment(&self, index: usize) -> &wgpu::BindGroup {
        &self.segments[index]
    }

    pub(crate) fn instance_buffer(&self) -> &wgpu::Buffer {
        &self.instance_buffer
    }
}

fn create_buffer(device: &wgpu::Device, label: &str, size: usize, usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size as u64,
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
