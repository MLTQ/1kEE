//! Offline Atlas height-tile generation, independent of OSM build state.
use crate::roads::RoadBuildProgress;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::{Command as Process, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tile_archive::atlas::{Key, Tier, Writer};
#[derive(Debug, Clone)]
pub struct Command {
    pub data_root: PathBuf,
    pub srtm_root: Option<PathBuf>,
    pub cache: PathBuf,
    pub gdal_bin_dir: PathBuf,
    pub bounds: [f64; 4],
    pub tiers: Vec<Tier>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Progress {
    pub total: u64,
    pub done: u64,
    pub built: u64,
    pub reused: u64,
    pub unavailable: u64,
    pub bytes: u64,
    pub elapsed: f64,
    pub state: String,
    pub message: String,
}
impl Progress {
    pub fn fraction(&self) -> f32 {
        self.done as f32 / self.total.max(1) as f32
    }
}
pub fn parse(args: impl Iterator<Item = String>) -> Result<Command, String> {
    let mut c = Command {
        data_root: PathBuf::new(),
        srtm_root: None,
        cache: PathBuf::new(),
        gdal_bin_dir: PathBuf::new(),
        bounds: [-180., -90., 180., 90.],
        tiers: vec![Tier::Overview, Tier::Terrain],
    };
    let mut args = args;
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("Missing value for {flag}"))?;
        match flag.as_str() {
            "--data-root" => c.data_root = value.into(),
            "--srtm-root" => c.srtm_root = Some(value.into()),
            "--out" => c.cache = value.into(),
            "--gdal-bin-dir" => c.gdal_bin_dir = value.into(),
            "--min-lon" => c.bounds[0] = value.parse().map_err(|_| "Invalid longitude")?,
            "--min-lat" => c.bounds[1] = value.parse().map_err(|_| "Invalid latitude")?,
            "--max-lon" => c.bounds[2] = value.parse().map_err(|_| "Invalid longitude")?,
            "--max-lat" => c.bounds[3] = value.parse().map_err(|_| "Invalid latitude")?,
            "--tiers" => {
                c.tiers = value
                    .split(',')
                    .map(|v| match v.trim() {
                        "overview" => Ok(Tier::Overview),
                        "terrain" => Ok(Tier::Terrain),
                        "land" => Ok(Tier::Land),
                        _ => Err(format!("Unknown Atlas tier {v}")),
                    })
                    .collect::<Result<_, _>>()?;
            }
            _ => return Err(format!("Unknown Atlas option {flag}")),
        }
    }
    if c.data_root.as_os_str().is_empty() || c.cache.as_os_str().is_empty() {
        return Err("atlas --data-root <Data folder or GEBCO raster> --out <Derived/terrain/atlas_heights_v1.sqlite> [--srtm-root <folder>] [--tiers overview,terrain,land] [--min-lon W --min-lat S --max-lon E --max-lat N] [--gdal-bin-dir <folder>]".into());
    }
    tile_archive::atlas::validate_bounds(c.bounds)?;
    if c.tiers.is_empty() {
        return Err("Select Atlas tiers".into());
    }
    Ok(c)
}
fn base_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    if root.is_file() {
        return Ok(vec![root.to_owned()]);
    }
    let root = if root.join("Data").is_dir() {
        root.join("Data")
    } else {
        root.to_owned()
    };
    let nested = root.join("GEBCO/gebco_2025_sub_ice_topo_geotiff");
    let root = if nested.is_dir() { nested } else { root };
    let mut files: Vec<_> = fs::read_dir(&root)
        .map_err(|e| format!("Cannot open GEBCO folder {}: {e}", root.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|s| s == "tif")
                && p.file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.starts_with("gebco_") && !s.contains("_tid_"))
        })
        .collect();
    files.sort();
    if files.is_empty() {
        return Err(format!(
            "No original GEBCO topography files found in {}",
            root.display()
        ));
    }
    Ok(files)
}
fn srtm_catalog(root: Option<&Path>) -> Result<HashMap<(i32, i32), PathBuf>, String> {
    let mut result = HashMap::new();
    let Some(root) = root else { return Ok(result) };
    for entry in fs::read_dir(root).map_err(|e| format!("Cannot read SRTM folder: {e}"))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let b = name.as_bytes();
        if b.len() != 11
            || &b[7..] != b".tif"
            || !matches!(b[0], b'N' | b'S')
            || !matches!(b[3], b'E' | b'W')
            || !b[1..3].iter().all(u8::is_ascii_digit)
            || !b[4..7].iter().all(u8::is_ascii_digit)
        {
            continue;
        }
        let Ok(mut lat) = name[1..3].parse::<i32>() else {
            continue;
        };
        let Ok(mut lon) = name[4..7].parse::<i32>() else {
            continue;
        };
        if b[0] == b'S' {
            lat = -lat;
        }
        if b[3] == b'W' {
            lon = -lon;
        }
        if (-90..90).contains(&lat) && (-180..180).contains(&lon) {
            result.insert((lat, lon), path);
        }
    }
    Ok(result)
}
fn land_files(catalog: &HashMap<(i32, i32), PathBuf>, b: [f64; 4]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for lat in (b[1].floor() as i32).max(-90)..(b[3].ceil() as i32).min(90) {
        for x in b[0].floor() as i32..b[2].ceil() as i32 {
            if let Some(p) = catalog.get(&(lat, (x + 180).rem_euclid(360) - 180))
                && !out.contains(p)
            {
                out.push(p.clone());
            }
        }
    }
    out.sort();
    out
}
type FileStamp = (PathBuf, u64, u128);

