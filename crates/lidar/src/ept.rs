//! Bounded, view-resolution preparation of public USGS EPT surveys.
//! These cached overviews are separate from full-density archive receipts.
use crate::render::{SIZE, read_mean};
use reqwest::blocking::Client;
use rusqlite::{Connection, OpenFlags, params};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, VecDeque},
    fs::{self, File},
    io::{Cursor, Read, Write},
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tiff::{
    encoder::{Compression, DeflateLevel, TiffEncoder, colortype::RGBA32Float},
    tags::Tag,
};

type Result<T> = std::result::Result<T, String>;
const NODE_LIMIT: u64 = 32 * 1024 * 1024;
const TRANSFER_LIMIT: u64 = 256 * 1024 * 1024;
const CACHE_LIMIT: u64 = 1024 * 1024 * 1024;
static ACTIVE: AtomicUsize = AtomicUsize::new(0);
struct Permit;
impl Permit {
    fn acquire(active: Option<&Path>) -> Result<Self> {
        loop {
            check(active)?;
            let running = ACTIVE.load(Ordering::Acquire);
            if running < 2
                && ACTIVE
                    .compare_exchange(running, running + 1, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
            {
                return Ok(Self);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::AcqRel);
    }
}
const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS tiles (
 id INTEGER PRIMARY KEY AUTOINCREMENT, key TEXT UNIQUE NOT NULL,
 survey TEXT NOT NULL,date TEXT NOT NULL,path TEXT NOT NULL,
 west REAL,south REAL,east REAL,north REAL,zoom INTEGER,created REAL);
 CREATE INDEX IF NOT EXISTS tile_location ON tiles(west,east);";
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn check(active: Option<&Path>) -> Result<()> {
    if active.is_some_and(|p| !p.is_file()) {
        Err("Camera moved".into())
    } else {
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Metadata {
    bounds: [f64; 6],
    data_type: String,
    span: u32,
    srs: Value,
}
impl Metadata {
    fn mercator(&self) -> Result<bool> {
        let code = self.srs["horizontal"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| self.srs["horizontal"].to_string());
        match code.as_str() {
            "3857" => Ok(true),
            "4326" => Ok(false),
            _ => Err(format!("Unsupported EPT horizontal CRS EPSG:{code}")),
        }
    }
    fn validate(&self) -> Result<()> {
        if self.data_type != "laszip"
            || self.span == 0
            || self.span > 4096
            || !self.bounds.iter().all(|v| v.is_finite())
            || (0..3).any(|i| self.bounds[i] >= self.bounds[i + 3])
        {
            return Err("Invalid/unsupported EPT metadata".into());
        }
        self.mercator().map(|_| ())
    }
}
fn native(x: f64, y: f64, mercator: bool) -> (f64, f64) {
    if mercator {
        (
            6378137. * x.to_radians(),
            6378137.
                * (std::f64::consts::FRAC_PI_4 + y.to_radians() / 2.)
                    .tan()
                    .ln(),
        )
    } else {
        (x, y)
    }
}
fn geographic(x: f64, y: f64, mercator: bool) -> (f64, f64) {
    if mercator {
        (
            (x / 6378137.).to_degrees(),
            (2. * (y / 6378137.).exp().atan() - std::f64::consts::FRAC_PI_2).to_degrees(),
        )
    } else {
        (x, y)
    }
}

struct Samples {
    sums: Vec<f64>,
    counts: Vec<u32>,
    low: Vec<f32>,
    high: Vec<f32>,
    nodes: Vec<Value>,
    points: u64,
}
impl Samples {
    fn new() -> Self {
        Self {
            sums: vec![0.; SIZE * SIZE],
            counts: vec![0; SIZE * SIZE],
            low: vec![f32::INFINITY; SIZE * SIZE],
            high: vec![f32::NEG_INFINITY; SIZE * SIZE],
            nodes: Vec::new(),
            points: 0,
        }
    }
    fn bands(&self) -> Vec<f32> {
        (0..SIZE * SIZE)
            .flat_map(|i| {
                if self.counts[i] == 0 {
                    [f32::NAN; 4]
                } else {
                    [
                        self.low[i],
                        self.high[i],
                        (self.sums[i] / self.counts[i] as f64) as f32,
                        self.counts[i] as f32,
                    ]
                }
            })
            .collect()
    }
}

fn sample(
    meta: &Metadata,
    bounds: [f64; 4],
    active: Option<&Path>,
    get: &mut impl FnMut(&str) -> Result<Vec<u8>>,
) -> Result<Samples> {
    meta.validate()?;
    let [w, s, e, n] = bounds;
    let mercator = meta.mercator()?;
    let (left, bottom) = native(w, s, mercator);
    let (right, top) = native(e, n, mercator);
    let [xmin, ymin, _, xmax, ymax, _] = meta.bounds;
    let target = (right - left).max(top - bottom) / SIZE as f64;
    // One additional level gives several measured returns per display cell.
    let depth_limit =
        (((xmax - xmin) / (meta.span as f64 * target)).log2().ceil() + 1.).clamp(0., 22.) as u32;
    let mut hierarchy: HashMap<String, i64> =
        serde_json::from_slice(&get("ept-hierarchy/0-0-0-0.json")?).map_err(err)?;
    let mut queue = VecDeque::from([(0u32, 0u32, 0u32, 0u32)]);
    let mut result = Samples::new();
    let started = Instant::now();
    while let Some((depth, x, y, z)) = queue.pop_front() {
        check(active)?;
        if started.elapsed() > Duration::from_secs(180) || result.nodes.len() >= 2048 {
            return Err("Lidar preparation limit reached; zoom in and retry".into());
        }
        let width = (xmax - xmin) / (1u32 << depth) as f64;
        let height = (ymax - ymin) / (1u32 << depth) as f64;
        if xmin + x as f64 * width > right
            || xmin + (x + 1) as f64 * width < left
            || ymin + y as f64 * height > top
            || ymin + (y + 1) as f64 * height < bottom
        {
            continue;
        }
        let node = format!("{depth}-{x}-{y}-{z}");
        let mut count = *hierarchy.get(&node).unwrap_or(&0);
        if count == -1 {
            let subtree: HashMap<String, i64> =
                serde_json::from_slice(&get(&format!("ept-hierarchy/{node}.json"))?)
                    .map_err(err)?;
            hierarchy.extend(subtree);
            count = *hierarchy
                .get(&node)
                .ok_or("Missing EPT hierarchy continuation")?;
        }
        if count == 0 {
            continue;
        }
        if !(1..=4_000_000).contains(&count) {
            return Err("Invalid/oversized EPT node".into());
        }
        if result.points + count as u64 > 64_000_000 {
            return Err("Lidar point limit reached; zoom in and retry".into());
        }
        let bytes = get(&format!("ept-data/{node}.laz"))?;
        let sha = format!("{:x}", Sha256::digest(&bytes));
        let mut reader = las::Reader::with_options(
            Cursor::new(bytes),
            las::ReaderOptions::default().with_laz_parallelism(las::LazParallelism::No),
        )
        .map_err(err)?;
        if reader.header().number_of_points() != count as u64 {
            return Err("EPT node count differs from hierarchy".into());
        }
        let mut seen = 0;
        loop {
            check(active)?;
            let points = reader.read_points(32_768).map_err(err)?;
            if points.is_empty() {
                break;
            }
            seen += points.len() as u64;
            for point in points.points() {
                let p = point.map_err(err)?;
                if p.is_withheld || matches!(u8::from(p.classification), 7 | 18) {
                    continue;
                }
                let (lon, lat) = geographic(p.x, p.y, mercator);
                if !lon.is_finite()
                    || !lat.is_finite()
                    || !p.z.is_finite()
                    || p.z.abs() > f32::MAX as f64
                {
                    return Err("Invalid EPT measurement".into());
                }
                let col = ((lon - w) * SIZE as f64 / (e - w)).floor();
                let row = ((n - lat) * SIZE as f64 / (n - s)).floor();
                if col < 0. || col >= SIZE as f64 || row < 0. || row >= SIZE as f64 {
                    continue;
                }
                let i = row as usize * SIZE + col as usize;
                result.counts[i] = result.counts[i]
                    .checked_add(1)
                    .filter(|v| *v <= 1 << 24)
                    .ok_or("EPT cell count overflow")?;
                result.sums[i] += p.z;
                result.low[i] = result.low[i].min(p.z as f32);
                result.high[i] = result.high[i].max(p.z as f32);
            }
        }
        if seen != count as u64 {
            return Err("Incomplete EPT node decode".into());
        }
        result.points += seen;
        result
            .nodes
            .push(json!({"node":node,"points":seen,"sha256":sha}));
        if depth < depth_limit {
            for a in 0..2 {
                for b in 0..2 {
                    for c in 0..2 {
                        queue.push_back((depth + 1, 2 * x + a, 2 * y + b, 2 * z + c));
                    }
                }
            }
        }
    }
    Ok(result)
}

fn write_raster(path: &Path, bounds: [f64; 4], samples: &Samples, manifest: &Value) -> Result<()> {
    let [w, s, e, n] = bounds;
    let mut encoder = TiffEncoder::new(File::create(path).map_err(err)?)
        .map_err(err)?
        .with_compression(Compression::Deflate(DeflateLevel::Balanced));
    let mut image = encoder
        .new_image::<RGBA32Float>(SIZE as u32, SIZE as u32)
        .map_err(err)?;
    image
        .encoder()
        .write_tag(
            Tag::ModelPixelScaleTag,
            &[(e - w) / SIZE as f64, (n - s) / SIZE as f64, 0.][..],
        )
        .map_err(err)?;
    image
        .encoder()
        .write_tag(Tag::ModelTiepointTag, &[0., 0., 0., w, n, 0.][..])
        .map_err(err)?;
    image
        .encoder()
        .write_tag(
            Tag::GeoKeyDirectoryTag,
            &[
                1u16, 1, 0, 3, 1024, 0, 1, 2, 1025, 0, 1, 1, 2048, 0, 1, 4326,
            ][..],
        )
        .map_err(err)?;
    image
        .encoder()
        .write_tag(Tag::GdalNodata, "nan")
        .map_err(err)?;
    image
        .encoder()
        .write_tag(
            Tag::ImageDescription,
            serde_json::to_string(manifest).map_err(err)?.as_str(),
        )
        .map_err(err)?;
    image.write_data(&samples.bands()).map_err(err)?;
    // Verify the actual numeric mean plane with the same reader used by Local.
    let values = read_mean(path, bounds)?;
    for (i, value) in values.into_iter().enumerate() {
        let expected = if samples.counts[i] == 0 {
            f32::NAN
        } else {
            (samples.sums[i] / samples.counts[i] as f64) as f32
        };
        if value != expected && !(value.is_nan() && expected.is_nan()) {
            return Err("EPT numeric round-trip mismatch".into());
        }
    }
    Ok(())
}

struct Fetch<'a> {
    client: Client,
    folder: &'a Path,
    active: Option<&'a Path>,
    transferred: u64,
    started: Instant,
}
impl Fetch<'_> {
    fn get(&mut self, url: &str) -> Result<Vec<u8>> {
        check(self.active)?;
        if self.started.elapsed() > Duration::from_secs(180) {
            return Err("Lidar preparation limit reached; zoom in and retry".into());
        }
        let path = self
            .folder
            .join(format!("{:x}", Sha256::digest(url.as_bytes())));
        if let Ok(meta) = path.metadata()
            && meta.len() <= NODE_LIMIT
            && meta
                .modified()
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age < Duration::from_secs(30 * 86400))
            && let Ok(data) = fs::read(&path)
        {
            return Ok(data);
        }
        let response = self
            .client
            .get(url)
            .send()
            .map_err(err)?
            .error_for_status()
            .map_err(err)?;
        let mut data = Vec::new();
        response
            .take(NODE_LIMIT + 1)
            .read_to_end(&mut data)
            .map_err(err)?;
        self.transferred += data.len() as u64;
        if data.len() as u64 > NODE_LIMIT || self.transferred > TRANSFER_LIMIT {
            return Err("Lidar download limit reached; zoom in and retry".into());
        }
        check(self.active)?;
        let mut temp = tempfile::NamedTempFile::new_in(self.folder).map_err(err)?;
        temp.write_all(&data).map_err(err)?;
        temp.persist(path).map_err(err)?;
        Ok(data)
    }
}

fn prune_cache(folder: &Path) -> Result<()> {
    let mut files = fs::read_dir(folder)
        .map_err(err)?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let meta = entry.metadata().ok()?;
            (meta.is_file() && entry.file_name().to_string_lossy().len() == 64).then_some((
                meta.modified().ok()?,
                meta.len(),
                entry.path(),
            ))
        })
        .collect::<Vec<_>>();
    let mut size: u64 = files.iter().map(|f| f.1).sum();
    files.sort_by_key(|f| f.0);
    for (_, bytes, path) in files {
        if size <= CACHE_LIMIT {
            break;
        }
        if fs::remove_file(path).is_ok() {
            size -= bytes;
        }
    }
    Ok(())
}

