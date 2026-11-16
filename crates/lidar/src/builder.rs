//! Resumable LAS/LAZ -> numeric GeoTIFF conversion. No Python subprocesses.
use crate::geo::{CoordTransform, Dataset, SpatialRef};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufReader, Read},
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, String>;
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn io(e: std::io::Error) -> String {
    format!("I/O: {e}")
}
pub const SCHEMA: &str = include_str!("schema.sql");

#[derive(Debug, Serialize)]
pub struct Grid {
    pub w: f64,
    pub s: f64,
    pub width: usize,
    pub height: usize,
    pub resolution: f64,
    pub bounds: [f64; 4],
    pub source_crs: String,
    #[serde(skip)]
    horizontal_crs: String,
}

#[derive(Debug, Serialize)]
pub struct Stats {
    pub points: u64,
    pub valid_points: u64,
    pub valid_cells: u64,
}

pub fn digest(path: &Path) -> Result<String> {
    let mut reader = BufReader::new(File::open(path).map_err(io)?);
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        let n = reader.read(&mut buffer).map_err(io)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn source_crs(header: &las::Header) -> Result<SpatialRef> {
    if let Some(wkt) = header.get_wkt_crs_bytes() {
        return SpatialRef::from_wkt(
            std::str::from_utf8(wkt)
                .map_err(err)?
                .trim_end_matches('\0'),
        )
        .map_err(err);
    }
    if let Some(keys) = header.get_geotiff_crs().map_err(err)?
        && let Some(code) = keys
            .get_projected_crs_geo_key_value()
            .or_else(|| keys.get_geodetic_crs_geo_key_value())
    {
        if code == 32767 {
            return Err("User-defined GeoTIFF CRS needs WKT".into());
        }
        let definition = if let Some(vertical) = keys.get_vertical_crs_geo_key_value() {
            format!("EPSG:{code}+{vertical}")
        } else {
            format!("EPSG:{code}")
        };
        return SpatialRef::from_definition(&definition).map_err(err);
    }
    Err("Missing point-cloud coordinate system".into())
}

fn horizontal(original: &SpatialRef) -> Result<SpatialRef> {
    original.horizontal()
}

fn transform(wkt: &str) -> Result<CoordTransform> {
    CoordTransform::new(SpatialRef::from_wkt(wkt)?, SpatialRef::from_epsg(4326)?)
}

pub fn grid(path: &Path, zoom: u8) -> Result<Grid> {
    if !(12..=16).contains(&zoom) {
        return Err("Zoom must be 12–16".into());
    }
    let reader = las::Reader::from_path(path).map_err(err)?;
    let header = reader.header();
    if header.number_of_points() == 0 {
        return Err("Empty point cloud".into());
    }
    let original = source_crs(header)?;
    let horizontal = horizontal(&original)?.to_wkt().map_err(err)?;
    let extent = header.bounds();
    let bounds = transform(&horizontal)?
        .transform_bounds(
            &[extent.min.x, extent.min.y, extent.max.x, extent.max.y],
            21,
        )
        .map_err(err)?;
    let [w, s, e, n] = bounds;
    if !bounds.iter().all(|v| v.is_finite())
        || !(-180. <= w && w <= e && e <= 180. && -90. <= s && s <= n && n <= 90.)
    {
        return Err("Unsupported wrapped/invalid geographic bounds".into());
    }
    let resolution = 180. / (1u32 << zoom) as f64 / 512.;
    let left = ((w + 180.) / resolution).floor() - 1.;
    let bottom = ((s + 90.) / resolution).floor() - 1.;
    let right = ((e + 180.) / resolution).ceil() + 1.;
    let top = ((n + 90.) / resolution).ceil() + 1.;
    let (width, height) = ((right - left) as usize, (top - bottom) as usize);
    if width
        .checked_mul(height)
        .is_none_or(|cells| cells > 16_000_000)
    {
        return Err("Source exceeds bounded 16-million-cell raster; needs tiled ingestion".into());
    }
    Ok(Grid {
        w: left * resolution - 180.,
        s: bottom * resolution - 90.,
        width,
        height,
        resolution,
        bounds,
        source_crs: original.to_wkt().map_err(err)?,
        horizontal_crs: horizontal,
    })
}

pub fn convert(source: &Path, output: &Path, zoom: u8) -> Result<(Grid, Stats)> {
    let grid = grid(source, zoom)?;
    let cells = grid.width * grid.height;
    let mut sums = vec![0f64; cells];
    let mut counts = vec![0u32; cells];
    let mut low = vec![f64::INFINITY; cells];
    let mut high = vec![f64::NEG_INFINITY; cells];
    let transform = transform(&grid.horizontal_crs)?;
    let mut reader = las::Reader::from_path(source).map_err(err)?;
    let expected = reader.header().number_of_points();
    let (mut seen, mut accepted) = (0u64, 0u64);
    loop {
        let data = reader.read_points(250_000).map_err(err)?;
        if data.is_empty() {
            break;
        }
        seen += data.len() as u64;
        let mut xs = Vec::with_capacity(data.len());
        let mut ys = Vec::with_capacity(data.len());
        let mut zs = Vec::with_capacity(data.len());
        for point in data.points() {
            let p = point.map_err(err)?;
            if p.is_withheld || matches!(u8::from(p.classification), 7 | 18) {
                continue;
            }
            xs.push(p.x);
            ys.push(p.y);
            zs.push(p.z);
        }
        transform.transform_coords(&mut xs, &mut ys)?;
        for ((x, y), z) in xs.into_iter().zip(ys).zip(zs) {
            if !x.is_finite() || !y.is_finite() || !z.is_finite() {
                return Err("Nonfinite transformed measurement".into());
            }
            if z.abs() > f32::MAX as f64 {
                return Err("Measurement exceeds finite float32 storage".into());
            }
            let col = ((x - grid.w) / grid.resolution).floor();
            let row = grid.height as f64 - 1. - ((y - grid.s) / grid.resolution).floor();
            if col < 0. || col >= grid.width as f64 || row < 0. || row >= grid.height as f64 {
                return Err("Transformed point outside header-derived raster".into());
            }
            let i = row as usize * grid.width + col as usize;
            counts[i] = counts[i]
                .checked_add(1)
                .filter(|c| *c <= 1 << 24)
                .ok_or("Count exceeds exact float32 storage")?;
            sums[i] += z;
            low[i] = low[i].min(z);
            high[i] = high[i].max(z);
            accepted += 1;
        }
    }
    if seen != expected {
        return Err("Incomplete point-cloud decode".into());
    }
    let mut dataset = Dataset::create(output, grid.width, grid.height)
        .map_err(|e| format!("I/O: raster create: {e}"))?;
    dataset
        .set_spatial_ref(&SpatialRef::from_epsg(4326).map_err(err)?)
        .map_err(err)?;
    dataset
        .set_geo_transform(&[
            grid.w,
            grid.resolution,
            0.,
            grid.s + grid.height as f64 * grid.resolution,
            0.,
            -grid.resolution,
        ])
        .map_err(err)?;
    dataset
        .set_metadata_item("source_crs", &grid.source_crs, "")
        .map_err(err)?;
    dataset
        .set_metadata_item("product", "all-return elevation statistics", "")
        .map_err(err)?;
    dataset
        .set_metadata_item("processor", "1kEE native Rust v1", "")
        .map_err(err)?;
    for (b, name) in ["min", "max", "mean", "count"].iter().enumerate() {
        let mut values: Vec<f32> = (0..cells)
            .map(|i| {
                if counts[i] == 0 {
                    f32::NAN
                } else {
                    match b {
                        0 => low[i] as f32,
                        1 => high[i] as f32,
                        2 => (sums[i] / counts[i] as f64) as f32,
                        _ => counts[i] as f32,
                    }
                }
            })
            .collect();
        let mut band = dataset.rasterband(b + 1).map_err(err)?;
        band.set_no_data_value(f64::NAN).map_err(err)?;
        band.set_description(name).map_err(err)?;
        band.write(&mut values)
            .map_err(|e| format!("I/O: raster write: {e}"))?;
    }
    dataset
        .flush_cache()
        .map_err(|e| format!("I/O: raster flush: {e}"))?;
    drop(dataset);
    let stored = Dataset::open(output)?;
    for b in 0..4 {
        for (i, value) in stored.rasterband(b + 1)?.read()?.into_iter().enumerate() {
            let expected = if counts[i] == 0 {
                f32::NAN
            } else {
                match b {
                    0 => low[i] as f32,
                    1 => high[i] as f32,
                    2 => (sums[i] / counts[i] as f64) as f32,
                    _ => counts[i] as f32,
                }
            };
            if value != expected && !(value.is_nan() && expected.is_nan()) {
                return Err("Numeric raster round-trip mismatch".into());
            }
        }
    }
    if counts.iter().map(|&v| v as u64).sum::<u64>() != accepted {
        return Err("Point-count mismatch".into());
    }
    Ok((
        grid,
        Stats {
            points: seen,
            valid_points: accepted,
            valid_cells: counts.iter().filter(|c| **c > 0).count() as u64,
        },
    ))
}

#[derive(Clone, Debug)]
struct Asset {
    url: String,
    survey: String,
    source: PathBuf,
    date: String,
}
fn open_read(path: &Path) -> Result<Connection> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(err)
}
fn assets(root: &Path) -> Result<Vec<Asset>> {
    let catalog = open_read(&root.join("catalog.sqlite"))?;
    let mut stmt = catalog
        .prepare("SELECT id,coalesce(date,'') FROM surveys")
        .map_err(err)?;
    let dates = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(err)?
        .collect::<std::result::Result<HashMap<_, _>, _>>()
        .map_err(err)?;
    let bulk = open_read(&root.join("bulk.sqlite"))?;
    let mut stmt = bulk
        .prepare("SELECT url,survey,path,bytes FROM files WHERE kind='copc'")
        .map_err(err)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, u64>(3)?,
            ))
        })
        .map_err(err)?;
    let mut result = Vec::new();
    for row in rows {
        let (url, survey, path, size) = row.map_err(err)?;
        let source = root.join(&path);
        if source.is_file() {
            if !source
                .canonicalize()
                .map_err(io)?
                .starts_with(root.canonicalize().map_err(io)?)
            {
                return Err("Receipt source escapes lidar root".into());
            }
            if source.metadata().map_err(io)?.len() != size {
                continue;
            }
            result.push(Asset {
                url,
                date: dates.get(&survey).cloned().unwrap_or_default(),
                survey,
                source,
            });
        }
    }
    result.sort_by(|a, b| (&b.date, &b.survey, &b.url).cmp(&(&a.date, &a.survey, &a.url)));
    Ok(result)
}
fn stamp(meta: &fs::Metadata) -> Result<i64> {
    i64::try_from(
        meta.modified()
            .map_err(io)?
            .duration_since(UNIX_EPOCH)
            .map_err(err)?
            .as_nanos(),
    )
    .map_err(err)
}

