//! Viewport-driven survey paths and measured raster overlays in both Earth views.
use crate::lidar_catalog::{ReadJob, completed, read_in_background};
use crate::panels::world_map::satellite_layer::SatelliteLayer;
use crate::satellite_image::SatelliteImage;
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, OnceLock,
        mpsc::{self, Receiver},
    },
    time::{Duration, Instant},
};

const MAP: &str = include_str!("../../../scripts/lidar_map.py");
const SURVEYS: &str = include_str!("../../../scripts/lidar_catalog.py");
const DETAIL_ZOOM: u8 = 5;

#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
struct Tile {
    z: u8,
    x: u32,
    y: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum View {
    Coverage,
    Fused,
}

impl Tile {
    fn bounds(self) -> [f32; 4] {
        let step = 180.0 / (1u32 << self.z) as f32;
        let west = -180. + self.x as f32 * step;
        let north = 90. - self.y as f32 * step;
        [west, north - step, west + step, north]
    }
}

fn visible([w, s, e, n]: [f32; 4]) -> Vec<Tile> {
    let (width, height) = ((e - w).min(360.), n - s);
    if ![w, s, e, n].iter().all(|v| v.is_finite()) || width <= 0. || height <= 0. {
        return Vec::new();
    }
    let z = (360. / width.max(height)).log2().ceil().clamp(0., 16.) as u8;
    let rows = 1i32 << z;
    let span = 180. / rows as f32;
    let x0 = ((w + 180.) / span).floor() as i32;
    let x1 = (((w + width + 180.) / span).ceil() as i32 - 1)
        .max(x0)
        .min(x0 + 2 * rows - 1);
    let y0 = (((90. - n) / span).floor() as i32).clamp(0, rows - 1);
    let y1 = (((90. - s) / span).ceil() as i32 - 1).clamp(y0, rows - 1);
    let mut tiles = Vec::new();
    for x in x0..=x1 {
        for y in y0..=y1 {
            tiles.push((
                (2 * x - x0 - x1).pow(2) + (2 * y - y0 - y1).pow(2),
                Tile {
                    z,
                    x: x.rem_euclid(rows * 2) as u32,
                    y: y as u32,
                },
            ));
        }
    }
    tiles.sort_by_key(|(distance, _)| *distance);
    tiles.into_iter().map(|(_, tile)| tile).collect()
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct Key {
    tile: Tile,
    detail: bool,
    variant: String,
}
type MapImage = (SatelliteImage, String);

#[derive(Debug)]
enum TileError {
    Render(String),
}

impl From<String> for TileError {
    fn from(error: String) -> Self {
        Self::Render(error)
    }
}

struct CatalogSnapshot {
    generation: u32,
    sequence: Option<i64>,
    revisions: Vec<(Key, i64)>,
}

struct Job {
    key: Arc<OnceLock<Key>>,
    rx: Receiver<Result<Option<MapImage>, TileError>>,
    // The Python renderer checks this marker between source requests.
    active: Option<tempfile::NamedTempFile>,
}

#[derive(Default)]
pub(crate) struct Tiles {
    view: RefCell<Vec<Tile>>,
    stable: Vec<Tile>,
    moved: Option<Instant>,
    preferred: String,
    measurements: bool,
    generation: u32,
    catalog_job: Option<ReadJob<CatalogSnapshot>>,
    last_catalog_check: Option<Instant>,
    catalog_ready: bool,
    bulk_sequence: i64,
    bulk_versions: HashMap<Key, i64>,
    loaded: HashMap<Key, Option<SatelliteLayer>>,
    order: VecDeque<Key>,
    jobs: [Option<Job>; 2],
    retry: HashMap<Key, Instant>,
    pub(crate) status: String,
    pub(crate) sources: String,
}

fn key(tile: Tile, detail: bool, generation: u32, preferred: &str) -> Key {
    use sha1::{Digest, Sha1};
    let variant = if !detail {
        format!("paths-v2-{generation}")
    } else if preferred.is_empty() {
        "detail-fused-local-v3".into()
    } else {
        format!(
            "detail-fused-local-v3-{:x}",
            Sha1::digest(preferred.as_bytes())
        )
    };
    Key {
        tile,
        detail,
        variant,
    }
}

impl Tiles {
    pub(crate) fn draw(
        &self,
        bounds: Option<[f32; 4]>,
        view: View,
        mut draw: impl FnMut(&SatelliteLayer),
    ) {
        let tiles = bounds.map(visible).unwrap_or_default();
        for detail in [view == View::Fused] {
            let mut shown = Vec::new();
            for &tile in &tiles {
                if detail && (!self.measurements || tile.z < DETAIL_ZOOM) {
                    continue;
                }
                let mut candidate = tile;
                loop {
                    let k = key(candidate, detail, self.generation, &self.preferred);
                    match self.loaded.get(&k) {
                        Some(Some(_)) => {
                            if !shown.contains(&k) {
                                shown.push(k);
                            }
                            break;
                        }
                        Some(None) => break,
                        None if tile.z - candidate.z < 2
                            && candidate.z > if detail { DETAIL_ZOOM } else { 0 } =>
                        {
                            candidate = Tile {
                                z: candidate.z - 1,
                                x: candidate.x / 2,
                                y: candidate.y / 2,
                            };
                        }
                        None => break,
                    }
                }
            }
            shown.sort_by_key(|k| k.tile.z);
            for k in shown {
                if let Some(Some(image)) = self.loaded.get(&k) {
                    draw(image);
                }
            }
        }
        *self.view.borrow_mut() = tiles;
    }

    #[cfg(test)]
    pub(crate) fn cache_test_image(
        &mut self,
        ctx: &egui::Context,
        bounds: Option<[f32; 4]>,
        detail: bool,
        name: &str,
    ) {
        self.measurements = true;
        for tile in bounds
            .map(visible)
            .unwrap_or_else(|| self.view.borrow().clone())
        {
            let k = key(tile, detail, self.generation, "");
            let image = SatelliteImage {
                bounds: tile.bounds(),
                pixels: egui::ColorImage::new(
                    [512, 512],
                    if detail {
                        egui::Color32::RED
                    } else {
                        egui::Color32::CYAN
                    },
                ),
            };
            self.loaded
                .insert(k, Some(SatelliteLayer::new(ctx, name.into(), image)));
        }
    }

    pub(crate) fn stop(&mut self) {
        self.measurements = false;
        for job in self.jobs.iter_mut().flatten() {
            job.active = None;
        }
        self.view.get_mut().clear();
        self.stable.clear();
        self.jobs = [None, None];
    }

    pub(crate) fn poll_catalog(&mut self, ctx: &egui::Context, path: &Path, force: bool) -> bool {
        if force {
            self.last_catalog_check = None;
        }
        if let Some(result) = completed(&mut self.catalog_job) {
            self.catalog_ready = result.is_ok();
            match result {
                Ok(snapshot) => {
                    if self.generation != snapshot.generation {
                        self.generation = snapshot.generation;
                        self.loaded.clear();
                        self.order.clear();
                        self.bulk_versions.clear();
                        self.retry.clear();
                        self.jobs = [None, None];
                    }
                    if let Some(sequence) = snapshot.sequence {
                        self.bulk_sequence = sequence;
                    }
                    for (k, version) in snapshot.revisions {
                        if self.bulk_versions.insert(k.clone(), version).unwrap_or(0) != version {
                            self.loaded.remove(&k);
                            self.order.retain(|old| old != &k);
                            self.retry.remove(&k);
                        }
                    }
                    self.bulk_versions
                        .retain(|k, _| self.stable.contains(&k.tile));
                }
                Err(error) => self.status = error,
            }
        }
        if self.catalog_job.is_none()
            && self
                .last_catalog_check
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(10))
        {
            self.last_catalog_check = Some(Instant::now());
            let path = path.to_owned();
            let previous = self.bulk_sequence;
            let preferred = self.preferred.clone();
            let keys: Vec<_> = self
                .stable
                .iter()
                .map(|&t| key(t, true, self.generation, &preferred))
                .filter(|k| self.loaded.contains_key(k))
                .collect();
            self.catalog_job = Some(read_in_background(ctx, move || {
                catalog_snapshot(&path, previous, &preferred, keys)
            }));
        }
        self.catalog_ready
    }

    /// Each map view requests only its own part of the single lidar layer.
    pub(crate) fn tick(&mut self, ctx: &egui::Context, path: &Path, view: View) {
        let selection = if view == View::Fused { Some("") } else { None };
        let changed = self.measurements != selection.is_some();
        if changed {
            self.jobs = [None, None];
            self.view.get_mut().clear();
            self.stable.clear();
            self.moved = None;
        }
        self.measurements = selection.is_some();
        let preferred = selection.unwrap_or_default();
        self.preferred = preferred.to_owned();
        let view = self.view.get_mut();
        if self.stable != *view {
            self.stable.clone_from(view);
            self.moved = Some(Instant::now());
        }
        for slot in 0..2 {
            if let Some(job) = &mut self.jobs[slot] {
                if let Some(k) = job.key.get()
                    && ((k.detail != self.measurements)
                        || !self.stable.contains(&k.tile)
                        || (k.detail && *k != key(k.tile, true, self.generation, preferred)))
                {
                    job.active = None;
                }

                match job.rx.try_recv() {
                    Ok(Ok(image)) => {
                        let k = job.key.get().expect("completed tile has a key").clone();
                        self.loaded.insert(
                            k.clone(),
                            image.map(|(pixels, name)| SatelliteLayer::new(ctx, name, pixels)),
                        );
                        self.order.push_back(k);
                        while self.order.len() > 100 {
                            if let Some(old) = self.order.pop_front() {
                                self.loaded.remove(&old);
                            }
                        }
                    }
                    Ok(Err(_)) if job.active.is_none() => {}
                    Ok(Err(e)) => {
                        let message = match e {
                            TileError::Render(message) => message,
                        };
                        self.status = message.clone();
                        if let Some(k) = job.key.get() {
                            self.retry.insert(k.clone(), Instant::now());
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => continue,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.status = "Lidar map worker stopped".into()
                    }
                }
                self.jobs[slot] = None;
            }
            if (slot == 1 || self.stable.first().is_some_and(|t| t.z > 3))
                && self
                    .moved
                    .is_some_and(|t| t.elapsed() < Duration::from_millis(350))
            {
                ctx.request_repaint_after(Duration::from_millis(350));
                continue;
            }
            let detail = self.measurements;
            if detail && !self.measurements {
                continue;
            }
            let candidates: Vec<_> = self
                .stable
                .iter()
                .copied()
                .filter(|t| !detail || t.z >= DETAIL_ZOOM)
                .map(|t| key(t, detail, self.generation, preferred))
                .filter(|k| {
                    !self.loaded.contains_key(k)
                        && !self
                            .jobs
                            .iter()
                            .flatten()
                            .any(|job| job.key.get().is_none_or(|active| active == k))
                        && !self
                            .retry
                            .get(k)
                            .is_some_and(|t| t.elapsed() < Duration::from_secs(60))
                })
                .collect();
            if candidates.is_empty() {
                continue;
            }
            let (tx, rx) = mpsc::channel();
            let db = path.to_owned();
            let selected = preferred.to_owned();
            let generation = self.generation;
            if detail {
                self.status = "Loading / preparing lidar…".into();
            }
            let active = match tempfile::NamedTempFile::new() {
                Ok(file) => file,
                Err(e) => {
                    self.status = e.to_string();
                    continue;
                }
            };
            let marker = active.path().to_owned();
            let chosen = Arc::new(OnceLock::new());
            let worker_key = Arc::clone(&chosen);
            match std::thread::Builder::new()
                .name("lidar-map-tile".into())
                .spawn(move || {
                    // Prefer saved maps without probing the external drive in the UI frame.
                    let requested = candidates
                        .iter()
                        .find(|k| tile_path(&db, k).is_file())
                        .unwrap_or(&candidates[0]);
                    let _ = worker_key.set(requested.clone());
                    let result = load(&db, requested, &selected, generation, &marker);
                    let _ = tx.send(result);
                    crate::app::request_repaint();
                }) {
                Ok(_) => {
                    self.jobs[slot] = Some(Job {
                        key: chosen,
                        rx,
                        active: Some(active),
                    })
                }
                Err(e) => self.status = e.to_string(),
            }
        }
        self.retry
            .retain(|_, at| at.elapsed() < Duration::from_secs(60));
        self.sources.clear();
        if !self.measurements {
            if !self
                .retry
                .keys()
                .any(|k| !k.detail && self.stable.contains(&k.tile))
            {
                self.status = "Worldwide survey boundaries".into();
            }
            return;
        }
        let mut blanks = 0;
        for &tile in &self.stable {
            match self
                .loaded
                .get(&key(tile, true, self.generation, preferred))
            {
                Some(Some(image)) => {
                    let name = image.name();
                    if !self.sources.lines().any(|line| line == name) {
                        if !self.sources.is_empty() {
                            self.sources.push('\n');
                        }
                        self.sources.push_str(name);
                    }
                }
                Some(None) => blanks += 1,
                None => {}
            }
        }
        if self.stable.first().is_some_and(|t| t.z < DETAIL_ZOOM) {
            self.status = "Zoom in to see fused lidar".into();
        } else if !self.sources.is_empty() {
            self.status = if self.jobs.iter().any(Option::is_some) {
                "Showing lidar; preparing nearby tiles…".into()
            } else {
                "Showing fused lidar".into()
            };
        } else if blanks > 0 && blanks == self.stable.len() {
            self.status = "No fused lidar prepared here yet".into();
        }
    }
}

fn tile_path(db: &Path, k: &Key) -> PathBuf {
    let Tile { z, x, y } = k.tile;
    db.parent()
        .unwrap_or(Path::new("."))
        .join("maps/v1")
        .join(&k.variant)
        .join(z.to_string())
        .join(x.to_string())
        .join(format!("{y}.png"))
}

fn catalog_snapshot(
    path: &Path,
    previous: i64,
    preferred: &str,
    keys: Vec<Key>,
) -> Result<CatalogSnapshot, String> {
    let db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| e.to_string())?;
    let generation = db
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let mut snapshot = CatalogSnapshot {
        generation,
        sequence: None,
        revisions: Vec::new(),
    };
    let bulk = bulk_index(path);
    let raw_sequence: i64 = bulk
        .as_ref()
        .and_then(|db| {
            db.query_row("SELECT coalesce(max(rowid),0) FROM files", [], |r| r.get(0))
                .ok()
        })
        .unwrap_or(0);
    snapshot.sequence = Some(raw_sequence + fused_revision(path, None, "") * 1_000_000_000);
    if snapshot.sequence.is_some_and(|seq| seq != previous) {
        snapshot.revisions = keys
            .into_iter()
            .map(|k| {
                let version = tile_revision(path, bulk.as_ref(), k.tile, preferred);
                (k, version)
            })
            .collect();
    }
    Ok(snapshot)
}

fn bulk_index(catalog: &Path) -> Option<rusqlite::Connection> {
    rusqlite::Connection::open_with_flags(
        catalog.with_file_name("bulk.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok()
}

fn fused_revision(catalog: &Path, tile: Option<Tile>, preferred: &str) -> i64 {
    let Some(root) = catalog.parent() else {
        return 0;
    };
    let mut revision = 0;
    for (file, table) in [("index.sqlite", "rasters"), ("previews.sqlite", "tiles")] {
        let Ok(db) = rusqlite::Connection::open_with_flags(
            root.join("fused/v1").join(file),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) else {
            continue;
        };
        revision += if let Some(tile) = tile {
            let [w, s, e, n] = tile.bounds();
            db.query_row(
                &format!(
                    "SELECT coalesce(max(id),0) FROM {table} WHERE west<?1 AND east>=?2
            AND south<?3 AND north>=?4 AND (?5='' OR survey=?5 OR
            (substr(?5,-1)='/' AND instr(survey,?5)=1))"
                ),
                rusqlite::params![e, w, n, s, preferred],
                |r| r.get(0),
            )
            .unwrap_or(0)
        } else {
            db.query_row(
                &format!("SELECT coalesce(max(id),0) FROM {table}"),
                [],
                |r| r.get(0),
            )
            .unwrap_or(0)
        };
    }
    revision
}

fn tile_revision(
    catalog: &Path,
    bulk: Option<&rusqlite::Connection>,
    tile: Tile,
    preferred: &str,
) -> i64 {
    bulk.map_or(0, |db| bulk_revision(db, tile, preferred))
        + fused_revision(catalog, Some(tile), preferred) * 1_000_000_000
}

fn bulk_revision(db: &rusqlite::Connection, tile: Tile, preferred: &str) -> i64 {
    let [w, s, e, n] = tile.bounds();
    let args = rusqlite::params![e, w, n, s, preferred];
    let direct: i64 = db
        .query_row(
            "SELECT coalesce(max(rowid),0) FROM files WHERE west<?1
        AND east>=?2 AND south<?3 AND north>=?4 AND (?5='' OR survey=?5 OR
        (substr(?5,-1)='/' AND instr(survey,?5)=1))",
            args,
            |r| r.get(0),
        )
        .unwrap_or(0);
    let archive = db
        .query_row(
            "SELECT coalesce(max(f.rowid),0) FROM transects t JOIN files f ON f.path=t.path
        WHERE t.west<?1 AND t.east>=?2 AND t.south<?3 AND t.north>=?4 AND (?5='' OR t.survey=?5)",
            args,
            |r| r.get(0),
        )
        .unwrap_or(0);
    direct.max(archive)
}

fn load(
    db: &Path,
    k: &Key,
    preferred: &str,
    generation: u32,
    active: &Path,
) -> Result<Option<MapImage>, TileError> {
    let Tile { z, x, y } = k.tile;
    let target = tile_path(db, k);
    let mut output = target.clone();
    let mut revision = if k.detail {
        tile_revision(db, bulk_index(db).as_ref(), k.tile, preferred)
    } else {
        0
    };
    // Older workers cached selected NASA granules as empty before they could read them.
    let empty_prefix = if k.detail
        && preferred.starts_with('G')
        && preferred.chars().nth(1).is_some_and(|c| c.is_ascii_digit())
    {
        "empty-granule"
    } else {
        "empty"
    };
    let empty = target.with_extension(if revision == 0 {
        format!("{empty_prefix}-{generation}")
    } else {
        format!("{empty_prefix}-{generation}-{revision}")
    });
    let stamp = target.with_extension("revision");
    let stale = revision > 0
        && std::fs::read_to_string(&stamp)
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            != Some(revision);
    if (!output.is_file() || stale) && !empty.is_file() {
        if k.detail {
            let span = 180. / (1u32 << z) as f64;
            let w = -180. + x as f64 * span;
            let n = 90. - y as f64 * span;
            let bounds = [w, n - span, w + span, n];
            let mut image = lidar_pipeline::render::render_prepared(db, bounds, Some(active))
                .map_err(TileError::Render)?;
            if image.as_ref().is_none_or(|image| {
                image.rgba.chunks_exact(4).filter(|p| p[3] > 0).count() * 100 < 512 * 512 * 90
            }) {
                // Prepare a numeric overview first. Local still displays only
                // fused rasters; no raw-source renderer or picker is involved.
                let preparation = lidar_pipeline::ept::prepare(db, bounds, z, Some(active));
                revision = tile_revision(db, bulk_index(db).as_ref(), k.tile, preferred);
                image = lidar_pipeline::render::render_prepared(db, bounds, Some(active))
                    .map_err(TileError::Render)?;
                // An unavailable historical source must not hide the newest
                // raster already published while this tile was preparing.
                if let Err(error) = preparation {
                    if image
                        .as_ref()
                        .is_none_or(|image| !image.rgba.chunks_exact(4).any(|p| p[3] > 0))
                    {
                        return Err(TileError::Render(error));
                    }
                    eprintln!("Lidar preparation incomplete: {error}");
                }
            }
            if let Some(image) = image.filter(|image| image.rgba.chunks_exact(4).any(|p| p[3] > 0))
            {
                lidar_pipeline::render::save_png(&target, &image).map_err(TileError::Render)?;
                output = target.clone();
                if revision > 0 {
                    let _ = std::fs::write(&stamp, revision.to_string());
                }
            } else {
                if let Some(parent) = empty.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                std::fs::write(&empty, "").map_err(|e| e.to_string())?;
                return Ok(None);
            }
        } else {
            // The existing catalog-boundary renderer is independent of raw scan loading.
            let tmp = tempfile::tempdir().map_err(|e| e.to_string())?;
            for (name, contents) in [("lidar_map.py", MAP), ("lidar_catalog.py", SURVEYS)] {
                std::fs::write(tmp.path().join(name), contents).map_err(|e| e.to_string())?;
            }
            let args = [
                tmp.path().join("lidar_map.py"),
                db.to_owned(),
                "paths".into(),
                z.to_string().into(),
                x.to_string().into(),
                y.to_string().into(),
                target.clone(),
                "".into(),
                "--empty".into(),
                empty.clone(),
                "--active".into(),
                active.to_owned(),
            ];
            let execute =
                |cmd: &mut Command| super::lidar_catalog::gdal_path(cmd).args(&args).output();
            let mut cmd = Command::new("uv");
            cmd.args([
                "run",
                "--no-project",
                "--with",
                "numpy",
                "--with",
                "pillow",
                "--",
                "python",
            ]);
            let result = match execute(&mut cmd) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    execute(&mut Command::new(if cfg!(windows) {
                        "python"
                    } else {
                        "python3"
                    }))
                }
                other => other,
            }
            .map_err(|e| format!("Cannot start lidar boundary worker: {e}"))?;
            if !result.status.success() {
                return Err(TileError::Render(format!(
                    "Lidar boundaries: {}",
                    String::from_utf8_lossy(&result.stderr).trim()
                )));
            }
        }
    }
    if !output.is_file() && empty.exists() {
        return Ok(None);
    }
    let file = std::fs::File::open(&output).map_err(|e| e.to_string())?;
    let mut png = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .map_err(|e| e.to_string())?;
    let info = png.info();
    if info.width != 512
        || info.height != 512
        || info.color_type != png::ColorType::Rgba
        || info.bit_depth != png::BitDepth::Eight
    {
        return Err(TileError::Render(
            "Invalid cached lidar map dimensions or encoding".into(),
        ));
    }
    let name = info
        .uncompressed_latin1_text
        .iter()
        .find(|chunk| chunk.keyword == "Surveys")
        .and_then(|chunk| serde_json::from_str::<Vec<[String; 3]>>(&chunk.text).ok())
        .map(|surveys| {
            surveys
                .into_iter()
                .map(|[name, _, _]| name)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Lidar".into());
    let mut rgba = vec![0; 512 * 512 * 4];
    png.next_frame(&mut rgba).map_err(|e| e.to_string())?;
    let pixels = egui::ColorImage::from_rgba_unmultiplied([512, 512], &rgba);
    Ok(Some((
        SatelliteImage {
            bounds: k.tile.bounds(),
            pixels,
        },
        name,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires public USGS access and writable ONEKEE_LIDAR_TEST_CATALOG"]
    fn kentucky_public_survey_prepares_and_loads_through_the_desktop_worker() {
        let catalog =
            PathBuf::from(std::env::var_os("ONEKEE_LIDAR_TEST_CATALOG").expect("catalog path"));
        let tile = Tile {
            z: 10,
            x: 522,
            y: 301,
        };
        let active = tempfile::NamedTempFile::new().unwrap();
        let (image, credit) = load(&catalog, &key(tile, true, 4, ""), "", 4, active.path())
            .unwrap()
            .unwrap();
        assert!(credit.contains("KY"), "{credit}");
        assert!(image.pixels.pixels.iter().filter(|p| p.a() > 0).count() > 512 * 512 / 2);
    }
    #[test]
    fn local_without_prepared_or_remote_coverage_stays_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let catalog = tmp.path().join("catalog.sqlite");
        rusqlite::Connection::open(&catalog).unwrap().execute_batch(
            "CREATE TABLE surveys(id TEXT,date TEXT,west REAL,south REAL,east REAL,north REAL);"
        ).unwrap();
        let tile = Tile {
            z: 12,
            x: 2387,
            y: 1175,
        };
        let k = key(tile, true, 0, "");
        let active = tempfile::NamedTempFile::new().unwrap();
        assert!(load(&catalog, &k, "", 0, active.path()).unwrap().is_none());
        assert!(!tile_path(&catalog, &k).is_file());
        assert!(tile_path(&catalog, &k).with_extension("empty-0").is_file());
    }
    #[test]
    fn prepared_view_loads_through_native_reader_and_keeps_source_credit() {
        let tmp = tempfile::tempdir().unwrap();
        let catalog = tmp.path().join("catalog.sqlite");
        let db = rusqlite::Connection::open(&catalog).unwrap();
        db.execute_batch("CREATE TABLE surveys (id TEXT,name TEXT,page TEXT); INSERT INTO surveys VALUES ('scan','Native scan','https://example.org');").unwrap();
        let tile = Tile {
            z: 12,
            x: 2387,
            y: 1175,
        };
        let span = 180. / (1u32 << tile.z) as f64;
        let w = -180. + tile.x as f64 * span;
        let n = 90. - tile.y as f64 * span;
        let folder = tmp.path().join("fused/v1");
        std::fs::create_dir_all(&folder).unwrap();
        let index = rusqlite::Connection::open(folder.join("index.sqlite")).unwrap();
        index.execute_batch("CREATE TABLE rasters (id INTEGER,url TEXT,survey TEXT,date TEXT,path TEXT,west REAL,south REAL,east REAL,north REAL);").unwrap();
        index.execute("INSERT INTO rasters VALUES(1,'https://example.org/scan','scan','2026','fused/v1/scan.tif',?1,?2,?3,?4)",
            rusqlite::params![w,n-span,w+span,n]).unwrap();
        {
            use tiff::{
                encoder::{TiffEncoder, colortype::RGBA32Float},
                tags::Tag,
            };
            let mut encoder =
                TiffEncoder::new(std::fs::File::create(folder.join("scan.tif")).unwrap()).unwrap();
            let mut image = encoder.new_image::<RGBA32Float>(2, 2).unwrap();
            image
                .encoder()
                .write_tag(Tag::ModelPixelScaleTag, &[span / 2., span / 2., 0.][..])
                .unwrap();
            image
                .encoder()
                .write_tag(Tag::ModelTiepointTag, &[0., 0., 0., w, n, 0.][..])
                .unwrap();
            image
                .encoder()
                .write_tag(
                    Tag::GeoKeyDirectoryTag,
                    &[1u16, 1, 0, 1, 2048, 0, 1, 4326][..],
                )
                .unwrap();
            image
                .write_data(&[
                    10f32, 20., 15., 2., 10., 20., 15., 2., 10., 20., 15., 2., 10., 20., 15., 2.,
                ])
                .unwrap();
        }
        let active = tempfile::NamedTempFile::new().unwrap();
        let selected = key(tile, true, 0, "scan");
        let (image, credit) = load(&catalog, &selected, "scan", 0, active.path())
            .unwrap()
            .unwrap();
        assert_eq!(credit, "Native scan");
        assert_eq!(image.pixels[(0, 0)].a(), 255);
        assert!(tile_path(&catalog, &selected).is_file());
    }
    #[test]
    fn automatic_revisit_does_not_reuse_an_arbitrary_selected_survey() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("absent.sqlite");
        let tile = Tile {
            z: 12,
            x: 2387,
            y: 1175,
        };
        let selected = key(tile, true, 1, "NOAAID:38");
        let output = tile_path(&db, &selected);
        std::fs::create_dir_all(output.parent().unwrap()).unwrap();
        let mut image = image::RgbaImage::new(512, 512);
        image.put_pixel(10, 20, image::Rgba([200, 130, 70, 255]));
        image.save(output).unwrap();
        let auto = key(tile, true, 9, "");
        let empty = tile_path(&db, &auto).with_extension("empty-9");
        std::fs::create_dir_all(empty.parent().unwrap()).unwrap();
        std::fs::write(empty, "").unwrap();
        assert!(
            load(&db, &auto, "", 9, &tmp.path().join("no-worker"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn converted_scan_invalidates_only_intersecting_acquisition_tiles() {
        let tmp = tempfile::tempdir().unwrap();
        let catalog = tmp.path().join("catalog.sqlite");
        rusqlite::Connection::open(&catalog).unwrap();
        let root = tmp.path().join("fused/v1");
        std::fs::create_dir_all(&root).unwrap();
        let db = rusqlite::Connection::open(root.join("index.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE rasters (id INTEGER, survey TEXT, west REAL, south REAL, east REAL, north REAL);
            INSERT INTO rasters VALUES (1,'new',-77.61,37.54,-77.60,37.55);").unwrap();
        let tile = Tile {
            z: 15,
            x: 18640,
            y: 9549,
        };
        let before = tile_revision(&catalog, None, tile, "new");
        assert!(before > 0);
        assert_eq!(tile_revision(&catalog, None, tile, "old"), 0);
        assert_eq!(
            tile_revision(
                &catalog,
                None,
                Tile {
                    x: tile.x + 10,
                    ..tile
                },
                "new"
            ),
            0
        );
        db.execute("UPDATE rasters SET id=2", []).unwrap();
        assert_ne!(tile_revision(&catalog, None, tile, "new"), before);
        assert_eq!(fused_revision(&catalog, None, ""), 2);
    }

    #[test]
    fn each_view_draws_only_its_own_kind_even_when_both_are_cached() {
        let ctx = egui::Context::default();
        let bounds = Some([1., 1., 1.02, 1.02]);
        let mut tiles = Tiles::default();
        tiles.cache_test_image(&ctx, bounds, false, "coverage");
        tiles.cache_test_image(&ctx, bounds, true, "fused");
        for (view, expected) in [(View::Coverage, "coverage"), (View::Fused, "fused")] {
            let mut names = Vec::new();
            tiles.draw(bounds, view, |image| names.push(image.name().to_owned()));
            assert!(!names.is_empty());
            assert!(names.iter().all(|name| name == expected));
        }
    }

    #[test]
    fn switching_views_discards_the_previous_viewport_and_stops_its_workers() {
        let ctx = egui::Context::default();
        let tmp = tempfile::tempdir().unwrap();
        let mut tiles = Tiles::default();
        tiles.draw(Some([1., 1., 1.02, 1.02]), View::Coverage, |_| {});
        tiles.tick(&ctx, &tmp.path().join("catalog.sqlite"), View::Fused);
        assert!(tiles.stable.is_empty());
        assert!(tiles.jobs.iter().all(Option::is_none));
        tiles.draw(Some([1., 1., 1.02, 1.02]), View::Fused, |_| {});
        tiles.tick(&ctx, &tmp.path().join("catalog.sqlite"), View::Coverage);
        assert!(tiles.stable.is_empty());
        assert!(tiles.jobs.iter().all(Option::is_none));
    }

    #[test]
    fn covers_small_views_and_wraps_at_date_line() {
        let view = visible([179.98, -0.05, 180.02, 0.05]);
        assert!(view.iter().all(|t| t.z >= DETAIL_ZOOM));
        assert!(view.iter().any(|t| t.x == 0));
        assert!(view.iter().any(|t| t.x == (2u32 << t.z) - 1));
        assert!(visible([-180., -90., 180., 90.]).len() <= 12);
    }
}
