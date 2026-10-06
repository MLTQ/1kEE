//! Adjacent-core regression through the real SQLite and packed read paths.
use super::*;
use crate::model::ActiveBody;
use crate::panels::world_map::{local_contour_pass::LocalTileId, local_terrain_scene::composition};

fn request(path: &Path, bucket: i32, y: i32, x: i32) -> ContourReadRequest {
    (
        CacheKey {
            path: path.into(),
            zoom_bucket: bucket,
            lat_bucket: y,
            lon_bucket: x,
        },
        srtm_focus_cache::FocusContourAsset {
            path: path.into(),
            zoom_bucket: bucket,
            lat_bucket: y,
            lon_bucket: x,
            simplify_step: srtm_focus_cache::zoom::spec_for_zoom(
                [0.5, 1.5, 2.5, 3.5, 5.0, 8.0, 12.0, 16.0, 25.0, 35.0, 50.0][bucket as usize],
            )
            .simplify_step,
        },
    )
}

fn core(key: &CacheKey) -> tile_archive::contour_grid::Bounds {
    composition::core(
        ActiveBody::Earth,
        LocalTileId {
            zoom_bucket: key.zoom_bucket,
            lat_bucket: key.lat_bucket,
            lon_bucket: key.lon_bucket,
        },
    )
}

fn read(
    requests: &[ContourReadRequest],
    selection: ReadSelection,
) -> Vec<(CacheKey, Vec<ContourPath>)> {
    let mut tiles = Vec::new();
    stream_local_contours(
        &requests[0].0.path,
        requests,
        selection,
        &mut |_, _, _| {},
        &mut |key, contours| {
            tiles.push((key, contours));
            true
        },
    )
    .unwrap();
    tiles
}

fn geometry(points: &[(f64, f64)]) -> Vec<u8> {
    let mut blob = b"GP\0\0\0\0\0\0".to_vec();
    blob.push(1);
    blob.extend_from_slice(&2u32.to_le_bytes());
    blob.extend_from_slice(&(points.len() as u32).to_le_bytes());
    for &(lon, lat) in points {
        blob.extend_from_slice(&lon.to_le_bytes());
        blob.extend_from_slice(&lat.to_le_bytes());
    }
    blob
}

fn boundary_ends(lines: &[ContourPath], edge: f32) -> Vec<(u32, u32)> {
    let mut ends = Vec::new();
    for line in lines {
        for point in [line.points.first().unwrap(), line.points.last().unwrap()] {
            if point.lon == edge {
                ends.push((line.elevation_m.to_bits(), point.lat.to_bits()));
            }
        }
    }
    ends.sort_unstable();
    ends
}

