//! SRTM GL1 compute backend. Sources are read-only; callers own cache writes.
//! One serialized device and a twelve-window VRAM LRU are shared by background jobs.
use crate::contours::{self, TileKey};
use crate::marching_squares::{ContourLine, FxHashMap, chain_segments};
use eframe::wgpu::{self, util::DeviceExt};
use rayon::prelude::*;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::Instant,
};
use tile_archive::contour_grid::{Bounds, CoreTile};

mod deflate;
mod window;
use window::Window;

const NODATA: f32 = -32768.0;
const MAX_N: u32 = 6000;
const CAPACITY: u32 = 4_000_000;
type Lines = (Vec<ContourLine>, Vec<Vec<(f32, f32)>>);

// The desktop supplies its actual render adapter before starting terrain jobs.
// Headless builders continue to require an explicit opt-in.
static DESKTOP_ADAPTER: OnceLock<wgpu::AdapterInfo> = OnceLock::new();
static WORKER: OnceLock<Mutex<Result<Worker, String>>> = OnceLock::new();

pub fn register_desktop_adapter(info: wgpu::AdapterInfo) {
    let _ = DESKTOP_ADAPTER.set(info);
}

fn select_compute(setting: Option<&str>, adapter: Option<&wgpu::AdapterInfo>) -> bool {
    match setting {
        Some(value) => value == "1",
        // PCI vendor/device pair of the tested desktop RTX 4090.
        None => adapter.is_some_and(|info| {
            info.backend == wgpu::Backend::Vulkan && info.vendor == 0x10de && info.device == 0x2684
        }),
    }
}

pub fn enabled() -> bool {
    select_compute(
        std::env::var("ONEKEE_TERRAIN_GPU").ok().as_deref(),
        DESKTOP_ADAPTER.get(),
    )
}

/// Finish submitted work and destroy the compute device before the process
/// unloads its graphics driver. Pending tile jobs see the stopped worker.
pub fn shutdown() {
    let stopped = || Err("terrain compute has shut down".to_owned());
    let worker = WORKER.get_or_init(|| Mutex::new(stopped()));
    let mut state = worker
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Ok(worker) = std::mem::replace(&mut *state, stopped()) {
        worker.device.poll(wgpu::Maintain::Wait);
        drop(worker);
    }
}

/// Generate a complete tile in memory. Does not modify any files. An error must
/// fall back or abort; it must never be committed as an empty successful tile.
pub fn build(root: &Path, zoom: i32, lat: i32, lon: i32) -> Result<TerrainTile, String> {
    let build_started = Instant::now();
    let spec = contours::all_specs()
        .into_iter()
        .find(|s| s.zoom_bucket == zoom)
        .ok_or("GPU terrain supports SRTM zoom buckets 0–6")?;
    let core = CoreTile::srtm(spec.half_extent_deg, lat, lon);
    validate_grid(core.source, core.raster_size)?;
    let mut state = WORKER
        .get_or_init(|| Mutex::new(Worker::new()))
        .lock()
        .map_err(|_| "GPU terrain worker unavailable".to_string())?;
    let worker = state.as_mut().map_err(|e| e.clone())?;
    let worker_wait_ms = build_started.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    worker
        .device
        .push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let computed = worker.resample_and_extract(
        root,
        core.source,
        core.raster_size,
        spec.interval_m as f32,
        CAPACITY,
        start,
    );
    if let Some(error) = block_on(worker.device.pop_error_scope()) {
        let error = format!("GPU terrain disabled after allocation failure: {error}");
        *state = Err(error.clone());
        return Err(error);
    }
    let (raw, source_ms, compute_ms) = computed?;
    // Other tile jobs can submit GPU work while this one stitches on the CPU.
    drop(state);
    let segment_count = raw.len();
    let start_stitch = Instant::now();
    let (contours, coastlines) = stitch(raw, core.source, core.raster_size);
    eprintln!(
        "[terrain-gpu] tile={zoom}/{lat}/{lon} worker_wait_ms={worker_wait_ms:.3} source_ms={source_ms:.3} compute_readback_ms={compute_ms:.3} stitch_ms={:.3} total_ms={:.3} segments={segment_count}",
        start_stitch.elapsed().as_secs_f64() * 1000.0,
        build_started.elapsed().as_secs_f64() * 1000.0
    );
    Ok(TerrainTile {
        tile: TileKey {
            zoom_bucket: zoom,
            lat_bucket: lat,
            lon_bucket: lon,
        },
        contours,
        coastlines,
    })
}

