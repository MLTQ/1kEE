//! Viewport-driven survey paths and measured raster overlays in both Earth views.
use crate::panels::world_map::satellite_layer::SatelliteLayer;
use crate::satellite_image::SatelliteImage;
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::{self, Receiver},
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
struct Job {
    key: Key,
    rx: Receiver<Result<Option<MapImage>, String>>,
    // The Python renderer checks this marker between source requests.
    active: Option<tempfile::NamedTempFile>,
}

#[derive(Default)]
pub(crate) struct Tiles {
    view: RefCell<Vec<Tile>>,
    stable: Vec<Tile>,
    moved: Option<Instant>,
    preferred: String,
    generation: u32,
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
        format!("paths-{generation}")
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
                    (detail && t.z < DETAIL_ZOOM)
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
        for job in self.jobs.iter_mut().flatten() {
            job.active = None;
        }
        self.view.get_mut().clear();
    }

    pub(crate) fn catalog_changed(&mut self, path: &Path) {
        if let Ok(db) =
            rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            && let Ok(generation) = db.query_row("PRAGMA user_version", [], |r| r.get(0))
            && self.generation != generation
        {
            self.generation = generation;
            self.loaded.clear();
            self.order.clear();
            self.retry.clear();
            self.jobs = [None, None];
        }
    }

    pub(crate) fn tick(&mut self, ctx: &egui::Context, path: &Path, preferred: &str) {
        self.preferred = preferred.to_owned();
        let view = self.view.get_mut();
        if self.stable != *view {
            self.stable.clone_from(view);
            self.moved = Some(Instant::now());
        }
        for slot in 0..2 {
            if let Some(job) = &mut self.jobs[slot] {
                if !self.stable.contains(&job.key.tile)
                    || (job.key.detail
                        && job.key != key(job.key.tile, true, self.generation, preferred))
                {
                    job.active = None;
                }
                let k = job.key.clone();
                match job.rx.try_recv() {
                    Ok(Ok(image)) => {
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
                        self.status = e;
                        self.retry.insert(k, Instant::now());
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
            let next = self
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
                .min_by_key(|k| !tile_path(path, k).is_file());
            let Some(k) = next else { continue };
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
            let requested = k.clone();
            match std::thread::Builder::new()
                .name("lidar-map-tile".into())
                .spawn(move || {
                    let result = load(&db, &requested, &selected, generation, &marker);
                    let _ = tx.send(result);
                    crate::app::request_repaint();
                }) {
                Ok(_) => {
                    self.jobs[slot] = Some(Job {
                        key: k,
                        rx,
                        active: Some(active),
                    })
                }
                Err(e) => self.status = e.to_string(),
            }
        }
        self.sources.clear();
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
        self.retry
            .retain(|_, at| at.elapsed() < Duration::from_secs(60));
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

fn load(
    db: &Path,
    k: &Key,
    preferred: &str,
    generation: u32,
    active: &Path,
) -> Result<Option<MapImage>, String> {
    let Tile { z, x, y } = k.tile;
    let mut output = tile_path(db, k);
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
    let empty = output.with_extension(format!("empty-{generation}"));
    if !output.is_file() && !empty.is_file() {
        let tmp = tempfile::tempdir().map_err(|e| e.to_string())?;
        for (name, contents) in [("lidar_map.py", MAP), ("lidar_catalog.py", SURVEYS)] {
            std::fs::write(tmp.path().join(name), contents).map_err(|e| e.to_string())?;
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
            output.clone(),
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
        if !result.status.success() {
            let err = String::from_utf8_lossy(&result.stderr);
            return Err(format!(
                "Lidar tile: {}",
                err.lines()
                    .rev()
                    .find(|l| !l.is_empty())
                    .unwrap_or("render failed")
            ));
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
        return Err("Invalid cached lidar map dimensions or encoding".into());
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
    fn covers_small_views_and_wraps_at_date_line() {
        let view = visible([179.98, -0.05, 180.02, 0.05]);
        assert!(view.iter().all(|t| t.z >= DETAIL_ZOOM));
        assert!(view.iter().any(|t| t.x == 0));
        assert!(view.iter().any(|t| t.x == (2u32 << t.z) - 1));
        assert!(visible([-180., -90., 180., 90.]).len() <= 12);
    }
}
