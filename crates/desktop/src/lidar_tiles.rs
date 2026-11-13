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
    AccessDenied(String),
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
    pub(crate) load_error: Option<String>,
    access_denied: bool,
}

fn key(tile: Tile, detail: bool, generation: u32, preferred: &str) -> Key {
    use sha1::{Digest, Sha1};
    let variant = if !detail {
        format!("paths-v2-{generation}")
    } else if preferred.is_empty() {
        "detail-auto".into()
    } else {
        format!("detail-{:x}", Sha1::digest(preferred.as_bytes()))
    };
    Key {
        tile,
        detail,
        variant,
    }
}

impl Tiles {
    pub(crate) fn draw(&self, bounds: Option<[f32; 4]>, mut draw: impl FnMut(&SatelliteLayer)) {
        let tiles = bounds.map(visible).unwrap_or_default();
        for detail in [true, false] {
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
    pub(crate) fn visible_tiles_loaded(&self) -> bool {
        !self.stable.is_empty()
            && self.stable.iter().all(|&t| {
                [false, true].into_iter().all(|detail| {
                    (detail && (!self.measurements || t.z < DETAIL_ZOOM))
                        || self.loaded.contains_key(&key(
                            t,
                            detail,
                            self.generation,
                            &self.preferred,
                        ))
                })
            })
    }

    pub(crate) fn stop(&mut self) {
        self.measurements = false;
        for job in self.jobs.iter_mut().flatten() {
            job.active = None;
        }
        self.view.get_mut().clear();
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

    pub(super) fn retry_selection(&mut self) {
        self.access_denied = false;
        self.retry.retain(|k, _| !k.detail);
        self.load_error = None;
    }

    pub(crate) fn at_overview(&self) -> bool {
        self.view
            .borrow()
            .first()
            .is_some_and(|t| t.z < DETAIL_ZOOM)
    }

    /// None browses outlines; an empty ID enables the automatic mosaic.
    pub(crate) fn tick(&mut self, ctx: &egui::Context, path: &Path, selection: Option<&str>) {
        self.measurements = selection.is_some();
        let preferred = selection.unwrap_or_default();
        if self.preferred != preferred || selection.is_none() {
            self.access_denied = false;
            self.load_error = None;
        }
        self.preferred = preferred.to_owned();
        let view = self.view.get_mut();
        if self.stable != *view {
            self.stable.clone_from(view);
            self.moved = Some(Instant::now());
        }
        for slot in 0..2 {
            if let Some(job) = &mut self.jobs[slot] {
                if let Some(k) = job.key.get()
                    && ((k.detail && !self.measurements)
                        || !self.stable.contains(&k.tile)
                        || (k.detail && *k != key(k.tile, true, self.generation, preferred)))
                {
                    job.active = None;
                }

                match job.rx.try_recv() {
                    Ok(Ok(image)) => {
                        let k = job.key.get().expect("completed tile has a key").clone();
                        if k.detail
                            && k.variant == key(k.tile, true, self.generation, preferred).variant
                            && image.is_some()
                        {
                            self.load_error = None;
                        }
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
                        let denied = matches!(&e, TileError::AccessDenied(_));
                        let message = match e {
                            TileError::Render(message) | TileError::AccessDenied(message) => {
                                message
                            }
                        };
                        self.status = message.clone();
                        if let Some(k) = job.key.get() {
                            if k.detail && !preferred.is_empty() {
                                self.access_denied = denied;
                                self.load_error = Some(message);
                            }
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
            let detail = slot == 1;
            if detail && (!self.measurements || self.access_denied) {
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
                self.status = "Loading lidar measurements…".into();
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
                self.status = "Right-click a survey boundary to load lidar".into();
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
            self.status = "Survey paths · zoom in for detail".into();
        } else if !self.sources.is_empty() {
            self.status = "Showing saved lidar measurements".into();
        } else if blanks > 0 && blanks == self.stable.len() {
            self.status = "Coverage only · no measured image here".into();
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
    if let Some(bulk) = bulk_index(path) {
        snapshot.sequence = bulk
            .query_row("SELECT coalesce(max(rowid),0) FROM files", [], |r| r.get(0))
            .ok();
        if snapshot.sequence.is_some_and(|seq| seq != previous) {
            snapshot.revisions = keys
                .into_iter()
                .map(|k| {
                    let version = bulk_revision(&bulk, k.tile, preferred);
                    (k, version)
                })
                .collect();
        }
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
    let revision = if k.detail {
        bulk_index(db).map_or(0, |b| bulk_revision(&b, k.tile, preferred))
    } else {
        0
    };
    // A selected acquisition is also a valid offline map in automatic mode.
    // Look it up across the saved variants without copying data on external drives.
    if k.detail && preferred.is_empty() && !output.is_file() {
        let root = db.parent().unwrap_or(Path::new(".")).join("maps/v1");
        if let Ok(variants) = std::fs::read_dir(root) {
            let relative = PathBuf::from(z.to_string())
                .join(x.to_string())
                .join(format!("{y}.png"));
            if let Some((_, cached)) = variants
                .flatten()
                .filter(|d| d.file_name().to_string_lossy().starts_with("detail-"))
                .filter_map(|d| {
                    let path = d.path().join(&relative);
                    Some((path.metadata().ok()?.modified().ok()?, path))
                })
                .max()
            {
                output = cached;
            }
        }
    }
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
        let tmp = tempfile::tempdir().map_err(|e| e.to_string())?;
        for (name, contents) in [("lidar_map.py", MAP), ("lidar_catalog.py", SURVEYS)] {
            std::fs::write(tmp.path().join(name), contents).map_err(|e| e.to_string())?;
        }
        if k.detail {
            std::fs::write(
                tmp.path().join("lidar_regions.py"),
                include_bytes!("../../../scripts/lidar_regions.py"),
            )
            .map_err(|e| e.to_string())?;
            std::fs::write(
                tmp.path().join("ne_110m_countries.json"),
                include_bytes!("../../cache_builder/assets/ne_110m_countries.json"),
            )
            .map_err(|e| e.to_string())?;
        }
        let mut cmd = Command::new("uv");
        cmd.args([
            "run",
            "--no-project",
            "--with",
            "laspy[lazrs]==2.7.0",
            "--with",
            "pyproj",
            "--with",
            "requests",
            "--with",
            "pillow",
            "--with",
            "rasterio",
            "--",
            "python",
        ]);
        let python = if cfg!(windows) { "python" } else { "python3" };
        let args = [
            tmp.path().join("lidar_map.py"),
            db.to_owned(),
            PathBuf::from(if k.detail { "detail" } else { "paths" }),
            z.to_string().into(),
            x.to_string().into(),
            y.to_string().into(),
            target.clone(),
            preferred.into(),
            "--empty".into(),
            empty.clone(),
            "--active".into(),
            active.to_owned(),
        ];
        let execute = |c: &mut Command| super::lidar_catalog::gdal_path(c).args(&args).output();
        let result = match execute(&mut cmd) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                execute(&mut Command::new(python))
            }
            other => other,
        }
        .map_err(|e| format!("Cannot start lidar map worker: {e}"))?;
        if result.status.success() && target.is_file() && !empty.is_file() {
            output = target;
            if revision > 0 {
                let _ = std::fs::write(stamp, revision.to_string());
            }
        } else if !result.status.success() && !output.is_file() {
            let err = String::from_utf8_lossy(&result.stderr);
            let message = err
                .lines()
                .rev()
                .find(|l| !l.is_empty())
                .unwrap_or("render failed");
            if result.status.code() == Some(77) {
                return Err(TileError::AccessDenied(message.to_owned()));
            }
            return Err(TileError::Render(format!("Lidar tile: {message}")));
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
    fn saved_selected_map_is_available_on_automatic_offline_revisit() {
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
        let (reopened, _) = load(&db, &auto, "", 9, &tmp.path().join("no-worker"))
            .unwrap()
            .unwrap();
        assert_eq!(reopened.bounds, tile.bounds());
        assert_eq!(
            reopened.pixels[(10, 20)],
            egui::Color32::from_rgb(200, 130, 70)
        );
        assert_eq!(reopened.pixels[(0, 0)], egui::Color32::TRANSPARENT);
    }

    #[test]
    fn boundaries_only_shows_paths_at_detail_zoom_and_does_not_request_measurements() {
        let ctx = egui::Context::default();
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("catalog.sqlite");
        let bounds = Some([10., 10., 11., 11.]);
        let mut tiles = Tiles::default();
        tiles.draw(bounds, |_| {});
        tiles.stable = tiles.view.borrow().clone();
        for &tile in &tiles.stable {
            for (detail, name) in [(false, "outline"), (true, "1998")] {
                let image = SatelliteImage {
                    bounds: tile.bounds(),
                    pixels: egui::ColorImage::new([1, 1], egui::Color32::WHITE),
                };
                tiles.loaded.insert(
                    key(tile, detail, 0, "flight"),
                    Some(SatelliteLayer::new(&ctx, name.into(), image)),
                );
            }
        }
        let painted = |tiles: &Tiles| {
            let mut names = Vec::new();
            tiles.draw(bounds, |image| names.push(image.name().to_owned()));
            names.dedup();
            names
        };
        tiles.tick(&ctx, &db, None);
        assert!(tiles.jobs.iter().all(Option::is_none));
        assert!(tiles.visible_tiles_loaded());
        assert_eq!(painted(&tiles), ["outline"]);
        tiles.tick(&ctx, &db, Some("flight"));
        assert_eq!(painted(&tiles), ["1998", "outline"]);
        tiles.tick(&ctx, &db, None);
        assert_eq!(painted(&tiles), ["outline"]);
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
