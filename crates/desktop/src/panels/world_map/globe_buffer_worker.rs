//! One admitted batch at a time. Driver allocation and last-handle release can
//! block, even for small vertex buffers, so both run off the map thread.
use super::*;
use crossbeam_channel::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

pub(super) type Requests = Vec<(Arc<Tile>, usize)>;
type Completed = Vec<(u64, InstanceChunk)>;
enum Work {
    Upload(Requests),
    Retire(Vec<InstanceChunk>, Option<Arc<Prepared>>),
}
pub(super) struct Worker {
    tx: Sender<Work>,
    rx: Receiver<Completed>,
}
#[derive(Default)]
struct Pool {
    buffers: HashMap<u64, Vec<wgpu::Buffer>>,
    bytes: u64,
    allocation: Duration,
    copy: Duration,
    retirement: Duration,
    reused: usize,
}
#[inline]
pub(super) fn alloc_bytes(raw: usize) -> usize {
    // Power-of-two buckets up to 64 KiB, then 64 KiB multiples (max ~6% slack)
    // so retired partial-bin buffers are reused while strictly respecting budgets.
    let bytes = raw.max(4096);
    if bytes <= 65_536 {
        bytes.next_power_of_two()
    } else {
        (bytes + 65_535) & !65_535
    }
}

#[inline]
fn bucket_size(bytes: u64) -> u64 {
    alloc_bytes(bytes as usize) as u64
}

impl Pool {
    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, jobs: Requests) -> Completed {
        let mut out = Vec::with_capacity(jobs.len());
        for (tile, index) in jobs {
            let data = &tile.chunks[index].instances;
            let bytes = bytemuck::cast_slice(data);
            let raw_size = bytes.len() as u64;
            let alloc_size = bucket_size(raw_size);
            let start = Instant::now();
            let buffer = if let Some(buffer) = self.buffers.get_mut(&alloc_size).and_then(Vec::pop)
            {
                self.bytes -= alloc_size;
                self.reused += 1;
                queue.write_buffer(&buffer, 0, bytes);
                self.copy += start.elapsed();
                buffer
            } else {
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("globe terrain chunk"),
                    size: alloc_size,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: true,
                });
                self.allocation += start.elapsed();
                let start = Instant::now();
                buffer
                    .slice(..raw_size)
                    .get_mapped_range_mut()
                    .copy_from_slice(bytes);
                buffer.unmap();
                self.copy += start.elapsed();
                buffer
            };
            out.push((
                tile.version,
                InstanceChunk {
                    buffer,
                    count: data.len() as u32,
                },
            ));
        }
        out
    }
    fn retire(&mut self, chunks: Vec<InstanceChunk>, previous: Option<Arc<Prepared>>, cap: u64) {
        let start = Instant::now();
        for chunk in chunks {
            let size = chunk.buffer.size();
            if self.bytes + size <= cap {
                self.buffers.entry(size).or_default().push(chunk.buffer);
                self.bytes += size;
            }
        }
        drop(previous);
        self.buffers.retain(|_, buffers| !buffers.is_empty());
        self.retirement += start.elapsed();
    }
}
impl Worker {
    pub fn spawn(device: &wgpu::Device, queue: &wgpu::Queue) -> std::io::Result<Self> {
        let (tx, work) = crossbeam_channel::unbounded();
        let (done, rx) = crossbeam_channel::bounded(1);
        let device = device.clone();
        let queue = queue.clone();
        let cap = (crate::gpu_config::settings().upload_bytes * 2) as u64;
        let run = move || {
            let mut pool = Pool::default();
            for job in work {
                match job {
                    Work::Upload(requests) => {
                        if done.send(pool.upload(&device, &queue, requests)).is_err() {
                            break;
                        }
                    }
                    Work::Retire(chunks, previous) => pool.retire(chunks, previous, cap),
                }
            }
            if std::env::var_os("ONEKEE_TERRAIN_TIMINGS").is_some() {
                eprintln!(
                    "[terrain] buffer worker allocation {:?}, copy/unmap {:?}, retirement {:?}, {} buffers reused",
                    pool.allocation, pool.copy, pool.retirement, pool.reused
                );
            }
        };
        std::thread::Builder::new()
            .name("globe-buffer-upload".into())
            .spawn(run)?;
        Ok(Self { tx, rx })
    }
    pub fn submit(&self, jobs: Requests) -> bool {
        self.tx.send(Work::Upload(jobs)).is_ok()
    }
    pub fn poll(&self) -> Result<Completed, TryRecvError> {
        self.rx.try_recv()
    }
    pub fn retire(&self, chunks: Vec<InstanceChunk>, previous: Option<Arc<Prepared>>) {
        if !chunks.is_empty() || previous.is_some() {
            let _ = self.tx.send(Work::Retire(chunks, previous));
        }
    }
}
