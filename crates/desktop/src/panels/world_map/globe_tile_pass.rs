//! Persistent spatial batches and bounded uploads for native Earth globe lines.
use super::super::contour_asset::{
    LocalTileGeometry, globe_merge::Frame, globe_residency::Viewport,
};
use super::*;
use tile_archive::contour_grid::Bounds;

#[path = "globe_contour_density.rs"]
mod density;
#[path = "globe_terrain_texture.rs"]
mod texture;

const CHUNK_SEGMENTS: usize = 32_768; // < 1 MiB; small individual allocations
const UPLOAD_BYTES_PER_FRAME: usize = 8 * 1024 * 1024;

pub(super) struct Chunk {
    bounds: Bounds,
    instances: Vec<SegmentInstance>,
    planes: Vec<density::Plane>,
}
struct Tile {
    version: u64,
    _source: Arc<Vec<ContourPath>>, // pins pointer identity
    chunks: Vec<Chunk>,
}
struct Prepared {
    source: Arc<Frame>,
    palette: u64,
    tiles: Vec<Arc<Tile>>,
}
#[derive(Default)]
struct State {
    active: bool,
    busy: bool,
    epoch: u64,
    current: Option<Arc<Prepared>>,
}
static CACHE: OnceLock<Mutex<State>> = OnceLock::new();
fn cache() -> &'static Mutex<State> {
    CACHE.get_or_init(Default::default)
}

pub(super) fn begin_frame() {
    cache().lock().unwrap().active = false;
}
pub(super) fn end_frame() {
    let mut state = cache().lock().unwrap();
    if !state.active && (state.current.is_some() || state.busy) {
        state.current = None;
        state.epoch = state.epoch.wrapping_add(1);
    }
}

fn build_tile(tile: &LocalTileGeometry, major: egui::Color32, minor: egui::Color32) -> Tile {
    let mut bins: HashMap<(i32, i32), Chunk> = HashMap::new();
    for contour in tile.contours.iter() {
        let color = linear_u8(
            if (contour.elevation_m.round() as i32).rem_euclid(50) == 0 {
                major
            } else {
                minor
            },
        );
        let Some(&first) = contour.points.first() else {
            continue;
        };
        let mut a = unit_vec(first);
        for points in contour.points.windows(2) {
            let b = unit_vec(points[1]);
            // Half-degree spatial bins only group segments; no point is moved,
            // dropped or snapped. Bounds below include both original endpoints.
            let cell = (
                (points[0].lon + points[1].lon).floor() as i32,
                (points[0].lat + points[1].lat).floor() as i32,
            );
            let bin = bins.entry(cell).or_insert_with(|| Chunk {
                bounds: Bounds {
                    min_lon: f64::INFINITY,
                    min_lat: f64::INFINITY,
                    max_lon: f64::NEG_INFINITY,
                    max_lat: f64::NEG_INFINITY,
                },
                instances: Vec::new(),
                planes: Vec::new(),
            });
            for p in points {
                bin.bounds.min_lon = bin.bounds.min_lon.min(f64::from(p.lon));
                bin.bounds.max_lon = bin.bounds.max_lon.max(f64::from(p.lon));
                bin.bounds.min_lat = bin.bounds.min_lat.min(f64::from(p.lat));
                bin.bounds.max_lat = bin.bounds.max_lat.max(f64::from(p.lat));
            }
            let elevation = contour.elevation_m.round() as i32;
            let index = bin.instances.len() as u32;
            if let Some(last) = bin.planes.last_mut()
                && last.elevation == elevation
            {
                last.range.end += 1;
            } else {
                bin.planes.push(density::Plane {
                    elevation,
                    range: index..index + 1,
                });
            }
            bin.instances.push(SegmentInstance { a, b, color });
            a = b;
        }
    }
    let mut keys: Vec<_> = bins.keys().copied().collect();
    keys.sort();
    let mut chunks = Vec::new();
    for key in keys {
        let bin = bins.remove(&key).unwrap();
        for (index, part) in bin.instances.chunks(CHUNK_SEGMENTS).enumerate() {
            let start = (index * CHUNK_SEGMENTS) as u32;
            let end = start + part.len() as u32;
            chunks.push(Chunk {
                bounds: bin.bounds,
                instances: part.to_vec(),
                planes: bin
                    .planes
                    .iter()
                    .filter_map(|p| {
                        let lo = p.range.start.max(start);
                        let hi = p.range.end.min(end);
                        (lo < hi).then_some(density::Plane {
                            elevation: p.elevation,
                            range: lo.saturating_sub(start)..hi.saturating_sub(start),
                        })
                    })
                    .collect(),
            });
        }
    }
    Tile {
        version: version_key(&tile.contours, palette_key(major, minor)),
        _source: tile.contours.clone(),
        chunks,
    }
}

