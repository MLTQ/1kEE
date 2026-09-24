use super::*;
use crate::marching_squares::build_tile_contours_on_grid;

fn crossings(db: &Connection, tile: TileKey, edge: f32) -> Vec<(i32, f32)> {
    let mut query=db.prepare("SELECT elevation_m,geom FROM contour_tiles WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3").unwrap();
    let mut rows = query
        .query(params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket])
        .unwrap();
    let mut points = Vec::new();
    while let Some(row) = rows.next().unwrap() {
        let elevation = row.get::<_, f32>(0).unwrap();
        let blob = row.get::<_, Vec<u8>>(1).unwrap();
        for line in tile_archive::gpkg::parse_gpkg_lines(&blob, |x, y| (x, y)) {
            for p in [line.first(), line.last()].into_iter().flatten() {
                if p.0 == edge {
                    points.push((elevation as i32, p.1));
                }
            }
        }
    }
    points.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    points.dedup();
    points
}

#[test]
fn native_core_tiles_keep_matching_seams_and_discard_halo() {
    let mut db = Connection::open_in_memory().unwrap();
    let path = std::env::temp_dir().join(format!("1kee-core-native-{}.sqlite", std::process::id()));
    // Use the production schema without retaining a filesystem database.
    let schema = open_cache_db(&path).unwrap();
    let sql: Vec<String> = schema
        .prepare("SELECT sql FROM sqlite_master WHERE sql IS NOT NULL AND type='table'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    drop(schema);
    let _ = std::fs::remove_file(&path);
    for statement in sql {
        db.execute_batch(&statement).unwrap();
    }
    let spec = all_specs()[6];
    let left = TileKey {
        zoom_bucket: 6,
        lat_bucket: 0,
        lon_bucket: 0,
    };
    let right = TileKey {
        lon_bucket: 1,
        ..left
    };
    for tile in [left, right] {
        let grid = CoreTile::new(
            spec.half_extent_deg,
            spec.raster_size,
            tile.lat_bucket,
            tile.lon_bucket,
        );
        let (lines, coast) = build_tile_contours_on_grid(
            FocusContourSpec {
                raster_size: grid.raster_size,
                ..spec
            },
            grid.source,
            |lat, lon| 100. + 200. * lat + 100. * lon,
        );
        write_tile_native(&mut db, tile, &lines, &coast).unwrap();
        let mut q = db
            .prepare("SELECT geom FROM contour_tiles WHERE lon_bucket=?1")
            .unwrap();
        let rows = q
            .query_map([tile.lon_bucket], |r| r.get::<_, Vec<u8>>(0))
            .unwrap();
        for row in rows {
            for line in tile_archive::gpkg::parse_gpkg_lines(&row.unwrap(), |x, y| (x, y)) {
                for (x, y) in line {
                    assert!(
                        x >= grid.core.min_lon as f32
                            && x <= grid.core.max_lon as f32
                            && y >= grid.core.min_lat as f32
                            && y <= grid.core.max_lat as f32
                    );
                }
            }
        }
    }
    let edge = CoreTile::new(spec.half_extent_deg, spec.raster_size, 0, 0)
        .core
        .max_lon as f32;
    let a = crossings(&db, left, edge);
    let b = crossings(&db, right, edge);
    assert!(a.len() >= 2);
    assert_eq!(a, b);
}

#[test]
#[ignore = "requires GDAL_BIN_DIR; builds only isolated synthetic terrain"]
fn gdal_core_tiles_keep_matching_seams() {
    let tools = PathBuf::from(std::env::var("GDAL_BIN_DIR").expect("GDAL_BIN_DIR"));
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("1kee-core-gdal-{nonce}"));
    fs::create_dir_all(&root).unwrap();
    let mut asc = String::from(
        "ncols 256\nnrows 256\nxllcorner -0.1\nyllcorner -0.1\ncellsize 0.001\nNODATA_value -32768\n",
    );
    for y in 0..256 {
        for x in 0..256 {
            let lat = 0.156 - (y as f64 + 0.5) * 0.001;
            let lon = -0.1 + (x as f64 + 0.5) * 0.001;
            asc.push_str(&format!("{} ", 100. + 200. * lat + 100. * lon));
        }
        asc.push('\n');
    }
    let source = root.join("source.asc");
    fs::write(&source, asc).unwrap();
    fs::write(root.join("source.prj"),"GEOGCS[\"WGS 84\",DATUM[\"WGS_1984\",SPHEROID[\"WGS 84\",6378137,298.257223563]],PRIMEM[\"Greenwich\",0],UNIT[\"degree\",0.0174532925199433]]").unwrap();
    let out = root.join("cache.sqlite");
    let spec = all_specs()[6];
    let left = TileKey {
        zoom_bucket: 6,
        lat_bucket: 0,
        lon_bucket: 0,
    };
    let right = TileKey {
        lon_bucket: 1,
        ..left
    };
    for tile in [left, right] {
        let core = CoreTile::new(spec.half_extent_deg, spec.raster_size, 0, tile.lon_bucket);
        let tif = root.join(format!("{}.tif", tile.lon_bucket));
        let gpkg = root.join(format!("{}.gpkg", tile.lon_bucket));
        run_gdalwarp(
            &tools.join("gdalwarp"),
            std::slice::from_ref(&source),
            &tif,
            core.source,
            FocusContourSpec {
                raster_size: core.raster_size,
                ..spec
            },
        )
        .unwrap();
        run_gdal_contour(&tools.join("gdal_contour"), &tif, &gpkg, spec.interval_m).unwrap();
        import_tile_clipped(&out, tile, &gpkg, Some(core.core)).unwrap();
    }
    let db = open_cache_db(&out).unwrap();
    let edge = CoreTile::new(spec.half_extent_deg, spec.raster_size, 0, 0)
        .core
        .max_lon as f32;
    let a = crossings(&db, left, edge);
    let b = crossings(&db, right, edge);
    assert!(a.len() >= 2);
    assert_eq!(a, b);
    drop(db);
    fs::remove_dir_all(root).unwrap();
}
