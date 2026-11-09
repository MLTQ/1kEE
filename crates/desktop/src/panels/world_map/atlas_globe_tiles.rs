//! Camera-independent Globe relief. A completed geographic tile is never rebuilt
//! just because the camera pans, zooms, or returns with slightly different bounds.
use super::{atlas_surface::Raster, atlas_viewport::Region, globe_scene::GlobeLayout};
use crate::model::GlobeViewState;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, mpsc},
    time::{Duration, Instant},
};

pub(super) const COLS: usize = 72;
pub(super) const ROWS: usize = 36;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct Key(pub usize, pub usize);
impl Key {
    pub fn bounds(self) -> [f32; 4] {
        let w = self.0 as f32 * 5. - 180.;
        let s = self.1 as f32 * 5. - 90.;
        [w, s, w + 5., s + 5.]
    }
    pub fn region(self) -> Region {
        let [w, s, e, n] = self.bounds().map(|v| (v * 3600.) as i32);
        // Neighboring cores sample identical boundary heights, including the
        // date line. The halo also supplies the lighting derivative at edges.
        Region([w - 15, (s - 15).max(-324000), e + 15, (n + 15).min(324000)])
    }
}

pub(super) fn keys(bounds: [f32; 4]) -> Vec<Key> {
    if !bounds.iter().all(|v| v.is_finite()) || bounds[2] <= bounds[0] || bounds[3] <= bounds[1] {
        return Vec::new();
    }
    let west = ((bounds[0] + 180.) / 5.).floor() as i32;
    let east = (((bounds[2] + 180.) / 5.).ceil() as i32).min(west + COLS as i32);
    let south = (((bounds[1] + 90.) / 5.).floor() as i32).clamp(0, ROWS as i32);
    let north = (((bounds[3] + 90.) / 5.).ceil() as i32).clamp(0, ROWS as i32);
    let mut result = Vec::new();
    for y in south..north {
        for x in west..east {
            result.push(Key(x.rem_euclid(COLS as i32) as usize, y as usize));
        }
    }
    result
}

pub(super) fn selection(rect: egui::Rect, layout: &GlobeLayout, view: &GlobeViewState) -> Vec<Key> {
    let Some(mut bounds) = super::atlas_viewport::globe_bounds(rect, layout, view) else {
        return Vec::new();
    };
    // Small displacement guard, not a new camera-dependent cache key.
    bounds[0] -= 0.05;
    bounds[1] -= 0.05;
    bounds[2] += 0.05;
    bounds[3] += 0.05;
    let mut result = keys(bounds);
    let center = view.globe_center_latlon();
    let distance = |key: &Key| {
        let b = key.bounds();
        let lon = ((b[0] + 2.5 - center.lon + 180.).rem_euclid(360.) - 180.)
            * center.lat.to_radians().cos();
        lon * lon + (b[1] + 2.5 - center.lat).powi(2)
    };
    result.sort_by(|a, b| distance(a).total_cmp(&distance(b)).then(a.cmp(b)));
    result
}

