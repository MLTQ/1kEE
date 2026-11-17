//! Original EPT returns for radial inspection; no gridding or height synthesis.
use super::*;
use crate::region::Region;
#[path = "point_budget.rs"]
pub(crate) mod budget;
#[path = "point_merge.rs"]
pub mod merge;
#[path = "point_sources.rs"]
mod point_sources;
use budget::StreamBudget;

/// Source record positions, not a lossy spatial deduplication grid. A later
/// pass resumes after exactly the last consumed source record.
#[derive(Default, Debug)]
pub(crate) struct StreamCursor {
    pub nodes: HashMap<String, u64>,
    pub bulk: u64,
    pub complete: bool,
}

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
pub struct Source {
    pub id: String,
    pub date: String,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct Point {
    /// Unmodified decoded coordinates in the EPT source reference.
    pub source_xyz: [f64; 3],
    /// Float64 geographic coordinates for selection and display reprojection.
    pub lon_lat: [f64; 2],
    pub classification: u8,
    pub intensity: u16,
    pub return_number: u8,
    pub rgb: Option<[u16; 3]>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Header {
    pub source: Source,
    pub crs: Value,
    pub publisher: String,
}

pub enum Event {
    Header(Header),
    Batch { points: Vec<Point>, spacing_m: f64 },
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Summary {
    pub points: usize,
    pub decoded: u64,
    pub nodes: usize,
    pub complete: bool,
    pub limit: String,
    pub source: String,
    pub transferred_bytes: u64,
}

/// These are candidate survey footprints. Actual returns determine coverage.
pub fn sources(catalog: &Path, region: Region) -> Result<Vec<Source>> {
    candidates(catalog, region).map(|(sources, _)| sources)
}
fn candidates(catalog: &Path, region: Region) -> Result<(Vec<Source>, bool)> {
    if !region.valid() {
        return Err("Invalid Target region".into());
    }
    let [w, s, e, n] = region.bounds();
    let db = Connection::open_with_flags(catalog, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(err)?;
    let mut query = db
        .prepare(
            "SELECT id,coalesce(date,''),name,CASE WHEN length(geometry)<=524288 THEN geometry ELSE '{}' END FROM surveys
        WHERE (id LIKE 'USGS_3DEP_ID:%' OR (id LIKE 'opentopoID:%' AND stac IS NOT NULL)) AND rowid IN
        (SELECT rowid FROM surveys WHERE south<?3 AND north>?4
        AND ((west<?1 AND east>?2) OR (west<?1-360 AND east>?2-360)
            OR (west<?1+360 AND east>?2+360)))
        ORDER BY CASE WHEN west<=?5 AND east>=?5 AND south<=?6 AND north>=?6 THEN 0 ELSE 1 END,date DESC,id LIMIT 257",
        )
        .map_err(err)?;
    let mut candidates = query
        .query_map(params![e, w, n, s, region.lon, region.lat], |r| {
            Ok((
                Source {
                    id: r.get(0)?,
                    date: r.get(1)?,
                    name: r.get(2)?,
                },
                r.get::<_, String>(3)?,
            ))
        })
        .map_err(err)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(err)?
        .into_iter()
        .map(|(source, geometry)| {
            let (center, mask) = point_sources::footprint_mask(&geometry, region);
            (u32::from(center) * 10_000 + mask.count_ones(), source, mask)
        })
        .collect::<Vec<_>>();
    // Bounding boxes can overlap a city while every return belongs to the
    // neighboring state. Prefer the actual center/radial footprint before date.
    candidates.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(b.1.date.cmp(&a.1.date))
            .then(a.1.id.cmp(&b.1.id))
    });
    let limited = candidates.len() > 64;
    let mut selected = Vec::new();
    let mut covered = 0u64;
    while !candidates.is_empty() && selected.len() < 64 {
        let index = if selected.is_empty() {
            0
        } else {
            let mut best = 0;
            let mut gain = 0;
            for (i, (_, _, mask)) in candidates.iter().enumerate() {
                let next = (mask & !covered).count_ones();
                if next > gain {
                    best = i;
                    gain = next;
                }
            }
            best
        };
        let (_, source, mask) = candidates.remove(index);
        covered |= mask;
        selected.push(source);
    }
    Ok((selected, limited))
}

/// Streams spatially intersecting octree nodes from coarse to original detail.
/// A limit produces an explicitly incomplete summary, preserving prior batches.
pub fn stream(
    catalog: &Path,
    region: Region,
    source: &Source,
    active: Option<&Path>,
    max_points: usize,
    emit: impl FnMut(Event) -> Result<()>,
) -> Result<Summary> {
    stream_with_transform(
        catalog,
        region,
        source,
        active,
        max_points,
        |_, _| Err("Projected LAS source requires a horizontal coordinate transformer".into()),
        emit,
    )
}

/// A projected LAS adapter supplies an XY-only transform. Original source XYZ,
/// including Z units/datum, is never passed through that transform.
pub fn stream_with_transform(
    catalog: &Path,
    region: Region,
    source: &Source,
    active: Option<&Path>,
    max_points: usize,
    project: impl FnMut(&str, &mut [[f64; 2]]) -> Result<()>,
    emit: impl FnMut(Event) -> Result<()>,
) -> Result<Summary> {
    stream_resume(
        catalog,
        region,
        source,
        active,
        max_points,
        &StreamBudget::default(),
        &mut StreamCursor::default(),
        project,
        emit,
    )
}
#[allow(clippy::too_many_arguments)]
fn stream_resume(
    catalog: &Path,
    region: Region,
    source: &Source,
    active: Option<&Path>,
    max_points: usize,
    resources: &StreamBudget,
    cursor: &mut StreamCursor,
    project: impl FnMut(&str, &mut [[f64; 2]]) -> Result<()>,
    mut emit: impl FnMut(Event) -> Result<()>,
) -> Result<Summary> {
    if !region.valid() || !(1..=16_000_000).contains(&max_points) {
        return Err("Invalid Target point budget or region".into());
    }
    let _permit = Permit::acquire(active)?;
    if !catalog.is_file() {
        return Err("LiDAR catalog is unavailable; check the configured data drive".into());
    }
    let root = catalog.parent().ok_or("Missing LiDAR catalog directory")?;
    if fs2::available_space(root).map_err(err)? < 10_000_000_000 + 2 * TRANSFER_LIMIT {
        return Err("Point loading paused at the 10 GB free-space reserve".into());
    }
    let cache = root.join("fused/v1/ept-cache");
    fs::create_dir_all(&cache).map_err(err)?;
    prune_cache(&cache)?;
    let mut fetch = Fetch {
        client: Client::builder()
            .user_agent("1kEE Target point viewer")
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(err)?,
        folder: &cache,
        active,
        transferred: 0,
        started: Instant::now(),
        budget: Some(resources),
    };
    if source.id.starts_with("opentopoID:") {
        let result = point_sources::stream_resume(
            catalog, region, source, active, max_points, &mut fetch, cursor, resources, project,
            &mut emit,
        );
        prune_cache(&cache)?;
        return result;
    }
    let id = source
        .id
        .strip_prefix("USGS_3DEP_ID:")
        .ok_or("Unsupported point source")?;
    let mut base = reqwest::Url::parse("https://s3-us-west-2.amazonaws.com/usgs-lidar-public/")
        .map_err(err)?;
    base.path_segments_mut()
        .map_err(|_| "Invalid point source URL")?
        .pop_if_empty()
        .push(id);
    let base = base.to_string();
    let meta: Metadata =
        serde_json::from_slice(&fetch.get(&format!("{base}/ept.json"))?).map_err(err)?;
    meta.validate()?;
    // Source Z is kept as-is. The viewer must separate an undeclared vertical
    // reference from its metre-scaled DEM rather than silently align the two.
    emit(Event::Header(Header {
        source: source.clone(),
        crs: meta.srs.clone(),
        publisher: format!("{base}/ept.json"),
    }))?;
    let mut summary = walk_resume(
        &meta,
        region,
        active,
        max_points,
        cursor,
        resources,
        &mut |relative| fetch.get(&format!("{base}/{relative}")),
        &mut emit,
    )?;
    summary.source = source.id.clone();
    summary.transferred_bytes = fetch.transferred;
    prune_cache(&cache)?;
    Ok(summary)
}

#[cfg(test)]
fn walk(
    meta: &Metadata,
    region: Region,
    active: Option<&Path>,
    max_points: usize,
    get: &mut impl FnMut(&str) -> Result<Vec<u8>>,
    emit: &mut impl FnMut(Event) -> Result<()>,
) -> Result<Summary> {
    walk_resume(
        meta,
        region,
        active,
        max_points,
        &mut StreamCursor::default(),
        &StreamBudget::default(),
        get,
        emit,
    )
}
#[allow(clippy::too_many_arguments)]
fn walk_resume(
    meta: &Metadata,
    region: Region,
    active: Option<&Path>,
    max_points: usize,
    cursor: &mut StreamCursor,
    resources: &StreamBudget,
    get: &mut impl FnMut(&str) -> Result<Vec<u8>>,
    emit: &mut impl FnMut(Event) -> Result<()>,
) -> Result<Summary> {
    let mercator = meta.mercator()?;
    let [w, s, e, n] = region.bounds();
    let (left, bottom) = native(w, s, mercator);
    let (right, top) = native(e, n, mercator);
    let [xmin, ymin, _, xmax, ymax, _] = meta.bounds;
    let mut hierarchy: HashMap<String, i64> =
        serde_json::from_slice(&get("ept-hierarchy/0-0-0-0.json")?).map_err(err)?;
    let mut queue = VecDeque::from([(0u32, 0u32, 0u32, 0u32)]);
    let mut summary = Summary {
        complete: true,
        ..Default::default()
    };
    let start = Instant::now();
    while let Some((depth, x, y, z)) = queue.pop_front() {
        check(active)?;
        if resources.limit().is_some() || start.elapsed() > Duration::from_secs(170) {
            summary.complete = false;
            summary.limit = resources.limit().unwrap_or("Loading budget reached").into();
            break;
        }
        let scale = 2f64.powi(depth as i32);
        let dx = (xmax - xmin) / scale;
        let dy = (ymax - ymin) / scale;
        let period = if mercator {
            std::f64::consts::TAU * 6378137.
        } else {
            360.
        };
        let intersects_x = [-period, 0., period].iter().any(|offset| {
            xmin + (x + 1) as f64 * dx >= left + offset && xmin + x as f64 * dx <= right + offset
        });
        if !intersects_x || ymin + (y + 1) as f64 * dy < bottom || ymin + y as f64 * dy > top {
            continue;
        }
        let key = format!("{depth}-{x}-{y}-{z}");
        let mut count = *hierarchy.get(&key).unwrap_or(&0);
        if count == -1 {
            let subtree: HashMap<String, i64> =
                serde_json::from_slice(&get(&format!("ept-hierarchy/{key}.json"))?).map_err(err)?;
            hierarchy.extend(subtree);
            count = *hierarchy
                .get(&key)
                .ok_or("Missing point hierarchy continuation")?;
        }
        if count == 0 {
            continue;
        }
        if !(1..=4_000_000).contains(&count) {
            return Err("Invalid/oversized point node".into());
        }
        let resume = cursor.nodes.get(&key).copied().unwrap_or(0);
        if resume < count as u64 {
            let bytes = get(&format!("ept-data/{key}.laz"))?;
            let mut reader = las::Reader::with_options(
                Cursor::new(bytes),
                las::ReaderOptions::default().with_laz_parallelism(las::LazParallelism::No),
            )
            .map_err(err)?;
            if reader.header().number_of_points() != count as u64 {
                return Err("Point node count differs from hierarchy".into());
            }
            reader.seek(resume).map_err(err)?;
            let mut seen = resume;
            resources.nodes.set(resources.nodes.get() + 1);
            let mut points = Vec::new();
            let mut limited = false;
            loop {
                check(active)?;
                let allowed = resources.remaining_decoded();
                if allowed == 0 {
                    limited = true;
                    break;
                }
                let batch = reader.read_points(allowed as u64).map_err(err)?;
                if batch.is_empty() {
                    break;
                }
                summary.decoded += batch.len() as u64;
                resources
                    .decoded
                    .set(resources.decoded.get() + batch.len() as u64);
                for point in batch.points() {
                    let p = point.map_err(err)?;
                    seen += 1;
                    if p.is_withheld || matches!(u8::from(p.classification), 7 | 18) {
                        continue;
                    }
                    let (lon, lat) = geographic(p.x, p.y, mercator);
                    if ![lon, lat, p.z].iter().all(|v| v.is_finite()) {
                        return Err("Invalid original point coordinate".into());
                    }
                    if !region.contains(lon, lat) {
                        continue;
                    }
                    if summary.points + points.len() >= max_points {
                        seen -= 1;
                        limited = true;
                        break;
                    }
                    points.push(Point {
                        source_xyz: [p.x, p.y, p.z],
                        lon_lat: [lon, lat],
                        classification: u8::from(p.classification),
                        intensity: p.intensity,
                        return_number: p.return_number,
                        rgb: p.color.map(|c| [c.red, c.green, c.blue]),
                    });
                }
                if limited {
                    break;
                }
            }
            cursor.nodes.insert(key.clone(), seen);
            if !limited && seen != count as u64 {
                return Err("Incomplete original point decode".into());
            }
            summary.nodes += 1;
            summary.points += points.len();
            if !points.is_empty() {
                // EPT spacing is in source XY units; EPSG:3857 is corrected for
                // latitude only for a descriptive density readout, never XYZ.
                let spacing = (xmax - xmin) / scale / f64::from(meta.span)
                    * if mercator {
                        region.lat.to_radians().cos().abs()
                    } else {
                        111_320.
                    };
                emit(Event::Batch {
                    points,
                    spacing_m: spacing,
                })?;
            }
            if limited {
                summary.complete = false;
                summary.limit = resources
                    .limit()
                    .unwrap_or("Point budget reached; raise the budget and refine")
                    .into();
                break;
            }
        }
        if depth < 22 {
            for dz in 0..2 {
                for dy in 0..2 {
                    for dx in 0..2 {
                        let child = (depth + 1, x * 2 + dx, y * 2 + dy, z * 2 + dz);
                        if hierarchy.contains_key(&format!(
                            "{}-{}-{}-{}",
                            child.0, child.1, child.2, child.3
                        )) {
                            queue.push_back(child);
                        }
                    }
                }
            }
        } else {
            summary.complete = false;
            summary.limit = "Maximum hierarchy depth reached".into();
        }
    }
    cursor.complete = summary.complete;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn regional_candidates_prefer_center_coverage_and_preserve_wrapped_footprints() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.sqlite");
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE surveys(id TEXT,name TEXT,date TEXT,west REAL,south REAL,east REAL,north REAL,geometry TEXT DEFAULT '{}',stac TEXT);
            CREATE INDEX surveys_extent ON surveys(west,east,south,north);
            INSERT INTO surveys(id,name,date,west,south,east,north) VALUES('USGS_3DEP_ID:fringe','fringe','2026',0.3,-0.1,0.6,0.1);
            INSERT INTO surveys(id,name,date,west,south,east,north) VALUES('USGS_3DEP_ID:center','center','2020',-0.1,-0.1,0.1,0.1);
            INSERT INTO surveys(id,name,date,west,south,east,north) VALUES('USGS_3DEP_ID:date-line','wrapped','2020',-181,-1,-179,1);").unwrap();
        for (id, w, e) in [("center", -0.1, 0.1), ("fringe", 0.3, 0.6)] {
            let geometry = json!({"type":"Polygon","coordinates":[[[w,-0.1],[e,-0.1],[e,0.1],[w,0.1],[w,-0.1]]]});
            db.execute(
                "UPDATE surveys SET geometry=?1 WHERE name=?2",
                params![geometry.to_string(), id],
            )
            .unwrap();
        }
        let candidates = sources(
            &path,
            Region {
                lon: 0.,
                lat: 0.,
                radius_m: 100_000.,
            },
        )
        .unwrap();
        assert_eq!(
            candidates
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["center", "fringe"]
        );
        assert_eq!(
            sources(
                &path,
                Region {
                    lon: 179.99,
                    lat: 0.,
                    radius_m: 100_000.
                }
            )
            .unwrap()[0]
                .name,
            "wrapped"
        );
    }
    #[test]
    fn merge_candidates_visit_complementary_footprints_before_repeated_center_surveys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.sqlite");
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE surveys(id TEXT,name TEXT,date TEXT,geometry TEXT,stac TEXT,west REAL,south REAL,east REAL,north REAL);").unwrap();
        for (id, date, w, e) in [
            ("center-new", "2026", -0.003, 0.003),
            ("center-old", "2025", -0.003, 0.003),
            ("east", "2020", 0.004, 0.02),
            ("west", "2020", -0.02, -0.004),
        ] {
            let geometry = json!({"type":"Polygon","coordinates":[[[w,-0.02],[e,-0.02],[e,0.02],[w,0.02],[w,-0.02]]]});
            db.execute(
                "INSERT INTO surveys VALUES(?1,?2,?3,?4,NULL,-0.03,-0.03,0.03,0.03)",
                params![format!("USGS_3DEP_ID:{id}"), id, date, geometry.to_string()],
            )
            .unwrap();
        }
        let sources = sources(
            &path,
            Region {
                lon: 0.,
                lat: 0.,
                radius_m: 1000.,
            },
        )
        .unwrap();
        assert_eq!(sources[0].name, "center-new");
        assert!(sources[1..3].iter().any(|s| s.name == "east"));
        assert!(sources[1..3].iter().any(|s| s.name == "west"));
        assert_eq!(sources[3].name, "center-old");
    }
    #[test]
    fn manhattan_uses_city_footprint_even_without_a_catalog_date() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.sqlite");
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE surveys(id TEXT,name TEXT,date TEXT,geometry TEXT,stac TEXT,west REAL,south REAL,east REAL,north REAL);").unwrap();
        let rows: Value =
            serde_json::from_str(include_str!("fixtures/manhattan-surveys.json")).unwrap();
        for row in rows.as_array().unwrap() {
            db.execute(
                "INSERT INTO surveys VALUES(?1,?2,?3,?4,NULL,-74.7,40.,-73.,42.)",
                params![
                    row["id"].as_str(),
                    row["name"].as_str(),
                    row["date"].as_str().unwrap_or(""),
                    row["geometry"].as_str()
                ],
            )
            .unwrap();
        }
        let region = Region {
            lon: -74.012680,
            lat: 40.709930,
            radius_m: 1000.,
        };
        let selected = sources(&path, region).unwrap();
        assert_eq!(selected[0].id, "USGS_3DEP_ID:NY_NewYorkCity");
        assert!(selected[0].date.is_empty());
    }
    #[test]
    #[ignore = "downloads original NC EPT points into a writable ONEKEE_TARGET_TEST_CATALOG cache"]
    fn live_target_nc_original_points() {
        let catalog = std::env::var_os("ONEKEE_TARGET_TEST_CATALOG")
            .map(std::path::PathBuf::from)
            .expect("catalog");
        let region = Region {
            lon: -77.58,
            lat: 35.26,
            radius_m: 1000.,
        };
        let sources = sources(&catalog, region).unwrap();
        let mut accepted = 0;
        let mut z = [f64::INFINITY, f64::NEG_INFINITY];
        let mut classes = std::collections::BTreeMap::<u8, usize>::new();
        for source in sources {
            let result = stream(&catalog, region, &source, None, 8_000_000, |event| {
                if let Event::Batch { points, .. } = event {
                    for p in &points {
                        assert!(region.contains(p.lon_lat[0], p.lon_lat[1]));
                        z = [z[0].min(p.source_xyz[2]), z[1].max(p.source_xyz[2])];
                        *classes.entry(p.classification).or_default() += 1;
                    }
                    accepted += points.len();
                    if accepted / 500_000 != (accepted - points.len()) / 500_000 {
                        eprintln!("Original NC returns: {accepted}");
                    }
                }
                Ok(())
            });
            match result {
                Ok(summary) if summary.points > 0 => {
                    assert!(summary.points > 10_000);
                    assert!(z[1] - z[0] > 1.);
                    std::fs::write(
                        "/tmp/1kee-target-native-nc.json",
                        serde_json::to_vec_pretty(
                            &json!({"summary":summary,"height_range_source":z,"classes":classes}),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                    return;
                }
                Ok(_) => {}
                Err(e) if accepted > 0 => panic!("partial source failure: {e}"),
                Err(e) => eprintln!("{}: {e}", source.id),
            }
        }
        panic!("No original returns loaded at Kinston");
    }
    #[test]
    fn radial_stream_retains_original_measurements_and_reports_limits() {
        let region = Region {
            lon: -77.58,
            lat: 35.26,
            radius_m: 1000.,
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("points.laz");
        let mut builder = las::Builder::from(las::Version::new(1, 4));
        builder.transforms = las::Vector {
            x: las::Transform {
                scale: 1e-9,
                offset: region.lon,
            },
            y: las::Transform {
                scale: 1e-9,
                offset: region.lat,
            },
            z: las::Transform {
                scale: 0.001,
                offset: 0.,
            },
        };
        let mut writer = las::Writer::from_path(&path, builder.into_header().unwrap()).unwrap();
        for (east, class, withheld) in [
            (0., 2, false),
            (999., 5, false),
            (1001., 2, false),
            (20., 7, false),
            (30., 2, true),
        ] {
            let [x, y] = region.location(east, 0.);
            writer
                .write_point(las::Point {
                    x,
                    y,
                    z: 123.456,
                    intensity: 1234,
                    return_number: 1,
                    number_of_returns: 1,
                    classification: las::point::Classification::new(class).unwrap(),
                    is_withheld: withheld,
                    ..Default::default()
                })
                .unwrap();
        }
        writer.close().unwrap();
        let bytes = fs::read(path).unwrap();
        let mut reader = las::Reader::new(Cursor::new(bytes.clone())).unwrap();
        let original = reader
            .read_points(5)
            .unwrap()
            .points()
            .map(|p| p.unwrap())
            .collect::<Vec<_>>();
        let meta = Metadata {
            bounds: [-77.6, 35.24, -100., -77.56, 35.28, 500.],
            data_type: "laszip".into(),
            span: 128,
            srs: json!({"horizontal":"4326","vertical":"5703"}),
        };
        let get = |name: &str| {
            Ok(if name.ends_with(".json") {
                br#"{"0-0-0-0":5}"#.to_vec()
            } else {
                bytes.clone()
            })
        };
        let mut received = Vec::new();
        let full = walk(&meta, region, None, 20, &mut get.clone(), &mut |e| {
            if let Event::Batch { points, .. } = e {
                received.extend(points);
            }
            Ok(())
        })
        .unwrap();
        assert!(full.complete);
        assert_eq!(full.points, 2);
        for (a, b) in received.iter().zip(original) {
            assert_eq!(a.source_xyz, [b.x, b.y, b.z]);
            assert_eq!(a.intensity, b.intensity);
            assert!(region.contains(a.lon_lat[0], a.lon_lat[1]));
        }
        let limited = walk(&meta, region, None, 1, &mut get.clone(), &mut |_| Ok(())).unwrap();
        assert!(!limited.complete);
        assert_eq!(limited.points, 1);
        assert!(limited.limit.contains("budget"));
        let mut cursor = StreamCursor::default();
        let resources = StreamBudget::default();
        let mut resumed = Vec::new();
        for quota in [1, 20] {
            walk_resume(
                &meta,
                region,
                None,
                quota,
                &mut cursor,
                &resources,
                &mut get.clone(),
                &mut |e| {
                    if let Event::Batch { points, .. } = e {
                        resumed.extend(points.into_iter().map(|p| p.source_xyz));
                    }
                    Ok(())
                },
            )
            .unwrap();
        }
        assert!(cursor.complete);
        assert_eq!(
            resumed,
            received.iter().map(|p| p.source_xyz).collect::<Vec<_>>()
        );
        let resources = StreamBudget {
            max_decoded: 1,
            ..Default::default()
        };
        let partial = walk_resume(
            &meta,
            region,
            None,
            20,
            &mut StreamCursor::default(),
            &resources,
            &mut get.clone(),
            &mut |_| Ok(()),
        )
        .unwrap();
        assert!(!partial.complete);
        assert_eq!(resources.decoded.get(), 1);
        assert!(
            walk(
                &meta,
                region,
                Some(Path::new("/missing-target-marker")),
                20,
                &mut get.clone(),
                &mut |_| Ok(())
            )
            .is_err()
        );
    }
}