fn prepare(root: &Path, db: &Connection, asset: &Asset, zoom: u8) -> Result<&'static str> {
    let before = asset.source.metadata().map_err(io)?;
    let old=db.query_row("SELECT source_bytes,source_mtime_ns,zoom,path,raster_sha256,date FROM rasters WHERE url=?1",[&asset.url],
        |r| Ok((r.get::<_,u64>(0)?,r.get::<_,i64>(1)?,r.get::<_,u8>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?))).optional().map_err(err)?;
    if let Some((size, mtime, z, path, sha, date)) = old
        && size == before.len()
        && mtime == stamp(&before)?
        && z == zoom
        && date == asset.date
    {
        let output = root.join(path);
        if output.is_file() && digest(&output)? == sha {
            return Ok("reused");
        }
    }
    let spec = grid(&asset.source, zoom)?;
    if fs2::available_space(root).map_err(io)?
        < 10_000_000_000 + spec.width as u64 * spec.height as u64 * 32
    {
        return Err("I/O: paused at 10 GB reserve plus raster scratch".into());
    }
    let sha = digest(&asset.source)?;
    let folder = root.join("fused/v1/rasters");
    fs::create_dir_all(&folder).map_err(io)?;
    let name = format!(
        "{:x}.tif",
        Sha256::digest(format!("{}{sha}{zoom}", asset.url).as_bytes())
    );
    let output = folder.join(name);
    let temporary = tempfile::Builder::new()
        .suffix(".tif")
        .tempfile_in(&folder)
        .map_err(io)?;
    let (spec, stats) = convert(&asset.source, temporary.path(), zoom)?;
    {
        let mut dataset = Dataset::update(temporary.path()).map_err(err)?;
        for (key, value) in [
            ("survey", &asset.survey),
            ("acquisition", &asset.date),
            ("publisher", &asset.url),
            ("source_sha256", &sha),
        ] {
            dataset.set_metadata_item(key, value, "").map_err(err)?;
        }
        dataset.flush_cache().map_err(err)?;
    }
    let after = asset.source.metadata().map_err(io)?;
    if before.len() != after.len() || stamp(&before)? != stamp(&after)? {
        return Err("Source changed during conversion".into());
    }
    let raster_sha = digest(temporary.path())?;
    let raster_bytes = fs::metadata(temporary.path()).map_err(io)?.len();
    File::open(temporary.path())
        .map_err(io)?
        .sync_all()
        .map_err(io)?;
    temporary.persist(&output).map_err(err)?;
    let [w, s, e, n] = spec.bounds;
    db.execute("INSERT OR REPLACE INTO rasters (url,survey,date,path,source_path,source_bytes,source_mtime_ns,
        source_sha256,raster_sha256,raster_bytes,source_crs,west,south,east,north,points,valid_points,valid_cells,zoom,created)
        VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)",
        params![asset.url,asset.survey,asset.date,output.strip_prefix(root).map_err(err)?.to_string_lossy(),
            asset.source.strip_prefix(root).map_err(err)?.to_string_lossy(),before.len(),stamp(&before)?,sha,raster_sha,raster_bytes,
            spec.source_crs,w,s,e,n,stats.points,stats.valid_points,stats.valid_cells,zoom,
            SystemTime::now().duration_since(UNIX_EPOCH).map_err(err)?.as_secs_f64()]).map_err(err)?;
    db.execute("DELETE FROM failures WHERE url=?1", [&asset.url])
        .map_err(err)?;
    Ok("built")
}