struct Entry {
    raster: Arc<Raster>,
    touched: u64,
}
struct Job {
    key: Key,
    epoch: u64,
    result: mpsc::Receiver<Result<Arc<Raster>, String>>,
}
#[derive(Default)]
pub(super) struct State {
    identity: Option<(Option<PathBuf>, usize)>,
    entries: HashMap<Key, Entry>,
    failed: HashMap<Key, (Instant, String)>,
    job: Option<Job>,
    epoch: u64,
    tick: u64,
    bytes: usize,
}
pub(super) struct Frame {
    pub rasters: Vec<Arc<Raster>>,
    pub wanted: usize,
    pub missing: usize,
    pub error: Option<String>,
}
impl State {
    fn clear(&mut self) {
        self.entries.clear();
        self.failed.clear();
        self.bytes = 0;
        self.epoch = self.epoch.wrapping_add(1);
        // Retain the single worker gate, but discard its old-epoch result.
    }
    fn poll(&mut self) {
        let Some(job) = &self.job else { return };
        let result = match job.result.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("Atlas tile worker stopped".into()),
        };
        let job = self.job.take().unwrap();
        if job.epoch != self.epoch {
            return;
        }
        match result {
            Ok(raster) => {
                self.bytes += raster.heights.len() * 4 + raster.colors.len();
                self.entries.insert(
                    job.key,
                    Entry {
                        raster,
                        touched: self.tick,
                    },
                );
            }
            Err(error) => {
                eprintln!("[atlas] Globe tile {:?}: {error}", job.key);
                self.failed.insert(job.key, (Instant::now(), error));
            }
        }
    }
    #[allow(clippy::too_many_arguments)] // Explicit request, retention policy and injectable source loader.
    pub(super) fn frame(
        &mut self,
        ctx: &egui::Context,
        root: Option<&Path>,
        max_edge: usize,
        wanted: &[Key],
        load_missing: bool,
        keep: bool,
        budget: usize,
        load: impl FnOnce(Key, Option<PathBuf>, usize) -> Result<Arc<Raster>, String> + Send + 'static,
    ) -> Frame {
        let identity = (root.map(Path::to_path_buf), max_edge.min(1202));
        if self.identity.as_ref() != Some(&identity) {
            self.clear();
            self.identity = Some(identity.clone());
        }
        self.poll();
        self.tick = self.tick.wrapping_add(1);
        let visible: HashSet<_> = wanted.iter().copied().collect();
        let mut rasters = Vec::new();
        let mut error = None;
        for key in wanted {
            if let Some(entry) = self.entries.get_mut(key) {
                entry.touched = self.tick;
                rasters.push(entry.raster.clone());
            }
            if let Some((_, message)) = self.failed.get(key) {
                error = Some(message.clone());
            }
        }
        // Stable draw ordering also keeps the rendered-target cache useful.
        rasters.sort_by_key(|r| r.id);
        if !keep {
            while self.bytes > budget {
                let oldest = self
                    .entries
                    .iter()
                    .filter(|(key, _)| !visible.contains(key))
                    .min_by_key(|(_, entry)| entry.touched)
                    .map(|(key, _)| *key);
                let Some(oldest) = oldest else { break };
                let entry = self.entries.remove(&oldest).unwrap();
                self.bytes -= entry.raster.heights.len() * 4 + entry.raster.colors.len();
            }
        }
        if load_missing && self.job.is_none() {
            let next = wanted.iter().find(|key| {
                !self.entries.contains_key(key)
                    && self
                        .failed
                        .get(key)
                        .is_none_or(|(when, _)| when.elapsed() >= Duration::from_secs(30))
            });
            if let Some(&key) = next {
                self.failed.remove(&key);
                let (sender, result) = mpsc::channel();
                let wake = ctx.clone();
                let spawn = std::thread::Builder::new()
                    .name("atlas-globe-tile".into())
                    .spawn(move || {
                        let result = load(key, identity.0, identity.1);
                        let _ = sender.send(result);
                        wake.request_repaint();
                    });
                match spawn {
                    Ok(_) => {
                        self.job = Some(Job {
                            key,
                            epoch: self.epoch,
                            result,
                        })
                    }
                    Err(e) => {
                        self.failed.insert(key, (Instant::now(), e.to_string()));
                    }
                }
            }
        }
        let missing = wanted.len() - rasters.len();
        if load_missing && missing > 0 {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        let count = if load_missing {
            wanted.len()
        } else {
            rasters.len()
        };
        Frame {
            rasters,
            wanted: count,
            missing: if load_missing { missing } else { 0 },
            error,
        }
    }
}
static STATE: OnceLock<Mutex<State>> = OnceLock::new();

pub(super) fn frame(
    ctx: &egui::Context,
    root: Option<&Path>,
    rect: egui::Rect,
    layout: &GlobeLayout,
    view: &GlobeViewState,
) -> Frame {
    let wanted = selection(rect, layout, view);
    let load =
        super::atlas_viewport::globe_visible(rect, layout, view, ctx.pixels_per_point()).is_some();
    let settings = crate::gpu_config::settings();
    STATE.get_or_init(Default::default).lock().unwrap().frame(
        ctx,
        root,
        ctx.input(|i| i.max_texture_side).clamp(16, 8192),
        &wanted,
        load,
        settings.atlas_keep_globe,
        settings.atlas_cpu_bytes,
        |key, root, edge| {
            let detail = super::atlas_source::load_globe(root.as_deref(), key.region(), edge)?;
            detail
                .into_raster_with_core(Some(key.bounds()))
                .ok_or_else(|| "Invalid Atlas tile".into())
        },
    )
}
pub(super) fn clear() {
    if let Some(state) = STATE.get() {
        state.lock().unwrap().clear();
    }
}

#[cfg(test)]
#[path = "atlas_globe_tiles_tests.rs"]
mod tests;