#[test]
fn adjacent_earth_tiles_keep_short_edge_fragments_in_sqlite_and_packed_reads() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("1kee-seams-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(root.join("terrain")).unwrap();
    let path = root.join("terrain/srtm_focus_cache.sqlite");
    let requests = [request(&path, 1, 28, 88), request(&path, 1, 28, 89)];
    let edge = core(&requests[0].0).max_lon;
    let mut connection = srtm_focus_cache::db::open_cache_db(&path).unwrap();
    let transaction = connection.transaction().unwrap();
    let mut packed = Vec::new();
    for (key, _) in &requests {
        let bounds = core(key);
        let center = (bounds.min_lon + bounds.max_lon) * 0.5;
        let mut rows = Vec::new();
        // Interior lines outnumber the old 120-path cap and are longer than
        // the fragments crossing the shared boundary.
        for i in 0..160 {
            rows.push((
                i as f32 * 25.0,
                geometry(
                    &(0..33)
                        .map(|j| {
                            (
                                center - 0.15 + j as f64 * 0.01,
                                bounds.min_lat + 0.1 + i as f64 * 0.003,
                            )
                        })
                        .collect::<Vec<_>>(),
                ),
            ));
        }
        for i in 0..24 {
            let lat = bounds.min_lat + 0.12 + i as f64 * 0.02;
            rows.push((
                5000.0 + i as f32 * 25.0,
                geometry(&[
                    (edge - 0.02, lat),
                    (edge - 0.001, lat + 0.002),
                    (edge + 0.001, lat - 0.002),
                    (edge + 0.02, lat),
                ]),
            ));
        }
        // A legacy halo must not inflate retained memory or leak into another core.
        rows.push((
            9999.0,
            geometry(&[
                (bounds.max_lon + 0.1, bounds.min_lat),
                (bounds.max_lon + 0.2, bounds.min_lat),
            ]),
        ));
        for (fid, (elevation, blob)) in rows.iter().enumerate() {
            transaction
                .execute(
                    "INSERT INTO contour_tiles VALUES (?1,?2,?3,?4,?5,?6)",
                    params![
                        key.zoom_bucket,
                        key.lat_bucket,
                        key.lon_bucket,
                        fid,
                        elevation,
                        blob
                    ],
                )
                .unwrap();
        }
        transaction.execute("INSERT INTO contour_tile_manifest(zoom_bucket,lat_bucket,lon_bucket,contour_count) VALUES(?1,?2,?3,?4)",
            params![key.zoom_bucket, key.lat_bucket, key.lon_bucket, rows.len()]).unwrap();
        packed.push((
            tile_archive::Key {
                body: 0,
                layer: *b"CNTR",
                grid: 2,
                level: 1,
                y: key.lat_bucket,
                x: key.lon_bucket,
            },
            tile_archive::contours::encode(&rows).unwrap(),
        ));
    }
    transaction.commit().unwrap();
    drop(connection);

    let before = read(&requests, ReadSelection::WholeTile(120));
    assert!(before.iter().all(|(_, lines)| lines.len() == 120));
    assert!(
        before
            .iter()
            .all(|(_, lines)| lines.iter().all(|c| c.elevation_m < 5000.0))
    );
    let sqlite = read(&requests, ReadSelection::EarthCore);
    for (_, lines) in &sqlite {
        assert_eq!(lines.len(), 184);
        assert!(
            lines
                .iter()
                .filter(|c| c.elevation_m < 5000.0)
                .all(|c| c.points.len() == 33)
        );
        assert!(
            lines
                .iter()
                .filter(|c| c.elevation_m >= 5000.0)
                .all(|c| c.points.len() == 3)
        );
        assert_eq!(boundary_ends(lines, edge as f32).len(), 24);
        assert!(lines.iter().all(|c| c.elevation_m != 9999.0));
    }
    assert_eq!(
        boundary_ends(&sqlite[0].1, edge as f32),
        boundary_ends(&sqlite[1].1, edge as f32)
    );

    let mut writer = tile_archive::Writer::create(&root.join(tile_archive::FILE_NAME)).unwrap();
    writer.put_batch(&packed, None).unwrap();
    writer
        .metadata(
            "contours:0",
            &tile_archive::contours::fingerprint(&path).unwrap(),
        )
        .unwrap();
    writer.finish().unwrap();
    assert!(
        crate::world_archive::ContourArchive::open(&path)
            .unwrap()
            .get(1, 28, 88)
            .is_some()
    );
    let packed = read(&requests, ReadSelection::EarthCore);
    for ((_, a), (_, b)) in sqlite.iter().zip(&packed) {
        assert_eq!(a.len(), b.len());
        for (a, b) in a.iter().zip(b) {
            assert_eq!(a.elevation_m, b.elevation_m);
            assert_eq!(a.points, b.points);
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn every_earth_tier_keeps_more_than_the_old_deep_tile_limit() {
    for bucket in 0..=10 {
        let (key, _) = request(Path::new("unused"), bucket, 0, 0);
        let bounds = core(&key);
        let selection = ReadSelection::EarthCore.tile(&key, 4);
        let mut contours = Vec::new();
        for i in 0..10_025 {
            selection.append(
                &mut contours,
                [vec![
                    GeoPoint {
                        lon: (bounds.min_lon * 0.5) as f32,
                        lat: 0.0,
                    },
                    GeoPoint {
                        lon: (bounds.max_lon * 0.5) as f32,
                        lat: 0.0,
                    },
                ]],
                i as f32,
            );
        }
        selection.finish(&mut contours);
        assert_eq!(
            contours.len(),
            10_025,
            "bucket {bucket} must not drop short paths"
        );
    }
}

#[test]
fn earth_lod_reads_preserve_bends_and_small_closed_contours() {
    for bucket in 0..=10 {
        let (key, asset) = request(Path::new("unused"), bucket, 0, 0);
        let scale = core(&key).max_lon as f32;
        let bend: Vec<_> = [
            (-0.8, 0.0),
            (-0.6, 0.5),
            (-0.4, 0.5),
            (-0.2, 0.0),
            (0.2, -0.5),
            (0.4, -0.5),
            (0.8, 0.0),
        ]
        .into_iter()
        .map(|(lon, lat)| GeoPoint {
            lon: lon * scale,
            lat: lat * scale,
        })
        .collect();
        let ring: Vec<_> = [
            (-0.5, -0.5),
            (0.5, -0.5),
            (0.5, 0.5),
            (-0.5, 0.5),
            (-0.5, -0.5),
        ]
        .into_iter()
        .map(|(lon, lat)| GeoPoint {
            lon: lon * scale,
            lat: lat * scale,
        })
        .collect();
        // Local and globe reads must both respect the geometry already sampled
        // for this LOD. Changing vertex order must not change its shape.
        for policy in [
            ReadSelection::EarthCore,
            ReadSelection::WholeTile(usize::MAX),
        ] {
            let selection = policy.tile(&key, asset.simplify_step);
            for original in [&bend, &ring] {
                for reverse in [false, true] {
                    let mut expected = original.clone();
                    if reverse {
                        expected.reverse();
                    }
                    let mut actual = Vec::new();
                    selection.append(&mut actual, [expected.clone()], 100.0);
                    selection.finish(&mut actual);
                    assert_eq!(actual.len(), 1);
                    assert_eq!(
                        actual[0].points, expected,
                        "bucket {bucket}, reverse={reverse}"
                    );
                }
            }
        }
    }
}

#[test]
#[ignore = "read-only contour fidelity check; ONEKEE_CONTOUR_BENCH_DB required"]
fn cached_earth_lods_preserve_source_vertices() {
    let path = PathBuf::from(std::env::var_os("ONEKEE_CONTOUR_BENCH_DB").unwrap());
    let mut export = Vec::new();
    for (name, bucket, y, x, old_step) in [("Yemen", 0, 9, 27, 5), ("Himalayas", 1, 28, 88, 4)] {
        for (view, policy) in [
            ("local", ReadSelection::EarthCore),
            ("globe", ReadSelection::WholeTile(usize::MAX)),
        ] {
            let req = request(&path, bucket, y, x);
            let actual = read(std::slice::from_ref(&req), policy).remove(0).1;
            let mut raw_request = req.clone();
            raw_request.1.simplify_step = 1;
            let raw = read(&[raw_request], policy).remove(0).1;
            assert!(!raw.is_empty());
            assert_eq!(actual.len(), raw.len());
            for (actual, raw) in actual.iter().zip(&raw) {
                assert_eq!(actual.elevation_m, raw.elevation_m);
                assert_eq!(actual.points, raw.points);
            }
            let mut old_request = req.clone();
            old_request.1.simplify_step = old_step;
            let old = read(&[old_request], policy).remove(0).1;
            // The globe simplifies the full footprint before composition,
            // whereas local reads clip first. Reproduce both orders, then show
            // the same owned core for an equal-area comparison at one LOD.
            let bounds = core(&req.0);
            let clip = |lines: Vec<ContourPath>| {
                let tile = LocalTileGeometry {
                    id: LocalTileId {
                        zoom_bucket: bucket,
                        lat_bucket: y,
                        lon_bucket: x,
                    },
                    bounds: residency::Bounds::from_contours(&lines),
                    contours: Arc::new(lines),
                };
                composition::clip_contours(&tile, &[bounds])
            };
            let old = clip(old);
            let raw = clip(raw);
            let segments = |lines: &[ContourPath]| {
                lines
                    .iter()
                    .map(|line| line.points.len().saturating_sub(1))
                    .sum::<usize>()
            };
            let encode = |lines: &[ContourPath]| {
                lines
                    .iter()
                    .map(|line| {
                        serde_json::json!({"elevation": line.elevation_m,
                    "points": line.points.iter().map(|p| [p.lon, p.lat]).collect::<Vec<_>>()})
                    })
                    .collect::<Vec<_>>()
            };
            eprintln!(
                "{name} {view} {bucket}/{y}/{x}: {} -> {} core paths; {} -> {} segments",
                old.len(),
                raw.len(),
                segments(&old),
                segments(&raw)
            );
            export.push(
                serde_json::json!({"name": name, "view": view, "bucket": bucket,
                "bounds": [bounds.min_lon, bounds.min_lat, bounds.max_lon, bounds.max_lat],
                "before": encode(&old), "after": encode(&raw)}),
            );
        }
    }
    if let Some(output) = std::env::var_os("ONEKEE_CONTOUR_FIDELITY_OUTPUT") {
        std::fs::write(output, serde_json::to_vec(&export).unwrap()).unwrap();
    }
}

#[test]
#[ignore = "read-only Himalayan cache check; ONEKEE_CONTOUR_BENCH_DB required"]
fn cached_himalayan_cores_recover_boundary_detail() {
    let path = PathBuf::from(std::env::var_os("ONEKEE_CONTOUR_BENCH_DB").unwrap());
    let requests: Vec<_> = [(28, 88), (28, 89), (29, 88), (29, 89)]
        .into_iter()
        .map(|(y, x)| request(&path, 1, y, x))
        .collect();
    let mut legacy_requests = requests.clone();
    for (_, asset) in &mut legacy_requests {
        asset.simplify_step = 4;
    }
    let before = read(&legacy_requests, ReadSelection::WholeTile(120));
    let after = read(&requests, ReadSelection::EarthCore);
    let mut export = Vec::new();
    for ((key, old), (_, new)) in before.iter().zip(&after) {
        let bounds = core(key);
        let selection = ReadSelection::EarthCore.tile(key, 1);
        let mut old_core = Vec::new();
        for line in old {
            selection.append(&mut old_core, [line.points.clone()], line.elevation_m);
        }
        let old_ends = boundary_ends(&old_core, bounds.max_lon as f32).len();
        let new_ends = boundary_ends(new, bounds.max_lon as f32).len();
        eprintln!(
            "Himalayan tile {}/{}: core paths {} -> {}; east-edge endpoints {} -> {}",
            key.lat_bucket,
            key.lon_bucket,
            old_core.len(),
            new.len(),
            old_ends,
            new_ends
        );
        assert!(new.len() > old_core.len() * 2);
        assert!(new_ends > old_ends * 2);
        for line in new {
            for point in &line.points {
                assert!(point.lon >= bounds.min_lon as f32 && point.lon <= bounds.max_lon as f32);
                assert!(point.lat >= bounds.min_lat as f32 && point.lat <= bounds.max_lat as f32);
            }
        }
        let encode = |lines: &[ContourPath]| {
            lines
                .iter()
                .map(|line| {
                    line.points
                        .iter()
                        .map(|p| [p.lon, p.lat])
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        };
        export.push(serde_json::json!({"bounds": [bounds.min_lon, bounds.min_lat, bounds.max_lon, bounds.max_lat],
            "before": encode(&old_core), "after": encode(new)}));
    }
    if let Some(output) = std::env::var_os("ONEKEE_CONTOUR_SEAM_OUTPUT") {
        std::fs::write(output, serde_json::to_vec(&export).unwrap()).unwrap();
    }
}
