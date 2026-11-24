//! Bounded, offline GDAL windows from original elevation files.
use super::{atlas_image::HeightGrid, atlas_viewport::Region, srtm_focus_cache::gdal};
use crate::{model::GeoPoint, terrain_assets, threedep};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

#[path = "atlas_view_cache.rs"]
mod view_cache;

pub(super) struct Detail {
    pub grid: HeightGrid,
    pub region: Region,
    pub source: &'static str,
    pub complete: bool,
    save: Option<view_cache::Ticket>,
    colors: Option<Vec<u8>>,
}
impl Detail {
    pub(super) fn into_raster(self) -> Option<std::sync::Arc<super::atlas_surface::Raster>> {
        self.into_raster_with_core(None)
    }
    pub(super) fn into_raster_with_core(
        self,
        core: Option<[f32; 4]>,
    ) -> Option<std::sync::Arc<super::atlas_surface::Raster>> {
        let mut raster = match self.colors {
            Some(colors) => {
                super::atlas_surface::Raster::prepared(self.grid, self.region.bounds(), colors)
            }
            None => super::atlas_surface::Raster::new(self.grid, self.region.bounds()),
        }?;
        std::sync::Arc::get_mut(&mut raster)?.core = core;
        if let Some(ticket) = self.save {
            view_cache::persist(ticket, raster.clone());
        }
        Some(raster)
    }
}

pub(super) fn load(root: Option<&Path>, region: Region, max_edge: usize) -> Result<Detail, String> {
    load_with_mode(root, region, max_edge, false)
}

/// Globe tiles have a fixed native GEBCO resolution, independent of camera zoom.
pub(super) fn load_globe(
    root: Option<&Path>,
    region: Region,
    max_edge: usize,
) -> Result<Detail, String> {
    load_with_mode(root, region, max_edge, true)
}