fn prepare(
    source: Arc<Frame>,
    previous: Option<&Prepared>,
    major: egui::Color32,
    minor: egui::Color32,
) -> Prepared {
    let palette = palette_key(major, minor);
    let old: HashMap<_, _> = previous
        .into_iter()
        .flat_map(|p| &p.tiles)
        .map(|t| (t.version, t))
        .collect();
    let tiles = source
        .tiles
        .par_iter()
        .map(|tile| {
            let version = version_key(&tile.contours, palette);
            old.get(&version).map_or_else(
                || Arc::new(build_tile(tile, major, minor)),
                |t| (*t).clone(),
            )
        })
        .collect();
    Prepared {
        source,
        palette,
        tiles,
    }
}

fn instances(
    source: &Arc<Frame>,
    major: egui::Color32,
    minor: egui::Color32,
    ctx: &egui::Context,
) -> Option<Arc<Prepared>> {
    let mut state = cache().lock().unwrap();
    state.active = true;
    let palette = palette_key(major, minor);
    if state
        .current
        .as_ref()
        .is_some_and(|p| Arc::ptr_eq(&p.source, source) && p.palette == palette)
    {
        return state.current.clone();
    }
    if !state.busy {
        state.busy = true;
        let epoch = state.epoch;
        let source = source.clone();
        let previous = state.current.clone();
        let wake = ctx.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("globe-tile-instances".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    prepare(source, previous.as_deref(), major, minor)
                }))
                .ok();
                let mut state = cache().lock().unwrap();
                state.busy = false;
                if state.active
                    && state.epoch == epoch
                    && let Some(result) = result
                {
                    state.current = Some(Arc::new(result));
                }
                drop(state);
                wake.request_repaint();
            })
        {
            state.busy = false;
            eprintln!("[1kEE] globe tile instance worker: {error}");
        }
    }
    state.current.clone()
}

#[derive(Default)]
pub(super) struct Gpu {
    pub used: bool,
    tiles: HashMap<u64, Vec<InstanceChunk>>,
    displayed: Option<Arc<Prepared>>,
    pending: Option<Arc<Prepared>>,
    density: density::Selection,
    texture: Option<texture::Cache>,
}

impl Gpu {
    fn update_density(&mut self, view: Viewport) {
        let visible: Vec<_> = self
            .displayed
            .iter()
            .flat_map(|f| &f.tiles)
            .flat_map(|t| &t.chunks)
            .filter(|c| view.intersects(c.bounds))
            .collect();
        self.density.update(|interval| {
            visible
                .iter()
                .map(|c| density::count(&c.planes, interval))
                .sum()
        });
    }