pub fn verify(root: &Path) -> Result<serde_json::Value> {
    let db = open_read(&root.join("fused/v1/index.sqlite"))?;
    let mut stmt = db
        .prepare("SELECT path,raster_sha256,source_bytes,raster_bytes,valid_points FROM rasters")
        .map_err(err)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, u64>(2)?,
                r.get::<_, u64>(3)?,
                r.get::<_, u64>(4)?,
            ))
        })
        .map_err(err)?;
    let (mut total, mut raw, mut packed) = (0u64, 0u64, 0u64);
    let mut failures = Vec::new();
    for row in rows {
        let (path, sha, raw_size, size, points) = row.map_err(err)?;
        let check = (|| -> Result<()> {
            let file = root.join(&path);
            if digest(&file)? != sha || file.metadata().map_err(io)?.len() != size {
                return Err("Raster checksum/size mismatch".into());
            }
            let ds = Dataset::open(file).map_err(err)?;
            if ds.raster_count() != 4 {
                return Err("Raster band count mismatch".into());
            }
            let (w, h) = ds.raster_size();
            if w.checked_mul(h).is_none_or(|v| v > 16_000_000) {
                return Err("Oversized raster".into());
            }
            let mut bands = Vec::new();
            for (i, name) in ["min", "max", "mean", "count"].iter().enumerate() {
                let band = ds.rasterband(i + 1).map_err(err)?;
                if band.description().map_err(err)? != *name {
                    return Err("Raster bands mismatch".into());
                }
                bands.push(band.read()?);
            }
            let mut count = 0u64;
            for (((&min, &max), &mean), &n) in
                bands[0].iter().zip(&bands[1]).zip(&bands[2]).zip(&bands[3])
            {
                if min.is_nan() && max.is_nan() && mean.is_nan() && n.is_nan() {
                    continue;
                }
                if ![min, max, mean, n].iter().all(|v| v.is_finite())
                    || min > mean
                    || mean > max
                    || n < 1.
                    || n.fract() != 0.
                {
                    return Err("Raster statistic/count mismatch".into());
                }
                count += n as u64;
            }
            if count != points {
                return Err("Raster point-count mismatch".into());
            }
            Ok(())
        })();
        match check {
            Ok(()) => {
                total += 1;
                raw += raw_size;
                packed += size;
            }
            Err(error) => failures.push(serde_json::json!({"path":path,"error":error})),
        }
    }
    Ok(
        serde_json::json!({"verified":total,"source_bytes":raw,"raster_bytes":packed,"failures":failures}),
    )
}

