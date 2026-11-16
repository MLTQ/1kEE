//! Original EPT returns for radial inspection; no gridding or height synthesis.
use super::*;
use crate::region::Region;

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

#[derive(Clone, Debug)]
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
    if !region.valid() {
        return Err("Invalid Target region".into());
    }
    let [w, s, e, n] = region.bounds();
    let db = Connection::open_with_flags(catalog, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(err)?;
    let mut query = db
        .prepare(
            "SELECT id,coalesce(date,''),name FROM surveys
        WHERE id LIKE 'USGS_3DEP_ID:%' AND south<?3 AND north>?4
        AND ((west<?1 AND east>?2) OR (west<?1-360 AND east>?2-360)
            OR (west<?1+360 AND east>?2+360)) ORDER BY date DESC,id LIMIT 16",
        )
        .map_err(err)?;
    query
        .query_map(params![e, w, n, s], |r| {
            Ok(Source {
                id: r.get(0)?,
                date: r.get(1)?,
                name: r.get(2)?,
            })
        })
        .map_err(err)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(err)
}

/// Streams spatially intersecting octree nodes from coarse to original detail.
/// A limit produces an explicitly incomplete summary, preserving prior batches.
pub fn stream(
    catalog: &Path,
    region: Region,
    source: &Source,
    active: Option<&Path>,
    max_points: usize,
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
    };
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
    let mut summary = walk(
        &meta,
        region,
        active,
        max_points,
        &mut |relative| fetch.get(&format!("{base}/{relative}")),
        &mut emit,
    )?;
    summary.source = source.id.clone();
    summary.transferred_bytes = fetch.transferred;
    prune_cache(&cache)?;
    Ok(summary)
}

fn walk(
    meta: &Metadata,
    region: Region,
    active: Option<&Path>,
    max_points: usize,
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
        if summary.nodes >= 4096
            || summary.decoded >= 96_000_000
            || start.elapsed() > Duration::from_secs(170)
        {
            summary.complete = false;
            summary.limit = "Loading budget reached; refine to request more detail".into();
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
        let bytes = get(&format!("ept-data/{key}.laz"))?;
        let mut reader = las::Reader::with_options(
            Cursor::new(bytes),
            las::ReaderOptions::default().with_laz_parallelism(las::LazParallelism::No),
        )
        .map_err(err)?;
        if reader.header().number_of_points() != count as u64 {
            return Err("Point node count differs from hierarchy".into());
        }
        let mut seen = 0;
        let mut points = Vec::new();
        let mut limited = false;
        loop {
            check(active)?;
            let batch = reader.read_points(32_768).map_err(err)?;
            if batch.is_empty() {
                break;
            }
            seen += batch.len() as u64;
            for point in batch.points() {
                let p = point.map_err(err)?;
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
        if !limited && seen != count as u64 {
            return Err("Incomplete original point decode".into());
        }
        summary.decoded += seen;
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
            summary.limit = "Point budget reached; raise the budget and refine".into();
            break;
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
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
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
