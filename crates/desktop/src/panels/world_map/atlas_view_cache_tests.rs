use super::*;
fn fixture(dir: &Path, region: Region) -> (Ticket, Arc<Raster>) {
    let input = dir.join("source.tif");
    if !input.exists() {
        fs::write(&input, b"synthetic source identity").unwrap();
    }
    let ticket = Ticket::new(dir, region, [8, 4], vec![input]).unwrap();
    let values: Vec<f32> = (0..32)
        .map(|i| {
            if i == 2 {
                f32::from_bits(0x7fc00123)
            } else {
                (i as f32 - 16.) * 137.21
            }
        })
        .collect();
    let raster = Raster::new(
        HeightGrid {
            width: 8,
            height: 4,
            metres: values,
        },
        region.bounds(),
    )
    .unwrap();
    (ticket, raster)
}
#[test]
fn recent_view_cache_roundtrip_is_lossless_and_source_sensitive() {
    let dir = tempfile::tempdir().unwrap();
    let (ticket, raster) = fixture(dir.path(), Region([0, 0, 360, 360]));
    assert!(ticket.read().unwrap().is_none());
    ticket.store(&raster).unwrap();
    let loaded = ticket.read().unwrap().unwrap();
    assert_eq!(
        loaded
            .metres
            .iter()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>(),
        raster
            .heights
            .iter()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>()
    );
    fs::write(&ticket.files[0], b"changed original terrain").unwrap();
    let changed =
        Ticket::new(dir.path(), ticket.region, ticket.size, ticket.files.clone()).unwrap();
    assert!(changed.read().unwrap().is_none());
    assert!(
        ticket.store(&raster).is_err(),
        "a source changing during a load cannot be published under the earlier identity"
    );
}
#[test]
fn recent_view_cache_detects_corruption_and_replaces_only_its_entry() {
    let dir = tempfile::tempdir().unwrap();
    let (ticket, raster) = fixture(dir.path(), Region([0, 0, 360, 360]));
    ticket.store(&raster).unwrap();
    Connection::open(&ticket.path)
        .unwrap()
        .execute("UPDATE views SET crc=crc+1", [])
        .unwrap();
    assert!(ticket.read().is_err());
    ticket.store(&raster).unwrap();
    assert!(ticket.read().unwrap().is_some());
    Connection::open(&ticket.path)
        .unwrap()
        .execute("UPDATE views SET payload=x'00'", [])
        .unwrap();
    assert!(ticket.read().is_err());
    ticket.store(&raster).unwrap();
    assert!(ticket.read().unwrap().is_some());
}
#[test]
fn recent_view_cache_bounds_retention_and_rejects_unrelated_database() {
    let dir = tempfile::tempdir().unwrap();
    let (first, _) = fixture(dir.path(), Region([0, 0, 360, 360]));
    for n in 0..MAX_VIEWS + 1 {
        let (ticket, raster) = fixture(
            dir.path(),
            Region([n as i32 * 360, 0, (n as i32 + 1) * 360, 360]),
        );
        ticket.store(&raster).unwrap();
    }
    let c = Connection::open(&first.path).unwrap();
    let count: usize = c
        .query_row("SELECT count(*) FROM views", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, MAX_VIEWS);
    assert!(first.read().unwrap().is_none());
    drop(c);
    let other = tempfile::tempdir().unwrap();
    let (ticket, raster) = fixture(other.path(), Region([0, 0, 360, 360]));
    fs::create_dir_all(ticket.path.parent().unwrap()).unwrap();
    Connection::open(&ticket.path)
        .unwrap()
        .execute("CREATE TABLE unrelated(value TEXT)", [])
        .unwrap();
    let before = fs::read(&ticket.path).unwrap();
    assert!(ticket.store(&raster).is_err());
    assert_eq!(fs::read(&ticket.path).unwrap(), before);
}
