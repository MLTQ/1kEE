//! Bounded GPU publication shared by globe and local line layers.
use super::contour_pass::{InstanceChunk, instances_per_buffer};
use eframe::wgpu;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

const CHUNK_BYTES: usize = 512 * 1024;

pub(super) struct Budget {
    remaining: usize,
    started: Option<Instant>,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            remaining: crate::gpu_config::settings().upload_bytes,
            started: None,
        }
    }
}
impl Budget {
    pub fn claim(&mut self, bytes: usize) -> bool {
        if bytes > self.remaining
            || self
                .started
                .is_some_and(|t| {
                    t.elapsed() >= Duration::from_micros(crate::gpu_config::settings().upload_us)
                })
        {
            return false;
        }
        self.started.get_or_insert_with(Instant::now);
        self.remaining -= bytes;
        true
    }
}

pub(super) struct Upload<T> {
    source: Arc<Vec<T>>,
    offset: usize,
    per_buffer: usize,
    pub chunks: Vec<InstanceChunk>,
}
impl<T: bytemuck::Pod> Upload<T> {
    pub fn new(source: Arc<Vec<T>>) -> Self {
        Self {
            source,
            offset: 0,
            per_buffer: 0,
            chunks: vec![],
        }
    }
    pub fn advance(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        budget: &mut Budget,
    ) -> bool {
        let stride = std::mem::size_of::<T>();
        if self.source.is_empty() {
            return true;
        }
        let chunk_items = (CHUNK_BYTES / stride).max(1);
        while self.offset < self.source.len() {
            // Allocate one vertex buffer per max_buffer_size slice (usually 1
            // buffer for the whole tile/layer) instead of a new buffer every
            // 512 KiB, then stream slices via queue.write_buffer under Budget.
            if self.chunks.is_empty() {
                let per_buffer = instances_per_buffer(device.limits().max_buffer_size, stride);
                self.per_buffer = per_buffer;
                let total = self.source.len();
                let mut start = 0;
                while start < total {
                    let count = (total - start).min(per_buffer);
                    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("staged line upload"),
                        size: (count * stride) as u64,
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    self.chunks.push(InstanceChunk {
                        buffer,
                        count: count as u32,
                    });
                    start += count;
                }
            }
            let buf_idx = self.offset / self.per_buffer;
            let buf_start = buf_idx * self.per_buffer;
            let buf_end = (buf_start + self.chunks[buf_idx].count as usize).min(self.source.len());
            let end = (self.offset + chunk_items).min(buf_end);
            if !budget.claim((end - self.offset) * stride) {
                return false;
            }
            let byte_offset = ((self.offset - buf_start) * stride) as u64;
            queue.write_buffer(
                &self.chunks[buf_idx].buffer,
                byte_offset,
                bytemuck::cast_slice(&self.source[self.offset..end]),
            );
            self.offset = end;
        }
        true
    }
    pub fn bytes(&self) -> u64 {
        (self.offset * std::mem::size_of::<T>()) as u64
    }
}
