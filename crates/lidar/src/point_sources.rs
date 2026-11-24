//! Published bulk LAS/LAZ access and actual-footprint ranking for Target.
use super::*;

pub(super) fn sample_points(region: Region) -> Vec<[f64; 2]> {
    let mut pts = Vec::with_capacity(49);
    for x in -4i32..=4 {
        for y in -4i32..=4 {
            if x * x + y * y > 16 {
                continue;
            }
            pts.push(region.location(
                x as f64 * region.radius_m / 4.,
                y as f64 * region.radius_m / 4.,
            ));
        }
    }
    pts
}

struct ParsedRing {
    points: Vec<(f64, f64)>,
    min_lat: f64,
    max_lat: f64,
}

impl ParsedRing {
    fn from_value(v: &Value) -> Option<Self> {
        let arr = v.as_array()?;
        let mut points = Vec::with_capacity(arr.len());
        let mut min_lat = f64::INFINITY;
        let mut max_lat = f64::NEG_INFINITY;
        for pt in arr {
            if let (Some(x), Some(y)) = (pt[0].as_f64(), pt[1].as_f64()) {
                min_lat = min_lat.min(y);
                max_lat = max_lat.max(y);
                points.push((x, y));
            }
        }
        (!points.is_empty()).then_some(Self {
            points,
            min_lat,
            max_lat,
        })
    }

    fn contains(&self, lon: f64, lat: f64) -> bool {
        if lat < self.min_lat || lat > self.max_lat || self.points.is_empty() {
            return false;
        }
        let mut inside = false;
        let n = self.points.len();
        let mut prev = self.points[n - 1];
        for &cur in &self.points {
            let (ax, ay) = prev;
            let (bx, by) = cur;
            prev = cur;
            if (ay > lat) != (by > lat) {
                let ax = lon + (ax - lon + 180.).rem_euclid(360.) - 180.;
                let bx = lon + (bx - lon + 180.).rem_euclid(360.) - 180.;
                if lon < ax + (bx - ax) * (lat - ay) / (by - ay) {
                    inside = !inside;
                }
            }
        }
        inside
    }
}

struct ParsedPolygon {
    outer: ParsedRing,
    holes: Vec<ParsedRing>,
}

impl ParsedPolygon {
    fn from_value(v: &Value) -> Option<Self> {
        let rings = v.as_array()?;
        let outer = ParsedRing::from_value(rings.first()?)?;
        let holes = rings
            .iter()
            .skip(1)
            .filter_map(ParsedRing::from_value)
            .collect();
        Some(Self { outer, holes })
    }

    fn contains(&self, lon: f64, lat: f64) -> bool {
        self.outer.contains(lon, lat) && !self.holes.iter().any(|h| h.contains(lon, lat))
    }
}

fn collect_polygons(g: &Value, out: &mut Vec<ParsedPolygon>) {
    match g["type"].as_str() {
        Some("Polygon") => {
            if let Some(poly) = ParsedPolygon::from_value(&g["coordinates"]) {
                out.push(poly);
            }
        }
        Some("MultiPolygon") => {
            if let Some(polys) = g["coordinates"].as_array() {
                for p in polys {
                    if let Some(poly) = ParsedPolygon::from_value(p) {
                        out.push(poly);
                    }
                }
            }
        }
        Some("GeometryCollection") => {
            if let Some(geoms) = g["geometries"].as_array() {
                for sub in geoms {
                    collect_polygons(sub, out);
                }
            }
        }
        _ => {}
    }
}

pub(super) fn footprint_mask_with_samples(
    geometry: &str,
    center_lon: f64,
    center_lat: f64,
    samples: &[[f64; 2]],
) -> (bool, u64) {
    let Ok(g) = serde_json::from_str::<Value>(geometry) else {
        return (false, 0);
    };
    let mut polygons = Vec::new();
    collect_polygons(&g, &mut polygons);
    if polygons.is_empty() {
        return (false, 0);
    }
    let contains = |lon: f64, lat: f64| polygons.iter().any(|p| p.contains(lon, lat));
    let center = contains(center_lon, center_lat);
    let mut mask = 0u64;
    for (bit, &[lon, lat]) in samples.iter().enumerate() {
        if contains(lon, lat) {
            mask |= 1u64 << bit;
        }
    }
    (center, mask)
}

