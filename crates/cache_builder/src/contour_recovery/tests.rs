use super::*;
use tempfile::tempdir;

fn seed(path: &Path, height: f64, built: i64) -> Connection {
    let db = crate::contours::open_cache_db(path).unwrap();
    tile_archive::native_contours::initialize(&db).unwrap();
    let blob = crate::contours::encode_gpkg_linestring(&[(10., 20.), (11., 21.)]);
    db.execute(
        "INSERT INTO contour_tiles VALUES (4,2,9,7,?,?)",
        params![height, blob],
    )
    .unwrap();
    db.execute(
        "INSERT INTO contour_tile_manifest VALUES (4,2,9,1,?)",
        [built],
    )
    .unwrap();
    db.execute(
        "INSERT INTO coastline_tile_manifest VALUES (4,2,9,0,?)",
        [built],
    )
    .unwrap();
    db
}
fn command(root: &Path) -> Command {
    Command {
        caches: vec![root.join("old.sqlite")],
        output: root.join(NATIVE),
        ..Default::default()
    }
}

#[test]
fn retires_verified_files_and_retries_cleanup() {
    let dir = tempdir().unwrap();
    let mut cmd = command(dir.path());
    drop(seed(&cmd.caches[0], 20., 100));
    run(&cmd, &mut |_| {}).unwrap();
    assert!(cmd.caches[0].exists());
    assert!(run(&cmd, &mut |_| {}).is_err());
    cmd.cleanup_only = true;
    assert!(run(&cmd, &mut |_| {}).unwrap().contains("Retired 1"));
    assert!(!cmd.caches[0].exists());
    assert!(run(&cmd, &mut |_| {}).unwrap().contains("Retired 0"));
}

#[test]
fn repair_wins_and_conflicting_original_is_retained() {
    let dir = tempdir().unwrap();
    let mut cmd = command(dir.path());
    let repairs = dir.path().join("repairs");
    let replacement = repairs
        .join("abcdef1234567890")
        .join(NATIVE)
        .join("4_2_9")
        .join(NATIVE);
    let unrelated = repairs
        .join("srtm_focus_cache.sqlite")
        .join("4_2_9")
        .join("srtm_focus_cache.sqlite");
    drop(seed(&cmd.caches[0], 10., 200));
    drop(seed(&replacement, 30., 100));
    drop(seed(&unrelated, 90., 100));
    cmd.repairs.push(repairs);
    cmd.remove_inputs = true;
    assert!(
        run(&cmd, &mut |_| {})
            .unwrap()
            .contains("Retired 1 input databases, retained 1")
    );
    assert!(cmd.caches[0].exists() && unrelated.exists() && !replacement.exists());
    let db = open_read(&cmd.output).unwrap();
    assert_eq!(
        db.query_row("SELECT elevation_m FROM contour_tiles", [], |r| r
            .get::<_, f64>(0))
            .unwrap(),
        30.
    );
    assert_eq!(receipts(&db).unwrap().len(), 2);
}

#[test]
fn tampered_destination_and_partial_source_never_trigger_deletion() {
    let dir = tempdir().unwrap();
    let mut cmd = command(dir.path());
    let db = seed(&cmd.caches[0], 20., 100);
    db.execute("UPDATE contour_tile_manifest SET contour_count=0", [])
        .unwrap();
    drop(db);
    cmd.remove_inputs = true;
    assert!(run(&cmd, &mut |_| {}).unwrap().contains("retained 1"));
    assert!(cmd.caches[0].exists());
    let output = Connection::open(&cmd.output).unwrap();
    assert_eq!(receipts(&output).unwrap().len(), 1);
    output
        .execute("UPDATE cache_recovery_digests SET digest=zeroblob(32)", [])
        .unwrap();
    drop(output);
    cmd.cleanup_only = true;
    assert!(run(&cmd, &mut |_| {}).is_err() && cmd.caches[0].exists());
}

#[test]
fn unknown_columns_and_busy_writer_are_retained() {
    let dir = tempdir().unwrap();
    let mut cmd = command(dir.path());
    let db = seed(&cmd.caches[0], 20., 100);
    cmd.remove_inputs = true;
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(run(&cmd, &mut |_| {}).unwrap().contains("retained 1"));
    db.execute_batch("ROLLBACK; ALTER TABLE contour_tiles ADD COLUMN survey_note TEXT")
        .unwrap();
    drop(db);
    cmd.cleanup_only = true;
    assert!(run(&cmd, &mut |_| {}).unwrap().contains("retained 1"));
    assert!(cmd.caches[0].exists());
}

#[test]
fn recovers_geometry_after_a_broken_lookup_index() {
    let dir = tempdir().unwrap();
    let cmd = command(dir.path());
    let db = seed(&cmd.caches[0], 20., 100);
    let index_page: u64 = db
        .query_row(
            "SELECT rootpage FROM sqlite_schema WHERE name='sqlite_autoindex_contour_tiles_1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let page_size: u64 = db.query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap();
    db.execute_batch("PRAGMA journal_mode=DELETE").unwrap();
    drop(db);
    use std::io::{Seek, SeekFrom, Write};
    let mut file = fs::OpenOptions::new()
        .write(true)
        .open(&cmd.caches[0])
        .unwrap();
    file.seek(SeekFrom::Start((index_page - 1) * page_size))
        .unwrap();
    file.write_all(&[0xfe]).unwrap();
    drop(file);
    let mut cmd = cmd;
    cmd.remove_inputs = true;
    assert!(run(&cmd, &mut |_| {}).unwrap().contains("retained 1"));
    assert!(cmd.caches[0].exists());
    let output = open_read(&cmd.output).unwrap();
    assert_eq!(
        output
            .query_row("SELECT COUNT(*) FROM contour_tiles", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn staged_publication_and_fast_cleanup_require_an_identical_copy() {
    let dir = tempdir().unwrap();
    let mut cmd = command(dir.path());
    drop(seed(&cmd.caches[0], 20., 100));
    run(&cmd, &mut |_| {}).unwrap();
    let stage = dir.path().join(".contour-recovery-finished");
    fs::rename(&cmd.output, &stage).unwrap();
    let fast = dir.path().join("ssd.sqlite");
    let finish = Command {
        output: cmd.output.clone(),
        finish_staged: Some(stage.clone()),
        verification_copy: Some(fast.clone()),
        ..Default::default()
    };
    run(&finish, &mut |_| {}).unwrap();
    assert!(fast.exists() && cmd.output.exists() && !stage.exists());
    let db = Connection::open(&fast).unwrap();
    db.execute("UPDATE contour_tiles SET elevation_m=25", [])
        .unwrap();
    drop(db);
    cmd.cleanup_only = true;
    cmd.verification_copy = Some(fast);
    assert!(run(&cmd, &mut |_| {}).is_err() && cmd.caches[0].exists());
}

#[test]
fn unfinished_stage_cannot_be_published() {
    let dir = tempdir().unwrap();
    let stage = dir.path().join(".contour-recovery-incomplete");
    drop(seed(&stage, 40., 100));
    let cmd = Command {
        output: dir.path().join(NATIVE),
        finish_staged: Some(stage.clone()),
        ..Default::default()
    };
    assert!(run(&cmd, &mut |_| {}).is_err());
    assert!(stage.exists() && !cmd.output.exists());
}