pub struct TerrainTile {
    tile: TileKey,
    contours: Vec<ContourLine>,
    coastlines: Vec<Vec<(f32, f32)>>,
}
impl TerrainTile {
    /// Atomically write both layers and manifests using existing core clipping.
    /// The caller must open/schema-initialize the intended destination first.
    pub fn write(&self, connection: &mut rusqlite::Connection) -> rusqlite::Result<()> {
        contours::write_tile_native(connection, self.tile, &self.contours, &self.coastlines)
    }
}

struct Source {
    path: PathBuf,
    modified: Option<std::time::SystemTime>,
    size: u64,
    window: Window,
    buffer: wgpu::Buffer,
}
struct Worker {
    device: wgpu::Device,
    queue: wgpu::Queue,
    initialize_pipeline: wgpu::ComputePipeline,
    sample_pipeline: wgpu::ComputePipeline,
    contour_pipeline: wgpu::ComputePipeline,
    sources: Vec<Source>,
    tiff_meta: HashMap<PathBuf, deflate::FastTiffMeta>,
    block_cache: deflate::BlockCache,
    grid_buffer: Option<wgpu::Buffer>,
    segment_buffer: Option<wgpu::Buffer>,
    count_buffer: wgpu::Buffer,
    count_staging: wgpu::Buffer,
    readback_staging: Option<wgpu::Buffer>,
}
fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    struct Wake(std::thread::Thread);
    impl std::task::Wake for Wake {
        fn wake(self: std::sync::Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = std::task::Waker::from(std::sync::Arc::new(Wake(std::thread::current())));
    let mut context = std::task::Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            std::task::Poll::Ready(v) => return v,
            std::task::Poll::Pending => {
                std::thread::park_timeout(std::time::Duration::from_millis(10))
            }
        }
    }
}
impl Worker {
    fn new() -> Result<Self, String> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });
        let requested = std::env::var("ONEKEE_GPU").ok().map(|v| v.to_lowercase());
        let auto_adapter = DESKTOP_ADAPTER
            .get()
            .filter(|_| std::env::var_os("ONEKEE_TERRAIN_GPU").is_none());
        let adapter = instance
            .enumerate_adapters(wgpu::Backends::PRIMARY)
            .into_iter()
            .filter(|a| {
                let info = a.get_info();
                if let Some(desktop) = auto_adapter {
                    return info.vendor == desktop.vendor
                        && info.device == desktop.device
                        && info.backend == desktop.backend;
                }
                requested
                    .as_ref()
                    .is_none_or(|s| info.name.to_lowercase().contains(s))
            })
            .min_by_key(|a| match a.get_info().device_type {
                wgpu::DeviceType::DiscreteGpu => 0,
                wgpu::DeviceType::IntegratedGpu => 1,
                _ => 2,
            })
            .ok_or("No matching compute GPU")?;
        let limits = wgpu::Limits {
            max_storage_buffer_binding_size: MAX_N * MAX_N * 4,
            max_buffer_size: u64::from(MAX_N) * u64::from(MAX_N) * 4,
            ..Default::default()
        };
        let (device, queue) = block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("SRTM compute worker"),
                required_limits: limits,
                ..Default::default()
            },
            None,
        ))
        .map_err(|e| e.to_string())?;
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let make = |source: &'static str| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("SRTM compute"),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("SRTM compute"),
                layout: None,
                module: &module,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let initialize_pipeline = make(include_str!("initialize.wgsl"));
        let sample_pipeline = make(include_str!("resample.wgsl"));
        let contour_pipeline = make(include_str!("contours.wgsl"));
        if let Some(e) = block_on(device.pop_error_scope()) {
            return Err(e.to_string());
        }
        eprintln!(
            "[terrain-gpu] adapter={} backend={:?} source_cache=12 windows",
            adapter.get_info().name,
            adapter.get_info().backend
        );
        let count_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("segment count"),
            size: 4,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let count_staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("segment count staging"),
            size: 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Ok(Self {
            device,
            queue,
            initialize_pipeline,
            sample_pipeline,
            contour_pipeline,
            sources: Vec::new(),
            tiff_meta: HashMap::new(),
            block_cache: Default::default(),
            grid_buffer: None,
            segment_buffer: None,
            count_buffer,
            count_staging,
            readback_staging: None,
        })
    }
    fn buffer(&self, label: &str, data: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: data,
                usage,
            })
    }
    fn empty(&self, label: &str, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage,
            mapped_at_creation: false,
        })
    }
    fn encode_dispatch(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::ComputePipeline,
        buffers: &[&wgpu::Buffer],
        n: u32,
    ) {
        self.encode_dispatch_2d(encoder, pipeline, buffers, n, n);
    }
    fn encode_dispatch_2d(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::ComputePipeline,
        buffers: &[&wgpu::Buffer],
        width: u32,
        height: u32,
    ) {
        let entries: Vec<_> = buffers
            .iter()
            .enumerate()
            .map(|(i, b)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: b.as_entire_binding(),
            })
            .collect();
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &entries,
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(width.div_ceil(16), height.div_ceil(16), 1);
    }
    #[cfg(test)]
    fn dispatch(&self, pipeline: &wgpu::ComputePipeline, buffers: &[&wgpu::Buffer], n: u32) {
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.encode_dispatch(&mut encoder, pipeline, buffers, n);
        self.queue.submit(Some(encoder.finish()));
    }
    #[cfg(test)]
    fn read(&self, buffer: &wgpu::Buffer, size: u64) -> Result<Vec<u8>, String> {
        let staging = self.empty(
            "terrain readback",
            size,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        );
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, size);
        self.queue.submit(Some(encoder.finish()));
        let (tx, rx) = std::sync::mpsc::channel();
        staging
            .slice(..size)
            .map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let out = staging.slice(..size).get_mapped_range().to_vec();
        staging.unmap();
        Ok(out)
    }
    fn read_mapped<R>(
        &mut self,
        buffer: &wgpu::Buffer,
        size: u64,
        f: impl FnOnce(&[u8]) -> R,
    ) -> Result<R, String> {
        if self
            .readback_staging
            .as_ref()
            .is_none_or(|b| b.size() < size)
        {
            self.readback_staging = Some(self.empty(
                "terrain readback",
                size.next_power_of_two().max(4096),
                wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            ));
        }
        let staging = self.readback_staging.as_ref().unwrap();
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(buffer, 0, staging, 0, size);
        self.queue.submit(Some(encoder.finish()));
        let (tx, rx) = std::sync::mpsc::channel();
        staging
            .slice(..size)
            .map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let out = {
            let mapped = staging.slice(..size).get_mapped_range();
            f(&mapped)
        };
        staging.unmap();
        Ok(out)
    }
    fn source(&mut self, path: &Path, lat: i32, lon: i32, needed: Window) -> Result<usize, String> {
        let meta = path
            .metadata()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let modified = meta.modified().ok();
        let size = meta.len();
        if let Some(i) = self.sources.iter().position(|s| {
            s.path == path
                && s.modified == modified
                && s.size == size
                && s.window.contains(needed)
        }) {
            let entry = self.sources.remove(i);
            self.sources.push(entry);
            return Ok(self.sources.len() - 1);
        }
        // Fast path: reuse cached validated TIFF IFD metadata (offsets/lengths) so
        // repeated windows from the same 1° SRTM tile skip IFD parsing and even
        // File::open when all needed blocks hit BlockCache.
        let fast_decoded = if let Some(cached_meta) = self.tiff_meta.get(path)
            && cached_meta.modified == modified
            && cached_meta.size == size
            && cached_meta.lat == lat
            && cached_meta.lon == lon
        {
            window::load_fast(path, cached_meta, needed, &mut self.block_cache)?
        } else {
            None
        };
        let decoded = match fast_decoded {
            Some(d) => d,
            None => {
                let mut decoder = window::open(path)?;
                validate_source(&mut decoder, path, lat, lon)?;
                let d = window::load_cached_with_coords(
                    decoder,
                    path,
                    lat,
                    lon,
                    needed,
                    &mut self.block_cache,
                )?;
                if let Some(fast_meta) = d.fast_meta {
                    self.tiff_meta.insert(path.to_path_buf(), fast_meta);
                }
                window::Decoded {
                    fast_meta: None,
                    ..d
                }
            }
        };
        let upload_start = Instant::now();
        let buffer = self.buffer(
            "resident SRTM",
            bytemuck::cast_slice(&decoded.samples),
            wgpu::BufferUsages::STORAGE,
        );
        let (resident_mib, limit_mib) = self.block_cache.usage_mib();
        eprintln!(
            "[terrain-gpu-source] file={} chunks={} reused_blocks={} bytes_read={} decoded_pixels={} io_ms={:.3} decode_ms={:.3} upload_submit_ms={:.3} block_cache_mib={resident_mib:.1} block_cache_limit_mib={limit_mib:.1}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            decoded.chunks,
            decoded.hits,
            decoded.bytes_read,
            decoded.samples.len(),
            decoded.io_ms,
            decoded.cpu_ms,
            upload_start.elapsed().as_secs_f64() * 1000.0
        );
        self.sources.retain(|s| {
            s.path != path
                || (s.modified == modified
                    && s.size == size
                    && !decoded.window.contains(s.window))
        });
        if self.sources.len() >= 12 {
            self.sources.remove(0);
        }
        self.sources.push(Source {
            path: path.into(),
            modified,
            size,
            window: decoded.window,
            buffer,
        });
        Ok(self.sources.len() - 1)
    }
    fn encode_resample_into(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        grid: &wgpu::Buffer,
        root: &Path,
        bounds: Bounds,
        n: u32,
    ) -> Result<(), String> {
        let init_params = self.buffer(
            "sampling parameters",
            bytemuck::cast_slice(&[n, 3601, 3601, 0]),
            wgpu::BufferUsages::UNIFORM,
        );
        self.encode_dispatch(encoder, &self.initialize_pipeline, &[&init_params, grid], n);
        let mut found = false;
        for lat in bounds.min_lat.floor() as i32..=bounds.max_lat.floor() as i32 {
            for lon in bounds.min_lon.floor() as i32..=bounds.max_lon.floor() as i32 {
                let path = root.join(format!(
                    "{}{:02}{}{:03}.tif",
                    if lat >= 0 { 'N' } else { 'S' },
                    lat.unsigned_abs(),
                    if lon >= 0 { 'E' } else { 'W' },
                    lon.unsigned_abs()
                ));
                if !path.try_exists().map_err(|e| e.to_string())? {
                    continue;
                }
                let mut axes = axes(bounds, n, lat, lon);
                let Some(needed) = Window::from_axes(&axes, n as usize) else {
                    continue;
                };
                let Some((out_x0, out_w)) = active_axis_span(&axes[..n as usize]) else {
                    continue;
                };
                let Some((out_y0, out_h)) = active_axis_span(&axes[n as usize..]) else {
                    continue;
                };
                let i = self.source(&path, lat, lon, needed)?;
                found = true;
                let window = self.sources[i].window;
                for v in &mut axes[..n as usize] {
                    if *v >= 0.0 {
                        *v -= window.x as f32;
                    }
                }
                for v in &mut axes[n as usize..] {
                    if *v >= 0.0 {
                        *v -= window.y as f32;
                    }
                }
                let origin = (out_x0 & 0xffff) | ((out_y0 & 0xffff) << 16);
                let params = self.buffer(
                    "window sampling parameters",
                    bytemuck::cast_slice(&[n, window.width, window.height, origin]),
                    wgpu::BufferUsages::UNIFORM,
                );
                let axes = self.buffer(
                    "local pixel coordinates",
                    bytemuck::cast_slice(&axes),
                    wgpu::BufferUsages::STORAGE,
                );
                self.encode_dispatch_2d(
                    encoder,
                    &self.sample_pipeline,
                    &[&params, &self.sources[i].buffer, &axes, grid],
                    out_w,
                    out_h,
                );
            }
        }
        if !found {
            return Err("No SRTM sources for GPU tile".into());
        }
        Ok(())
    }
    #[cfg(test)]
    fn resample(&mut self, root: &Path, bounds: Bounds, n: u32) -> Result<wgpu::Buffer, String> {
        validate_grid(bounds, n)?;
        let grid = self.empty(
            "sampled terrain",
            u64::from(n) * u64::from(n) * 4,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.encode_resample_into(&mut encoder, &grid, root, bounds, n)?;
        self.queue.submit(Some(encoder.finish()));
        Ok(grid)
    }
    fn resample_and_extract(
        &mut self,
        root: &Path,
        bounds: Bounds,
        n: u32,
        interval: f32,
        limit: u32,
        start: Instant,
    ) -> Result<(Vec<[f32; 5]>, f64, f64), String> {
        validate_grid(bounds, n)?;
        if !(2..=MAX_N).contains(&n)
            || !interval.is_finite()
            || interval < 1.0
            || limit == 0
            || limit > CAPACITY
        {
            return Err("Invalid GPU contour dimensions or interval".into());
        }
        let grid_bytes = u64::from(n) * u64::from(n) * 4;
        if self
            .grid_buffer
            .as_ref()
            .is_none_or(|b| b.size() < grid_bytes)
        {
            self.grid_buffer = Some(self.empty(
                "sampled terrain",
                grid_bytes,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            ));
        }
        let previous = self
            .segment_buffer
            .as_ref()
            .map_or(0, |b| (b.size() / 20) as u32);
        let mut capacity = (n * n / 4).max(64).max(previous).min(limit);
        let seg_bytes = u64::from(capacity) * 20;
        if self
            .segment_buffer
            .as_ref()
            .is_none_or(|b| b.size() < seg_bytes)
        {
            self.segment_buffer = Some(self.empty(
                "contour segments",
                seg_bytes,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            ));
        }

        // Record initialize + source resamples + initial contour extraction + count
        // copy into a single CommandEncoder submit.
        let grid = self.grid_buffer.take().unwrap();
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let resample_res = self.encode_resample_into(&mut encoder, &grid, root, bounds, n);
        if let Err(e) = resample_res {
            self.grid_buffer = Some(grid);
            return Err(e);
        }
        let source_ms = start.elapsed().as_secs_f64() * 1000.0;
        let start_contour = Instant::now();

        let count_res = (|| -> Result<u32, String> {
            let mut first_encoder = Some(encoder);
            loop {
                let bytes = u64::from(capacity) * 20;
                if self
                    .segment_buffer
                    .as_ref()
                    .is_none_or(|b| b.size() < bytes)
                {
                    self.segment_buffer = Some(self.empty(
                        "contour segments",
                        bytes,
                        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                    ));
                }
                let segments = self.segment_buffer.as_ref().unwrap();
                let params = self.buffer(
                    "contour parameters",
                    bytemuck::cast_slice(&[n, capacity, interval.to_bits(), 0]),
                    wgpu::BufferUsages::UNIFORM,
                );
                self.queue
                    .write_buffer(&self.count_buffer, 0, bytemuck::bytes_of(&0u32));
                let mut enc = first_encoder
                    .take()
                    .unwrap_or_else(|| self.device.create_command_encoder(&Default::default()));
                self.encode_dispatch(
                    &mut enc,
                    &self.contour_pipeline,
                    &[&params, &grid, segments, &self.count_buffer],
                    n,
                );
                enc.copy_buffer_to_buffer(&self.count_buffer, 0, &self.count_staging, 0, 4);
                self.queue.submit(Some(enc.finish()));
                let (tx, rx) = std::sync::mpsc::channel();
                self.count_staging
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |r| {
                        let _ = tx.send(r);
                    });
                self.device.poll(wgpu::Maintain::Wait);
                rx.recv()
                    .map_err(|e| e.to_string())?
                    .map_err(|e| e.to_string())?;
                let count = {
                    let mapped = self.count_staging.slice(..).get_mapped_range();
                    u32::from_ne_bytes(mapped[..4].try_into().unwrap())
                };
                self.count_staging.unmap();
                if count <= capacity {
                    return Ok(count);
                }
                if capacity == limit {
                    return Err(format!(
                        "GPU contour capacity exceeded ({count}>{capacity}); no partial output accepted"
                    ));
                }
                capacity = (capacity * 4).min(limit);
            }
        })();
        self.grid_buffer = Some(grid);
        let count = count_res?;
        let raw = if count == 0 {
            Vec::new()
        } else {
            let segments = self.segment_buffer.take().unwrap();
            let res = self.read_mapped(&segments, u64::from(count) * 20, |mapped| {
                bytemuck::cast_slice::<u8, [f32; 5]>(mapped).to_vec()
            });
            self.segment_buffer = Some(segments);
            res?
        };
        let compute_ms = start_contour.elapsed().as_secs_f64() * 1000.0;
        Ok((raw, source_ms, compute_ms))
    }
    #[cfg(test)]
    fn extract(
        &mut self,
        grid: &wgpu::Buffer,
        n: u32,
        interval: f32,
        limit: u32,
    ) -> Result<Vec<[f32; 5]>, String> {
        if !(2..=MAX_N).contains(&n)
            || !interval.is_finite()
            || interval < 1.0
            || limit == 0
            || limit > CAPACITY
        {
            return Err("Invalid GPU contour dimensions or interval".into());
        }
        // Most fine tiles use a fraction of the maximum. Grow on overflow and
        // retain one high-water buffer on the serialized worker for subsequent tiles.
        let previous = self
            .segment_buffer
            .as_ref()
            .map_or(0, |b| (b.size() / 20) as u32);
        let mut capacity = (n * n / 4).max(64).max(previous).min(limit);
        let count = loop {
            let bytes = u64::from(capacity) * 20;
            if self
                .segment_buffer
                .as_ref()
                .is_none_or(|b| b.size() < bytes)
            {
                self.segment_buffer = Some(self.empty(
                    "contour segments",
                    bytes,
                    wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                ));
            }
            let segments = self.segment_buffer.as_ref().unwrap();
            let params = self.buffer(
                "contour parameters",
                bytemuck::cast_slice(&[n, capacity, interval.to_bits(), 0]),
                wgpu::BufferUsages::UNIFORM,
            );
            self.queue
                .write_buffer(&self.count_buffer, 0, bytemuck::bytes_of(&0u32));
            let mut encoder = self.device.create_command_encoder(&Default::default());
            self.encode_dispatch(
                &mut encoder,
                &self.contour_pipeline,
                &[&params, grid, segments, &self.count_buffer],
                n,
            );
            encoder.copy_buffer_to_buffer(&self.count_buffer, 0, &self.count_staging, 0, 4);
            self.queue.submit(Some(encoder.finish()));
            let (tx, rx) = std::sync::mpsc::channel();
            self.count_staging
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| {
                    let _ = tx.send(r);
                });
            self.device.poll(wgpu::Maintain::Wait);
            rx.recv()
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
            let count = {
                let mapped = self.count_staging.slice(..).get_mapped_range();
                u32::from_ne_bytes(mapped[..4].try_into().unwrap())
            };
            self.count_staging.unmap();
            if count <= capacity {
                break count;
            }
            if capacity == limit {
                return Err(format!(
                    "GPU contour capacity exceeded ({count}>{capacity}); no partial output accepted"
                ));
            }
            capacity = (capacity * 4).min(limit);
        };
        if count == 0 {
            return Ok(Vec::new());
        }
        let segments = self.segment_buffer.take().unwrap();
        let result = self.read_mapped(&segments, u64::from(count) * 20, |mapped| {
            bytemuck::cast_slice::<u8, [f32; 5]>(mapped).to_vec()
        });
        self.segment_buffer = Some(segments);
        result
    }
}
fn validate_grid(b: Bounds, n: u32) -> Result<(), String> {
    if !(2..=MAX_N).contains(&n)
        || ![b.min_lon, b.max_lon, b.min_lat, b.max_lat]
            .iter()
            .all(|v| v.is_finite())
        || b.min_lon >= b.max_lon
        || b.min_lat >= b.max_lat
        || b.min_lon < -180.0
        || b.max_lon > 180.0
        || b.min_lat < -90.0
        || b.max_lat > 90.0
        || b.max_lon - b.min_lon > 2.0
        || b.max_lat - b.min_lat > 2.0
    {
        return Err("Unsupported GPU terrain grid".into());
    }
    Ok(())
}
fn active_axis_span(values: &[f32]) -> Option<(u32, u32)> {
    let first = values.iter().position(|&v| v >= 0.0)?;
    let last = values.iter().rposition(|&v| v >= 0.0)?;
    Some((first as u32, (last - first + 1) as u32))
}
fn axes(b: Bounds, n: u32, lat: i32, lon: i32) -> Vec<f32> {
    let local = |value: f64, base: i32, flip: bool| {
        // Snap tiny transform error back to a post before sampling.
        let post = value * 3600.0;
        let value = if (post - post.round()).abs() < 1e-6 {
            post.round() / 3600.0
        } else {
            value
        };
        if value < f64::from(base) || value > f64::from(base + 1) {
            return -1.0;
        }
        let v = (value - f64::from(base)) * 3600.0;
        (if flip { 3600.0 - v } else { v }) as f32
    };
    (0..n)
        .map(|i| {
            local(
                b.min_lon + (f64::from(i) + 0.5) * (b.max_lon - b.min_lon) / f64::from(n),
                lon,
                false,
            )
        })
        .chain((0..n).map(|i| {
            local(
                b.max_lat - (f64::from(i) + 0.5) * (b.max_lat - b.min_lat) / f64::from(n),
                lat,
                true,
            )
        }))
        .collect()
}
fn validate_source(
    d: &mut window::TiffReader,
    path: &Path,
    lat: i32,
    lon: i32,
) -> Result<(), String> {
    use tiff::tags::Tag;
    let mut check = || -> Result<bool, Box<dyn std::error::Error>> {
        let scale = d.get_tag_f64_vec(Tag::ModelPixelScaleTag)?;
        let tie = d.get_tag_f64_vec(Tag::ModelTiepointTag)?;
        let keys = d.get_tag_u16_vec(Tag::GeoKeyDirectoryTag)?;
        let format = d.get_tag_u16_vec(Tag::SampleFormat)?;
        let nodata = d.get_tag_ascii_string(Tag::GdalNodata)?;
        if format != [2]
            || nodata.trim_end_matches('\0').parse::<f32>()? != NODATA
            || d.find_tag(Tag::ModelTransformationTag)?.is_some()
        {
            return Ok(false);
        }
        let key = |id: u16, value: u16| {
            keys.get(4..)
                .is_some_and(|k| k.as_chunks::<4>().0.contains(&[id, 0, 1, value]))
        };
        Ok(d.dimensions()? == (3601, 3601)
            && scale.len() >= 2
            && tie.len() == 6
            && (scale[0] - 1.0 / 3600.0).abs() < 1e-12
            && (scale[1] - 1.0 / 3600.0).abs() < 1e-12
            && tie[0] == 0.0
            && tie[1] == 0.0
            && (tie[3] - f64::from(lon)).abs() < 1e-9
            && (tie[4] - f64::from(lat + 1)).abs() < 1e-9
            && key(1025, 2)
            && key(2048, 4326))
    };
    match check() {
        Ok(true) => Ok(()),
        other => Err(format!(
            "Unsupported SRTM georeferencing {}: {other:?}",
            path.display()
        )),
    }
}
fn stitch(raw: Vec<[f32; 5]>, b: Bounds, n: u32) -> Lines {
    // Elevations are integral. Partition in O(N) via FxHashMap, then sort the
    // small set of distinct elevation levels so output order stays canonical.
    let mut levels: FxHashMap<i32, Vec<[f32; 4]>> = FxHashMap::default();
    for [x, y, z, w, h] in raw {
        levels.entry(h as i32).or_default().push([x, y, z, w]);
    }
    let mut levels: Vec<(i32, Vec<[f32; 4]>)> = levels.into_iter().collect();
    levels.sort_unstable_by_key(|(h, _)| *h);
    let coord = |x: f32, y: f32| {
        (
            (b.min_lon + (f64::from(x) + 0.5) * (b.max_lon - b.min_lon) / f64::from(n)) as f32,
            (b.max_lat - (f64::from(y) + 0.5) * (b.max_lat - b.min_lat) / f64::from(n)) as f32,
        )
    };
    let lines_by_level: Vec<_> = levels
        .into_par_iter()
        .map(|(h, mut segments)| {
            // GPU atomics scatter segments in arbitrary order. Keep the canonical
            // raster order for equal-level endpoint ties and stable feature IDs.
            segments.sort_unstable_by(|a, b| {
                a[1].total_cmp(&b[1])
                    .then_with(|| a[0].total_cmp(&b[0]))
                    .then_with(|| a[3].total_cmp(&b[3]))
                    .then_with(|| a[2].total_cmp(&b[2]))
            });
            let segments = segments
                .into_iter()
                .map(|[x, y, z, w]| [coord(x, y), coord(z, w)])
                .collect();
            (h, chain_segments(segments))
        })
        .collect();
    let mut contours = Vec::new();
    let mut coastlines = Vec::new();
    for (h, lines) in lines_by_level {
        if h == 0 {
            coastlines = lines.clone();
        }
        // GDAL's interval contours include zero as well as the separate
        // coastline layer. Preserve both so cache consumers see the same data.
        contours.extend(lines.into_iter().map(|points| ContourLine {
            elevation_m: h as f32,
            points,
        }));
    }
    (contours, coastlines)
}