    fn stage(&mut self, device: &wgpu::Device, candidate: &Arc<Prepared>, view: Viewport) -> bool {
        if self.pending.is_none()
            && self
                .displayed
                .as_ref()
                .is_some_and(|p| Arc::ptr_eq(p, candidate))
        {
            return true;
        }
        // Finish one coherent snapshot even if later tiles arrive while it is
        // uploading. Otherwise a fast reader can postpone the first picture.
        let requested = candidate;
        let candidate = self
            .pending
            .get_or_insert_with(|| requested.clone())
            .clone();
        let mut remaining = UPLOAD_BYTES_PER_FRAME;
        // Publish only complete generations: staged fragments cannot double
        // draw the old LOD, expose half a tile, or erase fallback prematurely.
        let mut ordered: Vec<_> = candidate.tiles.iter().collect();
        ordered.sort_by_key(|t| !t.chunks.iter().any(|c| view.intersects(c.bounds)));
        for tile in ordered {
            let uploaded = self.tiles.entry(tile.version).or_default();
            for chunk in &tile.chunks[uploaded.len()..] {
                let bytes = chunk.instances.len() * std::mem::size_of::<SegmentInstance>();
                if bytes > remaining {
                    break;
                }
                uploaded.extend(split_instance_buffers(
                    device,
                    "globe_tile_chunk",
                    &chunk.instances,
                ));
                remaining -= bytes;
            }
        }
        let ready = candidate
            .tiles
            .iter()
            .all(|t| self.tiles[&t.version].len() == t.chunks.len());
        if ready {
            self.displayed = self.pending.take();
        }
        let keep: std::collections::HashSet<_> = candidate
            .tiles
            .iter()
            .chain(self.displayed.iter().flat_map(|p| &p.tiles))
            .map(|t| t.version)
            .collect();
        self.tiles.retain(|version, _| keep.contains(version));
        ready
            && self
                .displayed
                .as_ref()
                .is_some_and(|p| Arc::ptr_eq(p, requested))
    }
}

pub(crate) fn paint(
    painter: &egui::Painter,
    layout: &super::super::globe_scene::GlobeLayout,
    view: &GlobeViewState,
    source: &Arc<Frame>,
    major: egui::Color32,
    minor: egui::Color32,
    alpha: f32,
    width: f32,
) {
    lifecycle::requested(ContourLayer::SrtmGlobe);
    let Some(candidate) = instances(source, major, minor, painter.ctx()) else {
        return;
    };
    let rect = painter.clip_rect();
    let base = ContourCallback::new(
        ContourLayer::SrtmGlobe,
        0,
        Arc::new(vec![]),
        layout,
        view,
        0.020,
        alpha,
        width,
        painter.ctx().pixels_per_point(),
    );
    let mut uniforms = base.uniforms;
    let ppp = uniforms.pixels_per_point;
    uniforms.viewport_min = [rect.min.x * ppp, rect.min.y * ppp];
    uniforms.viewport_size = [
        (rect.width() * ppp).max(1.0),
        (rect.height() * ppp).max(1.0),
    ];
    painter.add(egui_wgpu::Callback::new_paint_callback(
        rect,
        Callback {
            candidate,
            uniforms,
            viewport: Viewport::new(layout, view, rect),
            ctx: painter.ctx().clone(),
        },
    ));
}

struct Callback {
    candidate: Arc<Prepared>,
    uniforms: ContourUniforms,
    viewport: Viewport,
    ctx: egui::Context,
}
impl egui_wgpu::CallbackTrait for Callback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(res) = resources.get_mut::<ContourPassResources>() else {
            return vec![];
        };
        res.terrain.used = true;
        queue.write_buffer(
            &res.uniform_buf,
            ContourLayer::SrtmGlobe.slot() as u64 * UNIFORM_STRIDE,
            bytemuck::bytes_of(&self.uniforms),
        );
        if !res.terrain.stage(device, &self.candidate, self.viewport) {
            self.ctx.request_repaint();
        }
        res.terrain.update_density(self.viewport);
        if let Some(frame) = &res.terrain.displayed {
            let mut image = res
                .terrain
                .texture
                .take()
                .unwrap_or_else(|| texture::Cache::new(device, res.format));
            if image.prepare(
                device,
                encoder,
                res.format,
                &res.pipeline,
                &res.bind_group,
                frame,
                &res.terrain.tiles,
                &self.uniforms,
                self.viewport,
                res.terrain.density.interval_m,
            ) {
                self.ctx.request_repaint();
            }
            res.terrain.texture = Some(image);
        }

        vec![]
    }
    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(res) = resources.get::<ContourPassResources>() else {
            return;
        };
        if let Some(image) = &res.terrain.texture {
            image.paint(pass);
        }
    }
}

#[cfg(test)]
#[path = "globe_tile_pass_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "globe_tile_gpu_tests.rs"]
mod gpu_tests;