/// Prepare only intersecting USGS surveys. Source blocks are a bounded download
/// cache; overview receipts never authorize removing full-resolution raw data.
pub fn prepare(catalog: &Path, bounds: [f64; 4], zoom: u8, active: Option<&Path>) -> Result<bool> {
    let [w, s, e, n] = bounds;
    if !(5..=16).contains(&zoom)
        || !bounds.iter().all(|v| v.is_finite())
        || w < -180.
        || e > 180.
        || s <= -85.
        || n >= 85.
        || w >= e
        || s >= n
    {
        return Err("Unsupported EPT view bounds".into());
    }
    check(active)?;
    let root = catalog.parent().ok_or("Missing lidar root")?;
    let db = Connection::open_with_flags(catalog, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(err)?;
    let mut query = db
        .prepare(
            "SELECT id,coalesce(date,'') FROM surveys WHERE id LIKE 'USGS_3DEP_ID:%'
        AND west<?1 AND east>?2 AND south<?3 AND north>?4 ORDER BY date DESC,id LIMIT 8",
        )
        .map_err(err)?;
    let surveys = query
        .query_map(params![e, w, n, s], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(err)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(err)?;
    if surveys.is_empty() {
        return Ok(false);
    }
    let _permit = Permit::acquire(active)?;
    if fs2::available_space(root).map_err(err)? < 10_000_000_000 + 2 * TRANSFER_LIMIT {
        return Err("Lidar preparation paused at 10 GB free-space reserve".into());
    }
    let folder = root.join("fused/v1/previews");
    let cache = root.join("fused/v1/ept-cache");
    fs::create_dir_all(&folder).map_err(err)?;
    fs::create_dir_all(&cache).map_err(err)?;
    prune_cache(&cache)?;
    let index_path = root.join("fused/v1/previews.sqlite");
    if !index_path.is_file() {
        // Readers must never observe a new database before its schema exists.
        let temporary = tempfile::NamedTempFile::new_in(root.join("fused/v1")).map_err(err)?;
        let initial = Connection::open(temporary.path()).map_err(err)?;
        initial.execute_batch(SCHEMA).map_err(err)?;
        drop(initial);
        if let Err(error) = temporary.persist_noclobber(&index_path)
            && error.error.kind() != std::io::ErrorKind::AlreadyExists
        {
            return Err(err(error));
        }
    }
    let index = Connection::open(index_path).map_err(err)?;
    index.busy_timeout(Duration::from_secs(10)).map_err(err)?;
    index.execute_batch(SCHEMA).map_err(err)?;
    let client = Client::builder()
        .user_agent("1kEE lidar preparation")
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(err)?;
    let mut fetch = Fetch {
        client,
        folder: &cache,
        active,
        transferred: 0,
        started: Instant::now(),
    };
    let mut covered = vec![false; SIZE * SIZE];
    let mut prepared = false;
    for (survey, date) in surveys {
        check(active)?;
        let key = format!("{survey}/{date}/{zoom}/{w}/{s}/{e}/{n}/v1");
        let name = format!("{:x}.tif", Sha256::digest(key.as_bytes()));
        let output = folder.join(name);
        let exists = index
            .query_row("SELECT count(*) FROM tiles WHERE key=?1", [&key], |r| {
                r.get::<_, u32>(0)
            })
            .map_err(err)?
            > 0;
        if !exists || !output.is_file() {
            let id = survey.strip_prefix("USGS_3DEP_ID:").unwrap();
            let mut base =
                reqwest::Url::parse("https://s3-us-west-2.amazonaws.com/usgs-lidar-public/")
                    .map_err(err)?;
            base.path_segments_mut()
                .map_err(|_| "Invalid EPT base URL")?
                .pop_if_empty()
                .push(id);
            let base = base.to_string();
            let metadata = fetch.get(&format!("{base}/ept.json"))?;
            let meta: Metadata = serde_json::from_slice(&metadata).map_err(err)?;
            let samples = sample(&meta, bounds, active, &mut |relative| {
                fetch.get(&format!("{base}/{relative}"))
            })?;
            let manifest = json!({"product":"view-resolution EPT overview; not full-density archive",
                "bands":["min","max","mean","count"],"survey":survey,"acquisition":date,
                "publisher":format!("{base}/ept.json"),"source_crs":meta.srs,"bounds":bounds,
                "zoom":zoom,"nodes":samples.nodes,"vertical_values":"original source Z, no datum conversion"});
            let temporary = tempfile::NamedTempFile::new_in(&folder).map_err(err)?;
            write_raster(temporary.path(), bounds, &samples, &manifest)?;
            check(active)?;
            temporary.as_file().sync_all().map_err(err)?;
            temporary.persist(&output).map_err(err)?;
            index.execute("INSERT OR REPLACE INTO tiles(key,survey,date,path,west,south,east,north,zoom,created)
                VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![key,survey,date,
                output.strip_prefix(root).map_err(err)?.to_string_lossy(),w,s,e,n,zoom,
                SystemTime::now().duration_since(UNIX_EPOCH).map_err(err)?.as_secs_f64()]).map_err(err)?;
        }
        let values = read_mean(&output, bounds)?;
        for (dest, src) in covered.iter_mut().zip(values) {
            *dest |= src.is_finite();
        }
        prepared |= covered.iter().any(|v| *v);
        // Water/flight-line gaps stay transparent. Avoid downloading historical
        // surveys merely to chase a small number of unmeasured cells.
        if covered.iter().filter(|v| **v).count() * 100 >= SIZE * SIZE * 90 {
            break;
        }
    }
    prune_cache(&cache)?;
    Ok(prepared)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(mercator: bool) -> (Metadata, [f64; 4], HashMap<String, Vec<u8>>) {
        let bounds = [-88.4, 37.1, -88.3, 37.2];
        let (w, s) = native(bounds[0], bounds[1], mercator);
        let (e, n) = native(bounds[2], bounds[3], mercator);
        let meta = Metadata {
            bounds: [w, s, -100., e, n, 500.],
            data_type: "laszip".into(),
            span: 128,
            srs: json!({"horizontal":if mercator {"3857"} else {"4326"}}),
        };
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("node.laz");
        let mut builder = las::Builder::from(las::Version::new(1, 4));
        builder.transforms = las::Vector {
            x: las::Transform {
                scale: if mercator { 0.001 } else { 1e-9 },
                offset: w,
            },
            y: las::Transform {
                scale: if mercator { 0.001 } else { 1e-9 },
                offset: s,
            },
            z: las::Transform {
                scale: 0.001,
                offset: 0.,
            },
        };
        let mut writer = las::Writer::from_path(&path, builder.into_header().unwrap()).unwrap();
        // All five points land in one cell: only the first two are measurements.
        for (height, class, withheld) in [
            (10., 2, false),
            (20., 2, false),
            (800., 7, false),
            (900., 18, false),
            (700., 2, true),
        ] {
            let (x, y) = native(
                bounds[0] + (100.25 / 512.) * 0.1,
                bounds[3] - (200.25 / 512.) * 0.1,
                mercator,
            );
            writer
                .write_point(las::Point {
                    x,
                    y,
                    z: height,
                    classification: las::point::Classification::new(class).unwrap(),
                    is_withheld: withheld,
                    ..Default::default()
                })
                .unwrap();
        }
        writer.close().unwrap();
        // Root continuation exercises EPT hierarchy pagination, with no children.
        let files = HashMap::from([
            (
                "ept-hierarchy/0-0-0-0.json".into(),
                br#"{"0-0-0-0":5,"1-0-1-0":-1}"#.to_vec(),
            ),
            (
                "ept-hierarchy/1-0-1-0.json".into(),
                br#"{"1-0-1-0":5}"#.to_vec(),
            ),
            ("ept-data/0-0-0-0.laz".into(), fs::read(&path).unwrap()),
            ("ept-data/1-0-1-0.laz".into(), fs::read(&path).unwrap()),
        ]);
        (meta, bounds, files)
    }

    #[test]
    fn native_ept_hierarchy_projection_noise_filter_and_numeric_raster_round_trip() {
        for mercator in [false, true] {
            let (meta, bounds, files) = fixture(mercator);
            let samples = sample(&meta, bounds, None, &mut |key| {
                files.get(key).cloned().ok_or(key.into())
            })
            .unwrap();
            assert_eq!(samples.nodes.len(), 2);
            assert_eq!(samples.points, 10);
            assert_eq!(samples.counts.iter().sum::<u32>(), 4);
            let cell = 200 * SIZE + 100;
            assert_eq!(samples.counts[cell], 4);
            assert_eq!(
                (samples.low[cell], samples.high[cell], samples.sums[cell]),
                (10., 20., 60.)
            );
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("overview.tif");
            write_raster(&path, bounds, &samples, &json!({"product":"EPT overview"})).unwrap();
            let values = read_mean(&path, bounds).unwrap();
            assert_eq!(values[cell], 15.);
            assert_eq!(values.iter().filter(|v| v.is_finite()).count(), 1);
            let mut decoder = tiff::decoder::Decoder::new(File::open(&path).unwrap()).unwrap();
            let tiff::decoder::DecodingResult::F32(stored) = decoder.read_image().unwrap() else {
                panic!("float32 bands")
            };
            assert_eq!(&stored[4 * cell..4 * cell + 4], &[10., 20., 15., 4.]);
        }
    }

    #[test]
    fn incomplete_nodes_and_cancelled_work_cannot_be_published() {
        let (meta, bounds, mut files) = fixture(true);
        files.insert(
            "ept-hierarchy/0-0-0-0.json".into(),
            br#"{"0-0-0-0":6}"#.to_vec(),
        );
        assert!(
            sample(&meta, bounds, None, &mut |key| Ok(files[key].clone()))
                .err()
                .unwrap()
                .contains("count differs")
        );
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("cancelled");
        assert_eq!(
            sample(&meta, bounds, Some(&marker), &mut |key| Ok(
                files[key].clone()
            ))
            .err()
            .unwrap(),
            "Camera moved"
        );
    }

    #[test]
    fn overview_only_archive_is_visible_without_full_density_receipts() {
        let (meta, bounds, files) = fixture(false);
        let samples = sample(&meta, bounds, None, &mut |key| Ok(files[key].clone())).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let folder = root.join("fused/v1");
        fs::create_dir_all(&folder).unwrap();
        let catalog = root.join("catalog.sqlite");
        Connection::open(&catalog)
            .unwrap()
            .execute_batch(
                "CREATE TABLE surveys(id TEXT,name TEXT,page TEXT);
            INSERT INTO surveys VALUES('USGS_3DEP_ID:test','Test survey','https://example.org');",
            )
            .unwrap();
        let index = Connection::open(folder.join("previews.sqlite")).unwrap();
        index.execute_batch(SCHEMA).unwrap();
        write_raster(&folder.join("overview.tif"), bounds, &samples, &json!({})).unwrap();
        index
            .execute(
                "INSERT INTO tiles(key,survey,date,path,west,south,east,north,zoom,created)
            VALUES('tile','USGS_3DEP_ID:test','2026','fused/v1/overview.tif',?1,?2,?3,?4,10,0)",
                params![bounds[0], bounds[1], bounds[2], bounds[3]],
            )
            .unwrap();
        let image = crate::render::render_prepared(&catalog, bounds, None)
            .unwrap()
            .unwrap();
        assert_eq!(image.rgba[(200 * SIZE + 100) * 4 + 3], 255);
        assert_eq!(image.credits[0][0], "Test survey");
        assert_eq!(image.overview_zoom, Some(10));
        assert!(
            crate::render::render_prepared_detail(&catalog, bounds, 12, None)
                .unwrap()
                .is_none()
        );
        assert!(!folder.join("index.sqlite").exists());

        // A full-density raster wins at the same acquisition date. A newer
        // acquisition overview still takes precedence over the older scan.
        let full = Connection::open(folder.join("index.sqlite")).unwrap();
        full.execute_batch("CREATE TABLE rasters(url TEXT,survey TEXT,date TEXT,path TEXT,west REAL,south REAL,east REAL,north REAL);").unwrap();
        full.execute("INSERT INTO rasters VALUES('full','USGS_3DEP_ID:test','2026','fused/v1/full.tif',?1,?2,?3,?4)",params![bounds[0],bounds[1],bounds[2],bounds[3]]).unwrap();
        let mut full_samples = Samples::new();
        let cell = 200 * SIZE + 100;
        full_samples.counts[cell] = 1;
        full_samples.sums[cell] = 40.;
        full_samples.low[cell] = 40.;
        full_samples.high[cell] = 40.;
        write_raster(&folder.join("full.tif"), bounds, &full_samples, &json!({})).unwrap();
        let full_image = crate::render::render_prepared(&catalog, bounds, None)
            .unwrap()
            .unwrap();
        assert_eq!(full_image.overview_zoom, None);
        assert_eq!(
            crate::render::render_prepared_detail(&catalog, bounds, 12, None)
                .unwrap()
                .unwrap()
                .rgba,
            full_image.rgba
        );
        assert_ne!(
            &full_image.rgba[cell * 4..cell * 4 + 3],
            &image.rgba[cell * 4..cell * 4 + 3]
        );
        index.execute("UPDATE tiles SET date='2027'", []).unwrap();
        let newer = crate::render::render_prepared(&catalog, bounds, None)
            .unwrap()
            .unwrap();
        assert_eq!(newer.rgba, image.rgba);
        assert_eq!(newer.overview_zoom, Some(10));
        assert_eq!(
            crate::render::render_prepared_detail(&catalog, bounds, 12, None)
                .unwrap()
                .unwrap()
                .rgba,
            full_image.rgba,
            "a newer coarse overview must not mask available detailed data"
        );
    }
}