#[cfg(test)]
pub(super) fn footprint_mask(geometry: &str, region: Region) -> (bool, u64) {
    let samples = sample_points(region);
    footprint_mask_with_samples(geometry, region.lon, region.lat, &samples)
}
#[cfg(test)]
fn coverage(geometry: &str, region: Region) -> u32 {
    let (center, mask) = footprint_mask(geometry, region);
    u32::from(center) * 10_000 + mask.count_ones()
}

fn collection(catalog: &Path, source: &Source) -> Result<(String, String)> {
    let db = Connection::open_with_flags(catalog, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(err)?;
    let (stac, page): (String, String) = db
        .query_row(
            "SELECT stac,page FROM surveys WHERE id=?1",
            [&source.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(err)?;
    let url = reqwest::Url::parse(&stac).map_err(err)?;
    let slug = url
        .path()
        .strip_prefix("/stac/")
        .and_then(|s| s.strip_suffix("_collection.json"))
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 120
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
        .ok_or("No supported public OpenTopography bulk collection is linked")?;
    if url.scheme() != "https" || url.host_str() != Some("portal.opentopography.org") {
        return Err("Unsupported OpenTopography metadata host".into());
    }
    Ok((slug.to_string(), page))
}
fn assets(xml: &str, slug: &str) -> Result<Vec<String>> {
    let doc = roxmltree::Document::parse(xml).map_err(err)?;
    if doc
        .descendants()
        .any(|n| n.has_tag_name("IsTruncated") && n.text() == Some("true"))
    {
        return Err(
            "This bulk collection needs a spatial tile index before Target can stream it".into(),
        );
    }
    let mut files = Vec::new();
    for node in doc.descendants().filter(|n| n.has_tag_name("Contents")) {
        let field = |key| {
            node.children()
                .find(|n| n.has_tag_name(key))
                .and_then(|n| n.text())
                .unwrap_or("")
        };
        let key = field("Key");
        if !key.to_ascii_lowercase().ends_with(".laz")
            && !key.to_ascii_lowercase().ends_with(".las")
        {
            continue;
        }
        if !key.starts_with(&format!("{slug}/")) || key.split('/').any(|s| s == "..") {
            return Err("Invalid bulk point path".into());
        }
        let size = field("Size").parse::<u64>().map_err(err)?;
        if size > NODE_LIMIT || files.len() >= 32 {
            return Err(
                "Bulk point files exceed the bounded reader; a spatial LAS/COPC index is needed"
                    .into(),
            );
        }
        let mut url =
            reqwest::Url::parse("https://opentopography.s3.sdsc.edu/pc-bulk/").map_err(err)?;
        url.path_segments_mut()
            .map_err(|_| "Invalid bulk URL")?
            .pop_if_empty()
            .extend(key.split('/'));
        files.push(url.into());
    }
    if files.is_empty() {
        return Err("No public LAS/LAZ files are linked for this survey".into());
    }
    Ok(files)
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(super) fn stream(
    catalog: &Path,
    region: Region,
    source: &Source,
    active: Option<&Path>,
    max_points: usize,
    fetch: &mut Fetch<'_>,
    project: impl FnMut(&str, &mut [[f64; 2]]) -> Result<()>,
    emit: &mut impl FnMut(Event) -> Result<()>,
) -> Result<Summary> {
    stream_resume(
        catalog,
        region,
        source,
        active,
        max_points,
        fetch,
        &mut StreamCursor::default(),
        &StreamBudget::default(),
        project,
        emit,
    )
}
#[allow(clippy::too_many_arguments)]
pub(super) fn stream_resume(
    catalog: &Path,
    region: Region,
    source: &Source,
    active: Option<&Path>,
    max_points: usize,
    fetch: &mut Fetch<'_>,
    cursor: &mut StreamCursor,
    resources: &StreamBudget,
    mut project: impl FnMut(&str, &mut [[f64; 2]]) -> Result<()>,
    emit: &mut impl FnMut(Event) -> Result<()>,
) -> Result<Summary> {
    let (slug, page) = collection(catalog, source)?;
    let mut listing =
        reqwest::Url::parse("https://opentopography.s3.sdsc.edu/pc-bulk").map_err(err)?;
    listing
        .query_pairs_mut()
        .append_pair("list-type", "2")
        .append_pair("prefix", &format!("{slug}/"))
        .append_pair("max-keys", "1000");
    let xml = fetch.get(listing.as_str())?;
    let urls = assets(std::str::from_utf8(&xml).map_err(err)?, &slug)?;
    if urls.len() != 1 {
        return Err("This multi-file bulk collection needs a spatial LAS/COPC index before Target can stream it".into());
    }
    let mut summary = Summary {
        complete: true,
        source: source.id.clone(),
        ..Default::default()
    };
    for url in urls {
        let bytes = fetch.get(&url)?;
        let mut reader = las::Reader::with_options(
            Cursor::new(bytes),
            las::ReaderOptions::default().with_laz_parallelism(las::LazParallelism::Yes),
        )
        .map_err(err)?;
        let header = reader.header();
        let keys = header.get_geotiff_crs().map_err(err)?;
        let code = keys
            .as_ref()
            .and_then(|k| {
                k.get_projected_crs_geo_key_value()
                    .or_else(|| k.get_geodetic_crs_geo_key_value())
            })
            .filter(|c| *c != 32767);
        let wkt = header.get_wkt_crs_bytes().map(|v| {
            String::from_utf8_lossy(v)
                .trim_end_matches('\0')
                .to_string()
        });
        let horizontal = code
            .map(|c| format!("EPSG:{c}"))
            .or_else(|| {
                wkt.clone()
                    .filter(|w| !w.contains("COMPD_CS") && !w.contains("COMPOUNDCRS"))
            })
            .ok_or("Bulk LAS needs an explicit horizontal CRS before accurate Target placement")?;
        let mut crs = json!({"horizontal":code.map(|c|c.to_string()),"wkt":wkt,
            "vertical":keys.as_ref().and_then(|k|k.get_vertical_crs_geo_key_value()).map(|c|c.to_string())});
        if let Some(object) = crs.as_object_mut() {
            object.retain(|_, v| !v.is_null());
        }
        emit(Event::Header(Header {
            source: source.clone(),
            crs,
            publisher: page.clone(),
        }))?;
        let expected = header.number_of_points();
        reader.seek(cursor.bulk).map_err(err)?;
        let mut seen = cursor.bulk;
        resources.nodes.set(resources.nodes.get() + 1);
        loop {
            check(active)?;
            if resources.limit().is_some() || fetch.started.elapsed() > Duration::from_secs(170) {
                summary.complete = false;
                summary.limit = "Loading budget reached; bulk source is partially inspected".into();
                break;
            }
            let batch = reader
                .read_points(resources.remaining_decoded() as u64)
                .map_err(err)?;
            if batch.is_empty() {
                break;
            }
            let mut retained = Vec::new();
            summary.decoded += batch.len() as u64;
            resources
                .decoded
                .set(resources.decoded.get() + batch.len() as u64);
            for p in batch.points() {
                let p = p.map_err(err)?;
                seen += 1;
                if p.is_withheld || matches!(u8::from(p.classification), 7 | 18) {
                    continue;
                }
                if ![p.x, p.y, p.z].iter().all(|v| v.is_finite()) {
                    return Err("Invalid original LAS coordinate".into());
                }
                retained.push((seen, p));
            }
            let mut xy = retained.iter().map(|(_, p)| [p.x, p.y]).collect::<Vec<_>>();
            project(&horizontal, &mut xy)?;
            let mut points = Vec::new();
            for ((ordinal, p), [lon, lat]) in retained.into_iter().zip(xy) {
                if !lon.is_finite()
                    || !lat.is_finite()
                    || !(-180. ..=180.).contains(&lon)
                    || !(-90. ..=90.).contains(&lat)
                {
                    return Err("Invalid transformed LAS coordinate".into());
                }
                if !region.contains(lon, lat) {
                    continue;
                }
                if summary.points + points.len() >= max_points {
                    seen = ordinal - 1;
                    summary.complete = false;
                    summary.limit = "Point budget reached; raise the budget and refine".into();
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
            cursor.bulk = seen;
            summary.points += points.len();
            if !points.is_empty() {
                emit(Event::Batch {
                    points,
                    spacing_m: 0.,
                })?;
            }
            if !summary.complete {
                break;
            }
        }
        summary.nodes += 1;
        if summary.complete && seen != expected {
            return Err("Incomplete original LAS decode".into());
        }
        if !summary.complete {
            break;
        }
    }
    cursor.complete = summary.complete;
    summary.transferred_bytes = fetch.transferred;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_footprints_outrank_neighboring_bbox_and_respect_holes() {
        let region = Region {
            lon: 0.,
            lat: 0.,
            radius_m: 1000.,
        };
        let full = r#"{"type":"Polygon","coordinates":[[[-1,-1],[1,-1],[1,1],[-1,1],[-1,-1]]]}"#;
        let fringe =
            r#"{"type":"Polygon","coordinates":[[[-1,-1],[-0.001,-1],[-0.001,1],[-1,1],[-1,-1]]]}"#;
        let hole = r#"{"type":"Polygon","coordinates":[[[-1,-1],[1,-1],[1,1],[-1,1],[-1,-1]],[[-0.1,-0.1],[0.1,-0.1],[0.1,0.1],[-0.1,0.1],[-0.1,-0.1]]]}"#;
        assert!(coverage(full, region) > coverage(fringe, region));
        assert_eq!(coverage(hole, region), 0);
    }
    #[test]
    fn original_bulk_las_preserves_xyz_attributes_noise_masks_and_budget() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = dir.path().join("catalog.sqlite");
        let db = Connection::open(&catalog).unwrap();
        db.execute_batch("CREATE TABLE surveys(id TEXT,stac TEXT,page TEXT); INSERT INTO surveys VALUES('opentopoID:fixture','https://portal.opentopography.org/stac/Fixture_collection.json','https://example.org/source');").unwrap();
        let mut header = las::Builder::from(las::Version::new(1, 4))
            .into_header()
            .unwrap();
        header.set_wkt_crs(b"EPSG:4326".to_vec()).unwrap();
        let path = dir.path().join("fixture.laz");
        let mut writer = las::Writer::from_path(&path, header).unwrap();
        for (z, class, withheld) in [
            (12.345, 2, false),
            (16.789, 5, false),
            (99., 7, false),
            (100., 18, false),
            (101., 2, true),
        ] {
            writer
                .write_point(las::Point {
                    x: 0.,
                    y: 0.,
                    z,
                    classification: las::point::Classification::new(class).unwrap(),
                    is_withheld: withheld,
                    intensity: 4321,
                    return_number: 1,
                    ..Default::default()
                })
                .unwrap();
        }
        writer.close().unwrap();
        let listing = "https://opentopography.s3.sdsc.edu/pc-bulk?list-type=2&prefix=Fixture%2F&max-keys=1000";
        let file = "https://opentopography.s3.sdsc.edu/pc-bulk/Fixture/fixture.laz";
        let xml = format!(
            "<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>Fixture/fixture.laz</Key><Size>{}</Size></Contents></ListBucketResult>",
            path.metadata().unwrap().len()
        );
        for (url, bytes) in [(listing, xml.into_bytes()), (file, fs::read(path).unwrap())] {
            fs::write(
                dir.path()
                    .join(format!("{:x}", Sha256::digest(url.as_bytes()))),
                bytes,
            )
            .unwrap();
        }
        for budget in [1, 10] {
            let mut fetch = Fetch {
                client: Client::new(),
                folder: dir.path(),
                active: None,
                transferred: 0,
                started: Instant::now(),
                budget: None,
            };
            let source = Source {
                id: "opentopoID:fixture".into(),
                date: "2026".into(),
                name: "Fixture".into(),
            };
            let mut measured = Vec::new();
            let summary = stream(
                &catalog,
                Region {
                    lon: 0.,
                    lat: 0.,
                    radius_m: 1000.,
                },
                &source,
                None,
                budget,
                &mut fetch,
                |crs, xy| {
                    assert_eq!(crs, "EPSG:4326");
                    assert!(xy.iter().all(|p| *p == [0., 0.]));
                    Ok(())
                },
                &mut |e| {
                    match e {
                        Event::Header(h) => assert!(h.crs.get("vertical").is_none()),
                        Event::Batch { points, .. } => measured.extend(points),
                    }
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(summary.points, budget.min(2));
            assert_eq!(summary.complete, budget > 1);
            assert!((measured[0].source_xyz[2] - 12.345).abs() < 0.0011);
            assert_eq!(measured[0].intensity, 4321);
            assert_eq!(measured[0].classification, 2);
            assert!(measured.iter().all(|p| !matches!(p.classification, 7 | 18)));
            assert_eq!(summary.transferred_bytes, 0);
        }
        let source = Source {
            id: "opentopoID:fixture".into(),
            date: "2026".into(),
            name: "Fixture".into(),
        };
        let resources = StreamBudget::default();
        let mut cursor = StreamCursor::default();
        let mut resumed = Vec::new();
        for quota in [1, 10] {
            let mut fetch = Fetch {
                client: Client::new(),
                folder: dir.path(),
                active: None,
                transferred: 0,
                started: Instant::now(),
                budget: Some(&resources),
            };
            stream_resume(
                &catalog,
                Region {
                    lon: 0.,
                    lat: 0.,
                    radius_m: 1000.,
                },
                &source,
                None,
                quota,
                &mut fetch,
                &mut cursor,
                &resources,
                |_, _| Ok(()),
                &mut |e| {
                    if let Event::Batch { points, .. } = e {
                        resumed.extend(points.into_iter().map(|p| p.source_xyz[2]));
                    }
                    Ok(())
                },
            )
            .unwrap();
        }
        assert!(cursor.complete);
        assert_eq!(cursor.bulk, 5);
        assert_eq!(resumed.len(), 2);
        assert!((resumed[0] - 12.345).abs() < 0.0011 && (resumed[1] - 16.789).abs() < 0.0011);
    }
    #[test]
    fn bulk_listing_rejects_truncation_oversize_and_traversal() {
        let xml = |key: &str, size: u64| {
            format!(
                "<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>{key}</Key><Size>{size}</Size></Contents></ListBucketResult>"
            )
        };
        assert!(
            assets(
                &xml("Israel_2006/ot_Israel_2006.laz", 24960180),
                "Israel_2006"
            )
            .unwrap()[0]
                .ends_with("Israel_2006/ot_Israel_2006.laz")
        );
        assert!(assets(&xml("Israel_2006/a.laz", NODE_LIMIT + 1), "Israel_2006").is_err());
        assert!(assets(&xml("Israel_2006/../a.laz", 1), "Israel_2006").is_err());
        assert!(
            assets(
                "<ListBucketResult><IsTruncated>true</IsTruncated></ListBucketResult>",
                "Israel_2006"
            )
            .is_err()
        );
    }
}
