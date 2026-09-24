use super::TileKey;
use super::db::*;
use rusqlite::{Connection, params};

#[test]
fn core_import_keeps_crossings_excludes_halo_and_rolls_back_bad_geometry() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("1kee-core-import-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let input = root.join("source.gpkg");
    let output = root.join("cache.sqlite");
    let source = Connection::open(&input).unwrap();
    source
        .execute_batch("CREATE TABLE contour(fid INTEGER PRIMARY KEY, geom BLOB, elevation_m REAL)")
        .unwrap();
    let crossing = tile_archive::contour_clip::encode(&[vec![(-1., 0.5), (2., 0.5)]]);
    let outside = tile_archive::contour_clip::encode(&[vec![(2., 0.5), (3., 0.5)]]);
    source
        .execute("INSERT INTO contour VALUES(1,?1,12)", [&crossing])
        .unwrap();
    source
        .execute("INSERT INTO contour VALUES(2,?1,15)", [&outside])
        .unwrap();
    let legacy = TileKey {
        zoom_bucket: 10,
        lat_bucket: 0,
        lon_bucket: 0,
    };
    let core = TileKey {
        zoom_bucket: 10,
        lat_bucket: 0,
        lon_bucket: 1,
    };
    let bounds = tile_archive::contour_grid::Bounds {
        min_lon: 0.,
        max_lon: 1.,
        min_lat: 0.,
        max_lat: 1.,
    };
    import_tile_into_cache(&output, legacy, &input, None).unwrap();
    import_tile_into_cache_clipped(&output, core, &input, None, Some(bounds)).unwrap();
    import_coastline_into_cache_clipped(&output, core, &input, Some(bounds)).unwrap();
    let db = open_cache_db_read_only(&output).unwrap();
    let bytes: Vec<u8> = db
        .query_row(
            "SELECT geom FROM contour_tiles WHERE lon_bucket=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        tile_archive::gpkg::parse_gpkg_lines(&bytes, |x, y| (x, y)),
        vec![vec![(0., 0.5), (1., 0.5)]]
    );
    for table in ["contour_tile_manifest", "coastline_tile_manifest"] {
        let field = if table.starts_with("contour_") {
            "contour_count"
        } else {
            "line_count"
        };
        assert_eq!(
            db.query_row(
                &format!("SELECT {field} FROM {table} WHERE lon_bucket=1"),
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }
    assert_eq!(
        db.query_row(
            "SELECT geom FROM contour_tiles WHERE lon_bucket=0 AND fid=1",
            [],
            |r| r.get::<_, Vec<u8>>(0)
        )
        .unwrap(),
        crossing
    );
    source
        .execute("UPDATE contour SET geom=?1 WHERE fid=2", params![vec![0u8]])
        .unwrap();
    assert!(import_tile_into_cache_clipped(&output, core, &input, None, Some(bounds)).is_err());
    assert_eq!(
        db.query_row(
            "SELECT geom FROM contour_tiles WHERE lon_bucket=1",
            [],
            |r| r.get::<_, Vec<u8>>(0)
        )
        .unwrap(),
        bytes
    );
    source.execute("DELETE FROM contour", []).unwrap();
    import_tile_into_cache_clipped(&output, core, &input, None, Some(bounds)).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT contour_count FROM contour_tile_manifest WHERE lon_bucket=1",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    drop(db);
    drop(source);
    std::fs::remove_dir_all(root).unwrap();
}
