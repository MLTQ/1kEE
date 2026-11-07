//! Opt-in SRTM GL1 compute backend. Sources are read-only; callers own cache writes.
//! One serialized device and an twelve-tile VRAM LRU are shared by background jobs.
use crate::contours::{self, TileKey};
use crate::marching_squares::{ContourLine, SrtmTile, chain_segments};
use eframe::wgpu::{self, util::DeviceExt};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::Instant,
};
use tile_archive::contour_grid::{Bounds, CoreTile};

const NODATA: f32 = -32768.0;
const MAX_N: u32 = 6000;
const CAPACITY: u32 = 4_000_000;
type Lines = (Vec<ContourLine>, Vec<Vec<(f32, f32)>>);

/// Enable explicitly with ONEKEE_TERRAIN_GPU=1. GDAL remains the default.
pub fn enabled() -> bool {
    std::env::var("ONEKEE_TERRAIN_GPU").is_ok_and(|v| v == "1")
}

/// Generate a complete tile in memory. Does not modify any files. An error must
/// fall back or abort; it must never be committed as an empty successful tile.
pub fn build(root: &Path, zoom: i32, lat: i32, lon: i32) -> Result<TerrainTile, String> {
    let spec = contours::all_specs()
        .into_iter()
        .find(|s| s.zoom_bucket == zoom)
        .ok_or("GPU terrain supports SRTM zoom buckets 0–6")?;
    let core = CoreTile::srtm(spec.half_extent_deg, lat, lon);
    static WORKER: OnceLock<Mutex<Result<Worker, String>>> = OnceLock::new();
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
    let computed = (|| {
        let grid = worker.resample(root, core.source, core.raster_size)?;
        let source_ms = start.elapsed().as_secs_f64() * 1000.0;
        let start_contour = Instant::now();
        let raw = worker.extract(&grid, core.raster_size, spec.interval_m as f32, CAPACITY)?;
        Ok::<_, String>((
            raw,
            source_ms,
            start_contour.elapsed().as_secs_f64() * 1000.0,
        ))
    })();
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
        "[terrain-gpu] tile={zoom}/{lat}/{lon} source_ms={source_ms:.3} compute_readback_ms={compute_ms:.3} stitch_ms={:.3} total_ms={:.3} segments={segment_count}",
        start_stitch.elapsed().as_secs_f64() * 1000.0,
        start.elapsed().as_secs_f64() * 1000.0
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
    buffer: wgpu::Buffer,
}
struct Worker {
    device: wgpu::Device,
    queue: wgpu::Queue,
    sample_pipeline: wgpu::ComputePipeline,
    contour_pipeline: wgpu::ComputePipeline,
    sources: Vec<Source>,
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
        let adapter = instance
            .enumerate_adapters(wgpu::Backends::PRIMARY)
            .into_iter()
            .filter(|a| {
                requested
                    .as_ref()
                    .is_none_or(|s| a.get_info().name.to_lowercase().contains(s))
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
        let sample_pipeline = make(include_str!("resample.wgsl"));
        let contour_pipeline = make(include_str!("contours.wgsl"));
        if let Some(e) = block_on(device.pop_error_scope()) {
            return Err(e.to_string());
        }
        eprintln!(
            "[terrain-gpu] adapter={} backend={:?} source_cache=12 tiles",
            adapter.get_info().name,
            adapter.get_info().backend
        );
        Ok(Self {
            device,
            queue,
            sample_pipeline,
            contour_pipeline,
            sources: Vec::new(),
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
    fn dispatch(&self, pipeline: &wgpu::ComputePipeline, buffers: &[&wgpu::Buffer], n: u32) {
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
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(n.div_ceil(16), n.div_ceil(16), 1);
        }
        self.queue.submit(Some(encoder.finish()));
    }
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
        staging.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let result = staging.slice(..).get_mapped_range().to_vec();
        staging.unmap();
        Ok(result)
    }
    fn source(&mut self, path: &Path, lat: i32, lon: i32) -> Result<usize, String> {
        let meta = path
            .metadata()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let modified = meta.modified().ok();
        if let Some(i) = self
            .sources
            .iter()
            .position(|s| s.path == path && s.modified == modified && s.size == meta.len())
        {
            let entry = self.sources.remove(i);
            self.sources.push(entry);
            return Ok(self.sources.len() - 1);
        }
        validate_source(path, lat, lon)?;
        let mut tile =
            SrtmTile::load(path).ok_or_else(|| format!("Unreadable SRTM {}", path.display()))?;
        if tile.width != 3601 || tile.height != 3601 {
            return Err("GPU terrain requires 3601×3601 GL1 tiles".into());
        }
        for v in &mut tile.samples {
            if !v.is_finite() {
                *v = NODATA;
            } else if *v < -12000.0 || *v > 10000.0 {
                return Err("Unexpected SRTM elevation range".into());
            }
        }
        let buffer = self.buffer(
            "resident SRTM",
            bytemuck::cast_slice(&tile.samples),
            wgpu::BufferUsages::STORAGE,
        );
        self.sources.retain(|s| s.path != path);
        if self.sources.len() >= 12 {
            self.sources.remove(0);
        }
        self.sources.push(Source {
            path: path.into(),
            modified,
            size: meta.len(),
            buffer,
        });
        Ok(self.sources.len() - 1)
    }
    fn resample(&mut self, root: &Path, bounds: Bounds, n: u32) -> Result<wgpu::Buffer, String> {
        validate_grid(bounds, n)?;
        let initial = vec![NODATA; n as usize * n as usize];
        let grid = self.buffer(
            "sampled terrain",
            bytemuck::cast_slice(&initial),
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let params = self.buffer(
            "sampling parameters",
            bytemuck::cast_slice(&[n, 3601, 3601, 0]),
            wgpu::BufferUsages::UNIFORM,
        );
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
                let i = self.source(&path, lat, lon)?;
                found = true;
                let axes = axes(bounds, n, lat, lon);
                let axes = self.buffer(
                    "local pixel coordinates",
                    bytemuck::cast_slice(&axes),
                    wgpu::BufferUsages::STORAGE,
                );
                self.dispatch(
                    &self.sample_pipeline,
                    &[&params, &self.sources[i].buffer, &axes, &grid],
                    n,
                );
            }
        }
        if !found {
            return Err("No SRTM sources for GPU tile".into());
        }
        Ok(grid)
    }
    fn extract(
        &self,
        grid: &wgpu::Buffer,
        n: u32,
        interval: f32,
        capacity: u32,
    ) -> Result<Vec<[f32; 5]>, String> {
        if !(2..=MAX_N).contains(&n)
            || !interval.is_finite()
            || interval < 1.0
            || capacity == 0
            || capacity > CAPACITY
        {
            return Err("Invalid GPU contour dimensions or interval".into());
        }
        let params = self.buffer(
            "contour parameters",
            bytemuck::cast_slice(&[n, capacity, interval.to_bits(), 0]),
            wgpu::BufferUsages::UNIFORM,
        );
        let segments = self.empty(
            "contour segments",
            u64::from(capacity) * 20,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let count = self.buffer(
            "segment count",
            bytemuck::bytes_of(&0u32),
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        self.dispatch(
            &self.contour_pipeline,
            &[&params, grid, &segments, &count],
            n,
        );
        let count_bytes = self.read(&count, 4)?;
        let count = u32::from_ne_bytes(count_bytes.try_into().unwrap());
        if count > capacity {
            return Err(format!(
                "GPU contour capacity exceeded ({count}>{capacity}); no partial output accepted"
            ));
        }
        if count == 0 {
            return Ok(Vec::new());
        }
        let bytes = self.read(&segments, u64::from(count) * 20)?;
        Ok(bytes
            .as_chunks::<20>()
            .0
            .iter()
            .map(|b| {
                std::array::from_fn(|i| f32::from_ne_bytes(b[i * 4..i * 4 + 4].try_into().unwrap()))
            })
            .collect())
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
fn validate_source(path: &Path, lat: i32, lon: i32) -> Result<(), String> {
    use tiff::{decoder::Decoder, tags::Tag};
    let check = || -> Result<bool, Box<dyn std::error::Error>> {
        let mut d = Decoder::new(std::fs::File::open(path)?)?;
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
                .is_some_and(|k| k.as_chunks::<4>().0.iter().any(|k| *k == [id, 0, 1, value]))
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
fn stitch(mut raw: Vec<[f32; 5]>, b: Bounds, n: u32) -> Lines {
    // Atomic output order varies; sort before stitching for repeatable caches.
    raw.sort_unstable_by(|a, b| {
        a[4].total_cmp(&b[4])
            .then(a[1].total_cmp(&b[1]))
            .then(a[0].total_cmp(&b[0]))
            .then(a[3].total_cmp(&b[3]))
            .then(a[2].total_cmp(&b[2]))
    });
    let mut levels: BTreeMap<i32, Vec<[(f32, f32); 2]>> = BTreeMap::new();
    let coord = |x: f32, y: f32| {
        (
            (b.min_lon + (f64::from(x) + 0.5) * (b.max_lon - b.min_lon) / f64::from(n)) as f32,
            (b.max_lat - (f64::from(y) + 0.5) * (b.max_lat - b.min_lat) / f64::from(n)) as f32,
        )
    };
    for [x, y, z, w, h] in raw {
        levels
            .entry(h as i32)
            .or_default()
            .push([coord(x, y), coord(z, w)]);
    }
    let mut contours = Vec::new();
    let mut coastlines = Vec::new();
    for (h, segs) in levels {
        let lines = chain_segments(segs);
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
    let mut built = 0;
    let mut skipped = 0;
    let specs: Vec<_> = contours::all_specs()
        .into_iter()
        .filter(|s| command.zoom_buckets.contains(&s.zoom_bucket))
        .collect();
    let count = |low, high, step| {
        let range = contours::bucket_range(low, high, step);
        (i64::from(*range.end()) - i64::from(*range.start()) + 1).max(0) as u64
    };
    let total: u64 = specs
        .iter()
        .map(|s| {
            let step = s.half_extent_deg * 0.45;
            count(command.min_lat, command.max_lat, step).saturating_mul(count(
                command.min_lon,
                command.max_lon,
                step,
            ))
        })
        .fold(0, u64::saturating_add);
    for spec in specs {
        let step = spec.half_extent_deg * 0.45;
        for lat in contours::bucket_range(command.min_lat, command.max_lat, step) {
            for lon in contours::bucket_range(command.min_lon, command.max_lon, step) {
                let present: bool = db
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM contour_tile_manifest
                     WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3)",
                        rusqlite::params![spec.zoom_bucket, lat, lon],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                if present {
                    skipped += 1;
                    continue;
                }
                let tile = build(&command.srtm_root, spec.zoom_bucket, lat, lon)?;
                tile.write(&mut db).map_err(|e| e.to_string())?;
                built += 1;
                progress(contours::ContourBuildProgress::info(
                    "GPU terrain",
                    ((built + skipped) as f64 / total.max(1) as f64) as f32,
                    format!("{built} built, {skipped} cached"),
                ));
            }
        }
    }
    db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "GPU contours complete: {built} built, {skipped} cached"
    ))
}
