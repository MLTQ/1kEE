use super::*;

fn tile(lat: i32) -> TileKey {
    TileKey {
        zoom_bucket: 0,
        lat_bucket: lat,
        lon_bucket: 27,
    }
}
fn insert(c: &Connection, t: TileKey, blob: &[u8]) {
    c.execute(
        "INSERT INTO contour_tiles VALUES(?1,?2,?3,1,50,?4)",
        params![t.zoom_bucket, t.lat_bucket, t.lon_bucket, blob],
    )
    .unwrap();
    c.execute(
        "INSERT INTO contour_tile_manifest VALUES(?1,?2,?3,1,0)",
        params![t.zoom_bucket, t.lat_bucket, t.lon_bucket],
    )
    .unwrap();
}
fn geometry() -> Vec<u8> {
    tile_archive::contour_clip::encode(&[vec![(48.0, 16.0), (48.1, 16.1)]])
}

#[test]
fn discard_bad_geometry_preserves_neighbour_and_allows_atomic_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("srtm_native_v1.sqlite");
    let c = db::open_cache_db(&path).unwrap();
    insert(&c, tile(9), b"broken geometry");
    insert(&c, tile(10), &geometry());
    c.execute(
        "INSERT INTO coastline_tiles VALUES(0,9,27,1,?1)",
        [geometry()],
    )
    .unwrap();
    c.execute("INSERT INTO coastline_tile_manifest VALUES(0,9,27,1,0)", [])
        .unwrap();
    drop(c);
    discard(&path, tile(9)).unwrap();
    let c = db::open_cache_db(&path).unwrap();
    assert!(!db::tile_exists(&c, tile(9)).unwrap());
    assert!(readable(&c, tile(10)).unwrap());
    assert_eq!(
        c.query_row("SELECT count(*) FROM coastline_tiles", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    insert(&c, tile(9), &geometry());
    drop(c);
    // A late failure from an old reader must not delete the repaired tile.
    discard(&path, tile(9)).unwrap();
    let c = db::open_cache_db_read_only(&path).unwrap();
    assert!(db::tile_exists(&c, tile(9)).unwrap());
    assert!(readable(&c, tile(9)).unwrap());
}

#[test]
fn corrupt_database_routes_to_durable_isolated_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("srtm_native_v1.sqlite");
    let original = b"not a SQLite database at all";
    std::fs::write(&path, original).unwrap();
    discard(&path, tile(9)).unwrap();
    let replacement = resolve(&path, tile(9));
    assert_ne!(replacement, path);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(count(&replacement, tile(9)).unwrap(), None);
    let c = db::open_cache_db(&replacement).unwrap();
    insert(&c, tile(9), &geometry());
    drop(c);
    assert_eq!(count(&resolve(&path, tile(9)), tile(9)).unwrap(), Some(1));
    assert_eq!(resolve(&path, tile(10)), path);
    // A corrupt original header still schedules/serves the rebuilt native tile.
    let conn = super::super::open_region_cache_db(&path).unwrap();
    assert!(
        db::contour_manifest_window(&conn, 0, 9, 9, 27, 27)
            .unwrap()
            .is_empty()
    );
    let asset = builders::ensure_bucket_asset(
        None,
        dir.path(),
        &path,
        None,
        false,
        super::super::zoom::spec_for_zoom(0.5),
        9,
        27,
        1.0,
    )
    .unwrap();
    assert_eq!(asset.path, replacement);
}

#[test]
fn shadowed_manifest_never_advertises_old_geometry_and_new_missing_tiles_use_safe_storage() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("srtm_native_v1.sqlite");
    let c = db::open_cache_db(&path).unwrap();
    insert(&c, tile(9), &geometry());
    insert(&c, tile(10), &geometry());
    let replaced = replacement(&path, tile(9));
    std::fs::create_dir_all(replaced.parent().unwrap()).unwrap();
    let m = db::contour_manifest_window(&c, 0, 9, 10, 27, 27).unwrap();
    assert!(!m.contains_key(&(9, 27)));
    assert_eq!(m.get(&(10, 27)), Some(&1));
    assert!(!db::tile_exists(&c, tile(9)).unwrap());
    let new_path = build_path(&path, tile(11), None, true).unwrap();
    assert_ne!(new_path, path);
    assert_eq!(build_path(&path, tile(10), Some(1), true).unwrap(), path);
    let r = db::open_cache_db(&replaced).unwrap();
    insert(&r, tile(9), &geometry());
    drop(r);
    let m = db::contour_manifest_window(&c, 0, 9, 10, 27, 27).unwrap();
    assert_eq!(m.len(), 2);
    assert!(db::tile_exists(&c, tile(9)).unwrap());
}