fn load_with_mode(
    root: Option<&Path>,
    region: Region,
    max_edge: usize,
    terrain_only: bool,
) -> Result<Detail, String> {
    let span = region.span();
    let bounds = region.bounds();
    let srtm = if !terrain_only && span[0] <= 8.0 && span[1] <= 8.0 {
        terrain_assets::find_srtm_root(root)
            .map(|root| srtm_files(&root, bounds))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let fine = if !terrain_only && span[0] <= 0.15 && span[1] <= 0.15 {
        terrain_assets::find_derived_root(root)
            .map(|root| fine_files(&root, bounds))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let mut cache_error = None;
    if fine.is_empty() {
        match cached(root, region, max_edge, !srtm.is_empty(), terrain_only) {
            Ok(Some(detail)) => return Ok(detail),
            Ok(None) => {}
            Err(error) => cache_error = Some(error),
        }
    }
    let base = terrain_assets::find_data_root(root)
        .map(|data| gdal::find_gebco_topography_tiles(&data))
        .unwrap_or_default();
    if base.is_empty() {
        return Err(cache_error.unwrap_or_else(|| "Atlas cache does not cover this view and original GEBCO terrain files were not found. Build Atlas tiles or configure the terrain data folder.".into()));
    }
    let (spacing, source) = if !fine.is_empty() {
        (1.0 / 30.0, "cached fine elevation")
    } else if !srtm.is_empty() {
        (1.0, "SRTM · 1 arcsecond land")
    } else {
        (15.0, "GEBCO · 15 arcsecond terrain")
    };
    let size = region.size(spacing, max_edge);
    let ticket = terrain_assets::find_derived_root(root).and_then(|derived| {
        view_cache::Ticket::new(
            &derived,
            region,
            size,
            base.iter().chain(&srtm).chain(&fine).cloned().collect(),
        )
        .ok()
    });
    if let Some(ticket) = &ticket {
        match ticket.read() {
            Ok(Some(saved)) => {
                let upgrade = saved.colors.is_none();
                return Ok(Detail {
                    grid: saved.grid,
                    colors: saved.colors,
                    region,
                    source,
                    complete: true,
                    save: upgrade.then(|| ticket.clone()),
                });
            }
            Ok(None) => {}
            Err(error) => eprintln!("[atlas] recent-view read: {error}"),
        }
    }
    let scratch =
        tempfile::tempdir().map_err(|e| format!("Cannot create Atlas working folder: {e}"))?;
    let metres = warp(&base, bounds, size, scratch.path(), "ocean")?;
    let mut grid = HeightGrid {
        width: size[0],
        height: size[1],
        metres,
    };
    let mut complete = true;
    for (name, files) in [("land", srtm), ("fine", fine)] {
        if !files.is_empty() {
            match warp(&files, bounds, size, scratch.path(), name) {
                Ok(land) => merge_land(&mut grid.metres, &land),
                Err(_) => complete = false, // Never persist a degraded fallback as finished detail.
            }
        }
    }
    Ok(Detail {
        grid,
        region,
        source,
        complete,
        save: if complete { ticket } else { None },
        colors: None,
    })
}

fn cached(
    root: Option<&Path>,
    region: Region,
    max_edge: usize,
    has_srtm: bool,
    terrain_only: bool,
) -> Result<Option<Detail>, String> {
    let Some(derived) = terrain_assets::find_derived_root(root) else {
        return Ok(None);
    };
    cached_file_mode(
        &derived.join("terrain").join(tile_archive::atlas::FILE_NAME),
        region,
        max_edge,
        has_srtm,
        terrain_only,
    )
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct ReaderKey {
    path: PathBuf,
    region: Option<tile_archive::atlas::regions::Region>,
}

type FileStamp = (u64, Option<std::time::SystemTime>, u64, Option<std::time::SystemTime>);

fn file_stamp(path: &Path) -> Option<FileStamp> {
    let meta = std::fs::metadata(path).ok()?;
    let mut wal_os = path.as_os_str().to_os_string();
    wal_os.push("-wal");
    let wal_meta = std::fs::metadata(Path::new(&wal_os)).ok();
    Some((
        meta.len(),
        meta.modified().ok(),
        wal_meta.as_ref().map_or(0, |m| m.len()),
        wal_meta.and_then(|m| m.modified().ok()),
    ))
}

struct PooledReader {
    key: ReaderKey,
    stamp: FileStamp,
    reader: tile_archive::atlas::Reader,
}

static ATLAS_READER_POOL: std::sync::OnceLock<std::sync::Mutex<Vec<PooledReader>>> =
    std::sync::OnceLock::new();

struct ReaderLease {
    key: ReaderKey,
    stamp: FileStamp,
    reader: Option<tile_archive::atlas::Reader>,
}

impl ReaderLease {
    fn open(path: &Path) -> Result<Self, String> {
        let key = ReaderKey {
            path: path.to_path_buf(),
            region: None,
        };
        let stamp = file_stamp(path).ok_or_else(|| format!("Cannot stat {}", path.display()))?;
        if let Some(reader) = Self::take(&key, stamp) {
            return Ok(Self {
                key,
                stamp,
                reader: Some(reader),
            });
        }
        let reader = tile_archive::atlas::Reader::open(path)?;
        Ok(Self {
            key,
            stamp,
            reader: Some(reader),
        })
    }

    fn open_region(
        directory: &Path,
        region: tile_archive::atlas::regions::Region,
    ) -> Result<Self, String> {
        let file = region.path(directory);
        let key = ReaderKey {
            path: directory.to_path_buf(),
            region: Some(region),
        };
        let stamp = file_stamp(&file).ok_or_else(|| format!("Cannot stat {}", file.display()))?;
        if let Some(reader) = Self::take(&key, stamp) {
            return Ok(Self {
                key,
                stamp,
                reader: Some(reader),
            });
        }
        let reader = tile_archive::atlas::Reader::open_region(directory, region)?;
        Ok(Self {
            key,
            stamp,
            reader: Some(reader),
        })
    }

    fn take(key: &ReaderKey, stamp: FileStamp) -> Option<tile_archive::atlas::Reader> {
        let pool = ATLAS_READER_POOL.get_or_init(|| std::sync::Mutex::new(Vec::new()));
        let mut guard = pool.lock().ok()?;
        let idx = guard.iter().position(|e| e.key == *key)?;
        let entry = guard.swap_remove(idx);
        if entry.stamp == stamp {
            Some(entry.reader)
        } else {
            None
        }
    }

    fn reader_mut(&mut self) -> &mut tile_archive::atlas::Reader {
        self.reader.as_mut().expect("reader leased")
    }
}

impl Drop for ReaderLease {
    fn drop(&mut self) {
        let Some(reader) = self.reader.take() else {
            return;
        };
        let pool = ATLAS_READER_POOL.get_or_init(|| std::sync::Mutex::new(Vec::new()));
        if let Ok(mut guard) = pool.lock() {
            if guard.len() >= 12 {
                guard.remove(0);
            }
            guard.push(PooledReader {
                key: self.key.clone(),
                stamp: self.stamp,
                reader,
            });
        }
    }
}

fn cached_file_mode(
    path: &Path,
    region: Region,
    max_edge: usize,
    has_srtm: bool,
    terrain_only: bool,
) -> Result<Option<Detail>, String> {
    use tile_archive::atlas::Tier;
    let directory = path.with_file_name("regions");
    let [w, s, e, n] = region.bounds().map(f64::from);
    let area = tile_archive::atlas::regions::Region::at((w + e) * 0.5, (s + n) * 0.5);
    let mut error = None;
    let mut main = if path.is_file() {
        ReaderLease::open(path).map_err(|e| error = Some(e)).ok()
    } else {
        None
    };
    let span = region.span();
    if !terrain_only && span[0] <= 8. && span[1] <= 8. {
        if let Some(cache) = &mut main {
            let reader = cache.reader_mut();
            match reader.has_tier(Tier::Land).and_then(|has| {
                if has {
                    sampled(reader, region, max_edge, Tier::Land)
                } else {
                    Ok(None)
                }
            }) {
                Ok(Some(detail)) => return Ok(Some(detail)),
                Ok(None) => {}
                Err(e) => error = Some(e),
            }
        }
        if has_srtm {
            return error.map_or(Ok(None), Err);
        }
    }
    if area.path(&directory).is_file() {
        match ReaderLease::open_region(&directory, area)
            .and_then(|mut r| sampled(r.reader_mut(), region, max_edge, Tier::Terrain))
        {
            Ok(Some(detail)) => return Ok(Some(detail)),
            Ok(None) => {}
            Err(e) => error = Some(e),
        }
    }
    if let Some(cache) = &mut main
        && let Some(detail) = sampled(cache.reader_mut(), region, max_edge, Tier::Terrain)?
    {
        return Ok(Some(detail));
    }
    error.map_or(Ok(None), Err)
}

#[cfg(test)]
fn cached_file(
    path: &Path,
    region: Region,
    max_edge: usize,
    has_srtm: bool,
) -> Result<Option<Detail>, String> {
    cached_file_mode(path, region, max_edge, has_srtm, false)
}

fn sampled(
    cache: &mut tile_archive::atlas::Reader,
    region: Region,
    max_edge: usize,
    tier: tile_archive::atlas::Tier,
) -> Result<Option<Detail>, String> {
    use tile_archive::atlas::Tier;
    let [w, s, e, n] = region.bounds().map(f64::from);
    let west = (w + 180.0).rem_euclid(360.0) - 180.0;
    let size = region.size(tier.spacing() as f32, max_edge);
    Ok(cache
        .sample([west, s, west + (e - w), n], size, tier)?
        .map(|metres| Detail {
            grid: HeightGrid {
                width: size[0],
                height: size[1],
                metres,
            },
            region,
            save: None,
            colors: None,
            complete: true,
            source: if tier == Tier::Land {
                "Atlas cache · 1 arcsecond land"
            } else {
                "Atlas cache · 15 arcsecond terrain"
            },
        }))
}

fn merge_land(base: &mut [f32], land: &[f32]) {
    for (base, &land) in base.iter_mut().zip(land) {
        // SRTM sea pixels are often zero; they must not erase measured depths.
        if land.is_finite() && (-11000.0..=9000.0).contains(&land) && (land > 0.0 || *base >= 0.0) {
            *base = land;
        }
    }
}

fn srtm_files(root: &Path, b: [f32; 4]) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for lat in (b[1].floor() as i32).max(-90)..(b[3].ceil() as i32).min(90) {
        for unwrapped in b[0].floor() as i32..b[2].ceil() as i32 {
            let lon = (unwrapped + 180).rem_euclid(360) - 180;
            let path = root.join(format!(
                "{}{:02}{}{:03}.tif",
                if lat >= 0 { 'N' } else { 'S' },
                lat.abs(),
                if lon >= 0 { 'E' } else { 'W' },
                lon.abs()
            ));
            if path.exists() {
                paths.push(path);
            }
        }
    }
    paths
}

fn fine_files(root: &Path, b: [f32; 4]) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for y in (b[1] / threedep::CHUNK_DEG).floor() as i32..(b[3] / threedep::CHUNK_DEG).ceil() as i32
    {
        for x in
            (b[0] / threedep::CHUNK_DEG).floor() as i32..(b[2] / threedep::CHUNK_DEG).ceil() as i32
        {
            let point = GeoPoint {
                lat: (y as f32 + 0.5) * threedep::CHUNK_DEG,
                lon: ((x as f32 + 0.5) * threedep::CHUNK_DEG + 180.0).rem_euclid(360.0) - 180.0,
            };
            if let Some(path) = threedep::peek_chunk(root, threedep::chunk_key_for(point)) {
                paths.push(path);
            }
        }
    }
    paths
}

/// Split output columns at longitude zones. Pixel edges/centres stay on one
/// exact grid, even when the half-pixel edge straddles the date line.
fn warp(
    files: &[PathBuf],
    b: [f32; 4],
    size: [usize; 2],
    scratch: &Path,
    name: &str,
) -> Result<Vec<f32>, String> {
    let mut out = vec![f32::NAN; size[0] * size[1]];
    let dx = (b[2] as f64 - b[0] as f64) / size[0] as f64;
    let mut start = 0;
    while start < size[0] {
        let center = b[0] as f64 + (start as f64 + 0.5) * dx;
        let zone = ((center + 180.0) / 360.0).floor();
        let end = ((((180.0 + zone * 360.0 - b[0] as f64) / dx) - 0.5).ceil() as usize)
            .clamp(start + 1, size[0]);
        let west = b[0] as f64 + start as f64 * dx - zone * 360.0;
        let east = b[0] as f64 + end as f64 * dx - zone * 360.0;
        let output = scratch.join(format!("{name}-{start}.bil"));
        let tool = gdal::gdal_tool_path("gdalwarp");
        let mut command = Command::new(&tool);
        command
            .args([
                "-q",
                "-overwrite",
                "-of",
                "EHdr",
                "-ot",
                "Float32",
                "-t_srs",
                "EPSG:4326",
                "-te_srs",
                "EPSG:4326",
                "-te",
            ])
            .args([
                west.to_string(),
                b[1].to_string(),
                east.to_string(),
                b[3].to_string(),
            ])
            .args([
                "-ts",
                &(end - start).to_string(),
                &size[1].to_string(),
                "-r",
                "bilinear",
                "-et",
                "0",
                "-dstnodata",
                "-999999",
                "-wm",
                "64",
                "-wo",
                "NUM_THREADS=1",
                "--config",
                "GDAL_CACHEMAX",
                "64",
            ])
            .args(files)
            .arg(&output)
            .stdout(Stdio::null());
        gdal::run_command_with_timeout(command, "Atlas native elevation", Duration::from_secs(120))
            .map_err(|e| format!("Cannot refine Atlas with {}: {e}. Check the GDAL folder in Settings → Files & Cache.", tool.display()))?;
        let bytes = std::fs::read(&output)
            .map_err(|e| format!("Cannot read refined Atlas terrain: {e}"))?;
        if bytes.len() != (end - start) * size[1] * 4 {
            return Err("Refined Atlas terrain has invalid dimensions.".into());
        }
        for (i, bytes) in bytes.chunks_exact(4).enumerate() {
            out[(i / (end - start)) * size[0] + start + i % (end - start)] =
                f32::from_le_bytes(bytes.try_into().unwrap());
        }
        start = end;
    }
    Ok(out)
}

#[cfg(test)]
#[path = "atlas_source_tests.rs"]
mod tests;

#[cfg(test)]
pub(super) fn flush_view_cache() {
    view_cache::flush();
}