#[cfg(test)]
mod tests;

pub(crate) fn build_bbox(
    command: &crate::args::ContoursBboxCommand,
    progress: &mut dyn FnMut(contours::ContourBuildProgress),
) -> Result<String, String> {
    let mut db = contours::open_cache_db(&command.cache_db_path).map_err(|e| e.to_string())?;
    tile_archive::native_contours::initialize(&db).map_err(|e| e.to_string())?;
    let existing = contours::load_manifest_set(&db).map_err(|e| e.to_string())?;
    let specs: Vec<_> = contours::all_specs()
        .into_iter()
        .filter(|s| command.zoom_buckets.contains(&s.zoom_bucket))
        .collect();

    let mut pending = Vec::new();
    let mut skipped = 0u64;
    for spec in specs {
        let step = spec.half_extent_deg * 0.45;
        for lat in contours::bucket_range(command.min_lat, command.max_lat, step) {
            for lon in contours::bucket_range(command.min_lon, command.max_lon, step) {
                if existing.contains(&(spec.zoom_bucket, lat, lon)) {
                    skipped += 1;
                } else {
                    pending.push((spec, lat, lon));
                }
            }
        }
    }
    let total = skipped + pending.len() as u64;
    let mut built = 0u64;

    if !pending.is_empty() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            mpsc,
        };
        type RawMsg = Result<(TileKey, CoreTile, Vec<[f32; 5]>), String>;
        type EncodedMsg = Result<(TileKey, Vec<(i64, f32, Vec<u8>)>, Vec<(i64, Vec<u8>)>), String>;

        let stop = AtomicBool::new(false);
        let srtm_root = &command.srtm_root;
        let (raw_tx, raw_rx) = mpsc::sync_channel::<RawMsg>(4);
        let (enc_tx, enc_rx) = mpsc::sync_channel::<EncodedMsg>(6);

        let pipeline_result: Result<(), String> = std::thread::scope(|scope| {
            let stop_ref = &stop;
            // Stage 1: GPU resample + contour segment extraction.
            let gpu_thread = scope.spawn(move || {
                for (spec, lat, lon) in pending {
                    if stop_ref.load(Ordering::Relaxed) {
                        break;
                    }
                    let core = CoreTile::srtm(spec.half_extent_deg, lat, lon);
                    let res = (|| -> Result<(TileKey, CoreTile, Vec<[f32; 5]>), String> {
                        validate_grid(core.source, core.raster_size)?;
                        let mut state = WORKER
                            .get_or_init(|| Mutex::new(Worker::new()))
                            .lock()
                            .map_err(|_| "GPU terrain worker unavailable".to_string())?;
                        let worker = state.as_mut().map_err(|e| e.clone())?;
                        let start = Instant::now();
                        worker
                            .device
                            .push_error_scope(wgpu::ErrorFilter::OutOfMemory);
                        let computed = worker.resample_and_extract(
                            srtm_root,
                            core.source,
                            core.raster_size,
                            spec.interval_m as f32,
                            CAPACITY,
                            start,
                        );
                        if let Some(error) = block_on(worker.device.pop_error_scope()) {
                            let error =
                                format!("GPU terrain disabled after allocation failure: {error}");
                            *state = Err(error.clone());
                            return Err(error);
                        }
                        let (raw, _, _) = computed?;
                        Ok((
                            TileKey {
                                zoom_bucket: spec.zoom_bucket,
                                lat_bucket: lat,
                                lon_bucket: lon,
                            },
                            core,
                            raw,
                        ))
                    })();
                    if res.is_err() {
                        stop_ref.store(true, Ordering::Relaxed);
                        let _ = raw_tx.send(res);
                        break;
                    }
                    if raw_tx.send(res).is_err() {
                        break;
                    }
                }
            });

            // Stage 2: CPU Rayon segment stitching + core clipping + GPKG WKB encoding.
            let cpu_thread = scope.spawn(move || {
                for msg in raw_rx {
                    match msg {
                        Ok((tile, core, raw)) => {
                            if stop_ref.load(Ordering::Relaxed) {
                                break;
                            }
                            let (contours_lines, coastline_lines) =
                                stitch(raw, core.source, core.raster_size);
                            let core_bounds = core.core;
                            let encoded_contours: Vec<(i64, f32, Vec<u8>)> = contours_lines
                                .par_iter()
                                .enumerate()
                                .filter_map(|(fid, line)| {
                                    let blob = tile_archive::contour_clip::clip_points_f32(
                                        &line.points,
                                        core_bounds,
                                    )?;
                                    Some((fid as i64, line.elevation_m, blob))
                                })
                                .collect();
                            let encoded_coastlines: Vec<(i64, Vec<u8>)> = coastline_lines
                                .par_iter()
                                .enumerate()
                                .filter_map(|(fid, pts)| {
                                    let blob = tile_archive::contour_clip::clip_points_f32(
                                        pts,
                                        core_bounds,
                                    )?;
                                    Some((fid as i64, blob))
                                })
                                .collect();
                            if enc_tx
                                .send(Ok((tile, encoded_contours, encoded_coastlines)))
                                .is_err()
                            {
                                break;
                            }
                        }
                        Err(e) => {
                            stop_ref.store(true, Ordering::Relaxed);
                            let _ = enc_tx.send(Err(e));
                            break;
                        }
                    }
                }
            });

            // Stage 3 (main thread): SQLite atomic tile writes + progress reporting.
            let mut err: Option<String> = None;
            for msg in enc_rx {
                match msg {
                    Ok((tile, encoded_contours, encoded_coastlines)) => {
                        if err.is_some() {
                            continue;
                        }
                        if let Err(e) = contours::write_encoded_tile(
                            &mut db,
                            tile,
                            &encoded_contours,
                            Some(&encoded_coastlines),
                        ) {
                            stop.store(true, Ordering::Relaxed);
                            err = Some(e.to_string());
                            continue;
                        }
                        built += 1;
                        progress(contours::ContourBuildProgress::info(
                            "GPU terrain",
                            ((built + skipped) as f64 / total.max(1) as f64) as f32,
                            format!("{built} built, {skipped} cached"),
                        ));
                    }
                    Err(e) => {
                        stop.store(true, Ordering::Relaxed);
                        err.get_or_insert(e);
                    }
                }
            }
            let _ = gpu_thread.join();
            let _ = cpu_thread.join();
            match err {
                Some(e) => Err(e),
                None => Ok(()),
            }
        });
        pipeline_result?;
    }

    db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "GPU contours complete: {built} built, {skipped} cached"
    ))
}