#[test]
fn missing_rows_are_invalid_but_genuine_empty_tile_is_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let c = db::open_cache_db(&path).unwrap();
    c.execute("INSERT INTO contour_tile_manifest VALUES(0,9,27,5,0)", [])
        .unwrap();
    c.execute("INSERT INTO contour_tile_manifest VALUES(0,10,27,0,0)", [])
        .unwrap();
    assert!(!readable(&c, tile(9)).unwrap());
    drop(c);
    discard(&path, tile(9)).unwrap();
    discard(&path, tile(10)).unwrap();
    let c = db::open_cache_db_read_only(&path).unwrap();
    assert!(!db::tile_exists(&c, tile(9)).unwrap());
    assert!(db::tile_exists(&c, tile(10)).unwrap());
    assert!(!invalid_geometry(&Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
        None
    )));
    assert!(!invalid_geometry(&Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR),
        None
    )));
}

#[test]
fn damaged_manifest_page_does_not_block_regeneration() {
    use std::io::{Seek, SeekFrom, Write};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let c = db::open_cache_db(&path).unwrap();
    insert(&c, tile(9), &geometry());
    let page: u64 = c.query_row("SELECT rootpage FROM sqlite_master WHERE name='sqlite_autoindex_contour_tile_manifest_1'",[],|r|r.get(0)).unwrap();
    let size: u64 = c.query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap();
    c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
    drop(c);
    let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.seek(SeekFrom::Start((page - 1) * size)).unwrap();
    file.write_all(&[0xff]).unwrap();
    drop(file);
    let c = db::open_cache_db_read_only(&path).unwrap();
    assert!(
        db::contour_manifest_window(&c, 0, 9, 9, 27, 27)
            .unwrap()
            .is_empty()
    );
    assert_ne!(build_path(&path, tile(9), None, true).unwrap(), path);
}

#[test]
fn isolated_namespaces_are_stable_and_separate_source_volumes() {
    let destination = Path::new("/internal/recovery");
    let a = root_for(
        Path::new("/drive-a/terrain/srtm_native_v1.sqlite"),
        Some(destination),
    );
    let b = root_for(
        Path::new("/drive-b/terrain/srtm_native_v1.sqlite"),
        Some(destination),
    );
    assert!(a.starts_with(destination));
    assert_ne!(a, b);
    assert_eq!(
        a,
        root_for(
            Path::new("/drive-a/terrain/srtm_native_v1.sqlite"),
            Some(destination)
        )
    );
}

#[test]
#[ignore = "set ONEKEE_TERRAIN_REPAIR_ROOT to a disposable absolute directory"]
fn isolated_recovery_never_changes_source_database_bytes() {
    assert!(std::env::var_os("ONEKEE_TERRAIN_REPAIR_ROOT").is_some());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("srtm_native_v1.sqlite");
    let c = db::open_cache_db(&path).unwrap();
    insert(&c, tile(9), b"broken geometry");
    insert(&c, tile(10), &geometry());
    drop(c);
    let before = std::fs::read(&path).unwrap();
    discard(&path, tile(9)).unwrap();
    assert_eq!(before, std::fs::read(&path).unwrap());
    let target = resolve(&path, tile(9));
    assert!(target.starts_with(isolated_root().unwrap()));
    let c = db::open_cache_db(&target).unwrap();
    insert(&c, tile(9), &geometry());
    drop(c);
    let original = db::open_cache_db_read_only(&path).unwrap();
    assert!(readable(&original, tile(10)).unwrap());
    assert!(db::tile_exists(&original, tile(9)).unwrap());
    assert_eq!(
        db::contour_manifest_window(&original, 0, 9, 10, 27, 27)
            .unwrap()
            .len(),
        2
    );
    drop(original);
    assert_eq!(before, std::fs::read(&path).unwrap());
    assert!(!db::wal_path_for(&path).exists());
    assert!(!db::shm_path_for(&path).exists());
    std::fs::remove_dir_all(root(&path)).unwrap();
}