fn file_stamp(p: &Path) -> Result<FileStamp, String> {
    let m = fs::metadata(p).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok((
        fs::canonicalize(p).map_err(|e| e.to_string())?,
        m.len(),
        m.modified()
            .map_err(|e| e.to_string())?
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos(),
    ))
}

fn restat_stamp(canonical: &Path) -> Result<FileStamp, String> {
    let m = fs::metadata(canonical).map_err(|e| format!("{}: {e}", canonical.display()))?;
    Ok((
        canonical.to_path_buf(),
        m.len(),
        m.modified()
            .map_err(|e| e.to_string())?
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos(),
    ))
}

fn hash_stamps(stamps: &[FileStamp]) -> Result<String, String> {
    let bytes = serde_json::to_vec(&(1, stamps)).map_err(|e| e.to_string())?;
    Ok(ring::digest::digest(&ring::digest::SHA256, &bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn build_base_vrt(base: &[PathBuf], gdal: &Path, scratch: &Path) -> Option<PathBuf> {
    if base.len() <= 1 {
        return None;
    }
    let vrt = scratch.join("base.vrt");
    let mut cmd = Process::new(if gdal.as_os_str().is_empty() {
        PathBuf::from("gdalbuildvrt")
    } else {
        gdal.join("gdalbuildvrt")
    });
    cmd.arg("-q")
        .args(["--config", "GDAL_PAM_ENABLED", "NO"])
        .arg(&vrt)
        .args(base)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::contours::run_gdal(cmd, "Atlas gdalbuildvrt").ok()?;
    vrt.is_file().then_some(vrt)
}

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Result<Self, String> {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "1kee-atlas-{}-{}-{id}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_nanos()
        ));
        fs::create_dir(&path).map_err(|e| e.to_string())?;
        Ok(Self(path))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn warp(
    files: &[PathBuf],
    bounds: [f64; 4],
    size: usize,
    scratch: &Path,
    name: &str,
    gdal: &Path,
) -> Result<Vec<f32>, String> {
    let mut out = vec![f32::NAN; size * size];
    let dx = (bounds[2] - bounds[0]) / size as f64;
    let mut start = 0;
    while start < size {
        let center = bounds[0] + (start as f64 + 0.5) * dx;
        let zone = ((center + 180.) / 360.).floor();
        let end = ((((180. + zone * 360. - bounds[0]) / dx) - 0.5).ceil() as usize)
            .clamp(start + 1, size);
        let output = scratch.join(format!("{name}-{start}.bil"));
        let log = scratch.join("gdal.log");
        let mut command = Process::new(if gdal.as_os_str().is_empty() {
            PathBuf::from("gdalwarp")
        } else {
            gdal.join("gdalwarp")
        });
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
                (bounds[0] + start as f64 * dx - zone * 360.).to_string(),
                bounds[1].to_string(),
                (bounds[0] + end as f64 * dx - zone * 360.).to_string(),
                bounds[3].to_string(),
            ])
            .args([
                "-ts",
                &(end - start).to_string(),
                &size.to_string(),
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
                "--config",
                "GDAL_PAM_ENABLED",
                "NO",
            ])
            .args(files)
            .arg(&output)
            .stdout(Stdio::null())
            .stderr(fs::File::create(&log).map_err(|e| e.to_string())?);
        crate::contours::run_gdal(command, "Atlas gdalwarp").map_err(|e| {
            format!(
                "{e}: {}",
                fs::read_to_string(&log)
                    .unwrap_or_default()
                    .chars()
                    .take(3000)
                    .collect::<String>()
            )
        })?;
        let bytes = fs::read(output).map_err(|e| e.to_string())?;
        if bytes.len() != (end - start) * size * 4 {
            return Err("Invalid Atlas GDAL raster size".into());
        }
        for (i, b) in bytes.as_chunks::<4>().0.iter().enumerate() {
            let v = f32::from_le_bytes(*b);
            out[(i / (end - start)) * size + start + i % (end - start)] =
                if v.is_finite() && (-12000.0..=10000.0).contains(&v) {
                    v
                } else {
                    f32::NAN
                };
        }
        start = end;
    }
    Ok(out)
}
fn tile(key: Key, base: &[PathBuf], land: &[PathBuf], gdal: &Path) -> Result<Vec<f32>, String> {
    let scratch = Scratch::new()?;
    let b = key.source_bounds();
    let side = key.tier.side();
    let mut heights = warp(base, b, side, &scratch.0, "base", gdal)?;
    if !land.is_empty() {
        let values = warp(land, b, side, &scratch.0, "land", gdal)?;
        for (base, land) in heights.iter_mut().zip(values) {
            if land.is_finite() && (-11000.0..=9000.0).contains(&land) && (land > 0. || *base >= 0.)
            {
                *base = land;
            }
        }
    }
    if !(1..side - 1).any(|y| {
        heights[y * side + 1..y * side + side - 1]
            .iter()
            .any(|v| v.is_finite())
    }) {
        return Err(format!(
            "No usable elevation samples for {key:?}; tile was not saved"
        ));
    }
    Ok(heights)
}
pub fn run(c: Command, progress: &mut dyn FnMut(Progress)) -> Result<String, String> {
    let started = Instant::now();
    let mut p = Progress {
        state: "Planning Atlas tiles".into(),
        ..Default::default()
    };
    let mut emit = |p: &mut Progress| {
        p.elapsed = started.elapsed().as_secs_f64();
        progress(p.clone());
    };
    emit(&mut p);
    let result: Result<String, String> = (|| {
        let base = base_files(&c.data_root)?;
        let catalog = if c.tiers.contains(&Tier::Land) {
            let root = c.srtm_root.clone().or_else(|| {
                [
                    c.data_root.join("srtm_gl1"),
                    c.data_root.join("Data/srtm_gl1"),
                ]
                .into_iter()
                .find(|p| p.is_dir())
            });
            srtm_catalog(root.as_deref())?
        } else {
            HashMap::new()
        };
        if c.tiers.contains(&Tier::Land) && catalog.is_empty() {
            return Err("Land detail requires a folder containing SRTM .tif tiles".into());
        }
        let mut keys = tile_archive::atlas::plan(c.bounds, &c.tiers)?;
        keys.retain(|k| {
            if k.tier != Tier::Land {
                return true;
            }
            let b = k.bounds();
            let yes = catalog.contains_key(&(
                ((b[1] + b[3]) * 0.5).floor() as i32,
                ((b[0] + b[2]) * 0.5).floor() as i32,
            ));
            if !yes {
                p.unavailable += 1;
            }
            yes
        });
        p.total = keys.len() as u64;
        if keys.is_empty() {
            return Err("No SRTM coverage intersects the selected bounds".into());
        }
        let writer = Writer::open(&c.cache)?;
        // Precompute canonical stamps for `base` once (and any SRTM files on demand)
        // to avoid thousands of redundant canonicalize/JSON/SHA-256 calls.
        let base_stamps: Vec<FileStamp> = base
            .iter()
            .map(|p| file_stamp(p))
            .collect::<Result<Vec<_>, _>>()?;
        let base_identity = hash_stamps(&base_stamps)?;
        let mut land_stamp_cache: HashMap<PathBuf, FileStamp> = HashMap::new();

        // Phase 1: check reusability sequentially on the writer connection and
        // collect the tiles that actually need building.
        struct BuildItem {
            key: Key,
            land: Vec<PathBuf>,
            canonical_files: Vec<PathBuf>,
            stamp: String,
            needed: u64,
        }
        let mut to_build: Vec<BuildItem> = Vec::new();
        p.state = "Checking Atlas tiles".into();
        for key in keys {
            let land = if key.tier == Tier::Land {
                land_files(&catalog, key.source_bounds())
            } else {
                Vec::new()
            };
            let (stamp, canonical_files) = if land.is_empty() {
                (
                    base_identity.clone(),
                    base_stamps.iter().map(|s| s.0.clone()).collect(),
                )
            } else {
                let mut stamps = Vec::with_capacity(base_stamps.len() + land.len());
                stamps.extend(base_stamps.iter().cloned());
                for p in &land {
                    let s = if let Some(cached) = land_stamp_cache.get(p) {
                        cached.clone()
                    } else {
                        let fresh = file_stamp(p)?;
                        land_stamp_cache.insert(p.clone(), fresh.clone());
                        fresh
                    };
                    stamps.push(s);
                }
                let hash = hash_stamps(&stamps)?;
                let canon = stamps.into_iter().map(|s| s.0).collect();
                (hash, canon)
            };
            p.message = format!("{} · tile {},{}", key.tier.name(), key.x, key.y);
            emit(&mut p);
            match writer.reusable(key, &stamp) {
                Ok(true) => {
                    p.reused += 1;
                    p.done += 1;
                    emit(&mut p);
                    continue;
                }
                Ok(false) => {}
                Err(e) => {
                    p.message = format!("Rebuilding unreadable tile: {e}");
                    emit(&mut p);
                }
            }
            let needed = (key.tier.side().pow(2) * 4) as u64;
            to_build.push(BuildItem {
                key,
                land,
                canonical_files,
                stamp,
                needed,
            });
        }

        if !to_build.is_empty() {
            // Build a single shared GEBCO VRT once when multiple base files exist.
            let vrt_scratch = Scratch::new()?;
            let warp_base: Vec<PathBuf> = match build_base_vrt(&base, &c.gdal_bin_dir, &vrt_scratch.0)
            {
                Some(vrt) => vec![vrt],
                None => base.clone(),
            };

            let max_needed = to_build.iter().map(|i| i.needed).max().unwrap_or(0);
            if crate::archive_space::available(&c.cache)
                .is_some_and(|free| free < max_needed * 4 + 1024 * 1024 * 1024)
            {
                return Err("Not enough free space for Atlas tile; saved tiles preserved. Free space and resume.".into());
            }

            let cache_path = &c.cache;
            let gdal_dir = &c.gdal_bin_dir;

            // Build the first pending tile sequentially as a fast-fail preflight
            // (verifies GDAL tooling/inputs and avoids thread-pool overhead for
            // 1-tile builds), then fan out all remaining tiles in parallel.
            let mut rest = to_build;
            let first = rest.remove(0);
            p.state = "Building Atlas tile".into();
            p.message = format!(
                "{} · tile {},{}",
                first.key.tier.name(),
                first.key.x,
                first.key.y
            );
            emit(&mut p);
            let heights = tile(first.key, &warp_base, &first.land, gdal_dir)?;
            let after: Vec<FileStamp> = first
                .canonical_files
                .iter()
                .map(|path| restat_stamp(path))
                .collect::<Result<Vec<_>, _>>()?;
            if hash_stamps(&after)? != first.stamp {
                return Err(
                    "Atlas source changed during tile generation; tile was not published".into(),
                );
            }
            writer.put(first.key, &first.stamp, &heights)?;
            p.built += 1;
            p.done += 1;
            p.bytes += first.needed;
            emit(&mut p);

            if !rest.is_empty() {
                let num_workers = (std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(4)
                    / 2)
                .clamp(2, 8)
                .min(rest.len());

                p.state = "Building Atlas tiles".into();
                p.message = format!("Building {} tiles across {num_workers} workers", rest.len());
                emit(&mut p);

                let stop = std::sync::atomic::AtomicBool::new(false);
                let (tx, rx) =
                    std::sync::mpsc::sync_channel::<Result<(Key, String, Vec<f32>, u64), String>>(
                        num_workers * 2,
                    );

                let build_result: Result<(), String> = std::thread::scope(|scope| {
                    let stop_ref = &stop;
                    let warp_base_ref = &warp_base;
                    let producer = scope.spawn(move || {
                        use rayon::prelude::*;
                        let pool = rayon::ThreadPoolBuilder::new()
                            .num_threads(num_workers)
                            .build()
                            .expect("atlas rayon pool");
                        pool.install(|| {
                            rest.into_par_iter().for_each_with(tx, |tx, item| {
                                if stop_ref.load(std::sync::atomic::Ordering::Relaxed) {
                                    return;
                                }
                                let res = (|| -> Result<(Key, String, Vec<f32>, u64), String> {
                                    let heights =
                                        tile(item.key, warp_base_ref, &item.land, gdal_dir)?;
                                    let after: Vec<FileStamp> = item
                                        .canonical_files
                                        .iter()
                                        .map(|p| restat_stamp(p))
                                        .collect::<Result<Vec<_>, _>>()?;
                                    if hash_stamps(&after)? != item.stamp {
                                        return Err(
                                            "Atlas source changed during tile generation; tile was not published"
                                                .into(),
                                        );
                                    }
                                    Ok((item.key, item.stamp, heights, item.needed))
                                })();
                                if res.is_err() {
                                    stop_ref.store(true, std::sync::atomic::Ordering::Relaxed);
                                }
                                let _ = tx.send(res);
                            });
                        });
                    });

                    let mut last_space = Instant::now();
                    let mut err: Option<String> = None;
                    loop {
                        match rx.recv_timeout(Duration::from_secs(1)) {
                            Ok(Ok((key, stamp, heights, needed))) => {
                                if err.is_some() {
                                    continue;
                                }
                                if last_space.elapsed() >= Duration::from_secs(2) {
                                    last_space = Instant::now();
                                    if crate::archive_space::available(cache_path)
                                        .is_some_and(|free| free < needed * 4 + 1024 * 1024 * 1024)
                                    {
                                        stop.store(true, std::sync::atomic::Ordering::Relaxed);
                                        err = Some("Not enough free space for Atlas tile; saved tiles preserved. Free space and resume.".into());
                                        continue;
                                    }
                                }
                                if let Err(e) = writer.put(key, &stamp, &heights) {
                                    stop.store(true, std::sync::atomic::Ordering::Relaxed);
                                    err = Some(e);
                                    continue;
                                }
                                p.built += 1;
                                p.done += 1;
                                p.bytes += needed;
                                p.message =
                                    format!("{} · tile {},{}", key.tier.name(), key.x, key.y);
                                emit(&mut p);
                            }
                            Ok(Err(e)) => {
                                stop.store(true, std::sync::atomic::Ordering::Relaxed);
                                err.get_or_insert(e);
                            }
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                                emit(&mut p);
                            }
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    }
                    let _ = producer.join();
                    match err {
                        Some(e) => Err(e),
                        None => Ok(()),
                    }
                });
                build_result?;
            }
        }
        writer.finish()?;
        Ok(format!(
            "Atlas: {} tiles built, {} reused; {} covered tiles ready in {}",
            p.built,
            p.reused,
            p.total,
            c.cache.display()
        ))
    })();
    p.state = if result.is_ok() {
        "Completed"
    } else {
        "Failed"
    }
    .into();
    p.message = match &result {
        Ok(s) | Err(s) => s.clone(),
    };
    emit(&mut p);
    result
}
pub fn road_progress(p: &Progress) -> RoadBuildProgress {
    RoadBuildProgress {
        metrics: None,
        stage: p.state.clone(),
        fraction: p.fraction(),
        message: p.message.clone(),
    }
}
#[cfg(test)]
#[path = "atlas_tests.rs"]
mod tests;
