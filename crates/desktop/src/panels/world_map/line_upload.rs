//! Bounded GPU publication shared by globe and local line layers.
use super::contour_pass::{InstanceChunk, split_instance_buffers};
use eframe::wgpu;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

const FRAME_BYTES: usize = 4 * 1024 * 1024;
const CHUNK_BYTES: usize = 512 * 1024;

pub(super) struct Budget {
    remaining: usize,
    started: Option<Instant>,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            remaining: FRAME_BYTES,
            started: None,
        }
    }
}
impl Budget {
    pub fn claim(&mut self, bytes: usize) -> bool {
        if bytes > self.remaining
            || self
                .started
                .is_some_and(|t| t.elapsed() >= Duration::from_millis(2))
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
    pub chunks: Vec<InstanceChunk>,
}
impl<T: bytemuck::Pod> Upload<T> {
    pub fn new(source: Arc<Vec<T>>) -> Self {
        Self {
            source,
            offset: 0,
            chunks: vec![],
        }
    }
    pub fn advance(&mut self, device: &wgpu::Device, budget: &mut Budget) -> bool {
        let stride = std::mem::size_of::<T>();
        while self.offset < self.source.len() {
            let end = (self.offset + (CHUNK_BYTES / stride).max(1)).min(self.source.len());
            if !budget.claim((end - self.offset) * stride) {
                return false;
            }
            self.chunks.extend(split_instance_buffers(
                device,
                "staged line upload",
                &self.source[self.offset..end],
            ));
            self.offset = end;
        }
        true
    }
    pub fn bytes(&self) -> u64 {
        (self.offset * std::mem::size_of::<T>()) as u64
    }
}