pub fn run(args: impl Iterator<Item = String>) -> Result<()> {
    let mut args = args
        .flat_map(|arg| {
            if arg.starts_with("--")
                && let Some((key, value)) = arg.split_once('=')
            {
                return vec![key.into(), value.into()];
            }
            vec![arg]
        })
        .peekable();
    let mode = if args.peek().is_some_and(|s| !s.starts_with('-')) {
        args.next().unwrap()
    } else {
        "build".into()
    };
    let mut options = HashMap::new();
    while let Some(flag) = args.next() {
        if flag == "--plan" || flag == "--verify" {
            options.insert(flag, "true".into());
            continue;
        }
        if ![
            "--root",
            "--zoom",
            "--max-files",
            "--volume-uuid",
            "--input",
            "--output",
            "--catalog",
            "--bounds",
            "--preferred",
        ]
        .contains(&flag.as_str())
        {
            return Err(format!(
                "Unknown option {flag}. Use --root PATH [--plan|--verify] [--volume-uuid UUID] [--max-files N], or convert --input LAS/LAZ --output TIFF"
            ));
        }
        let value = args
            .next()
            .ok_or_else(|| format!("Missing value for {flag}"))?;
        if options.insert(flag.clone(), value).is_some() {
            return Err(format!("Repeated option {flag}"));
        }
    }
    let required = |key: &str| {
        options
            .get(key)
            .map(PathBuf::from)
            .ok_or_else(|| format!("Missing {key}"))
    };
    let zoom = options
        .get("--zoom")
        .map(|v| v.parse::<u8>().map_err(err))
        .transpose()?
        .unwrap_or(15);
    #[cfg(feature = "ept")]
    if mode == "prepare-view" {
        let bounds = options
            .get("--bounds")
            .ok_or("Missing --bounds west,south,east,north")?
            .split(',')
            .map(|v| v.parse::<f64>().map_err(err))
            .collect::<Result<Vec<_>>>()?
            .try_into()
            .map_err(|_| "Expected four bounds")?;
        let catalog = required("--catalog")?;
        let prepared = crate::ept::prepare(&catalog, bounds, zoom, None)?;
        if let Some(output) = options.get("--output")
            && let Some(image) = crate::render::render_prepared(&catalog, bounds, None)?
        {
            crate::render::save_png(Path::new(output), &image)?;
        }
        println!(
            "{}",
            serde_json::json!({"prepared":prepared,"bounds":bounds,"zoom":zoom})
        );
        return Ok(());
    }
    if !(12..=16).contains(&zoom) {
        return Err("Zoom must be 12–16".into());
    }
    if mode == "convert" {
        let source = required("--input")?;
        let output = required("--output")?;
        let (grid, stats) = convert(&source, &output, zoom)?;
        println!("{}", serde_json::json!({"grid":grid,"stats":stats}));
        return Ok(());
    }
    if mode == "render" {
        let bounds: Vec<f64> = options
            .get("--bounds")
            .ok_or("Missing --bounds west,south,east,north")?
            .split(',')
            .map(|v| v.parse().map_err(err))
            .collect::<Result<_>>()?;
        let bounds = bounds.try_into().map_err(|_| "Expected four bounds")?;
        let rendered = crate::render::render(
            &required("--catalog")?,
            bounds,
            options.get("--preferred").map(String::as_str).unwrap_or(""),
            None,
        )?
        .ok_or("View is not completely prepared")?;
        crate::render::save_png(&required("--output")?, &rendered)?;
        return Ok(());
    }
    if mode != "build" {
        return Err(format!("Unknown command {mode}"));
    }
    let root = required("--root")?.canonicalize().map_err(io)?;
    std::env::set_current_dir(&root).map_err(io)?;
    if let Some(uuid) = options.get("--volume-uuid") {
        let identity = std::process::Command::new("findmnt")
            .args(["--noheadings", "--raw", "--output", "UUID", "--target", "."])
            .output()
            .map_err(io)?;
        if !identity.status.success()
            || !String::from_utf8_lossy(&identity.stdout)
                .trim()
                .eq_ignore_ascii_case(uuid)
        {
            return Err("LiDAR data volume identity mismatch".into());
        }
    }
    // Relative paths remain pinned to the opened volume through a lazy unmount.
    let root = Path::new(".");
    if options.contains_key("--verify") {
        let report = verify(root)?;
        println!("{report}");
        return if report["failures"].as_array().is_some_and(|f| !f.is_empty()) {
            Err("Archive audit failed".into())
        } else {
            Ok(())
        };
    }
    let mut planned = assets(root)?;
    if let Some(n) = options.get("--max-files") {
        let n = n.parse::<usize>().map_err(err)?;
        if n == 0 {
            return Err("--max-files must be positive".into());
        }
        planned.truncate(n);
    }
    if options.contains_key("--plan") {
        let bytes = planned
            .iter()
            .map(|a| a.source.metadata().map(|m| m.len()).map_err(io))
            .collect::<Result<Vec<_>>>()?
            .iter()
            .sum::<u64>();
        println!(
            "{}",
            serde_json::json!({"files":planned.len(),"source_bytes":bytes,"newest_first":true,"zoom":zoom,"raw_deletion":false})
        );
        return Ok(());
    }
    let folder = root.join("fused/v1");
    fs::create_dir_all(&folder).map_err(io)?;
    let lock = File::options()
        .create(true)
        .append(true)
        .open(folder.join("build.lock"))
        .map_err(io)?;
    fs2::FileExt::try_lock_exclusive(&lock)
        .map_err(|e| format!("Another conversion is running: {e}"))?;
    let db = Connection::open(folder.join("index.sqlite")).map_err(err)?;
    db.execute_batch(SCHEMA).map_err(err)?;
    for asset in planned {
        let start = Instant::now();
        let status = match prepare(root, &db, &asset, zoom) {
            Ok(status) => status.to_owned(),
            Err(e) if e.starts_with("I/O:") => return Err(e),
            Err(e) => {
                db.execute(
                    "INSERT OR REPLACE INTO failures VALUES (?1,?2,?3)",
                    params![
                        asset.url,
                        e,
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map_err(err)?
                            .as_secs_f64()
                    ],
                )
                .map_err(err)?;
                e
            }
        };
        println!(
            "{}",
            serde_json::json!({"survey":asset.survey,"status":status,"seconds":start.elapsed().as_secs_f64()})
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
