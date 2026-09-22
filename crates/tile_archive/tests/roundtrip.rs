use cell_format::{CellFeature, CellPoint, write::write_cell};
use tile_archive::{Key, Reader, Writer, contours, vector};

fn root() -> std::path::PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!("1kee-archive-test-{}-{n}", std::process::id()));
    std::fs::create_dir(&p).unwrap();
    p
}
fn feature(id: i64, coords: &[(f32, f32)], polygon: bool) -> CellFeature {
    CellFeature {
        way_id: id,
        class: 3,
        is_polygon: polygon,
        name: Some("Élevation".into()),
        points: coords
            .iter()
            .map(|&(lat, lon)| CellPoint { lat, lon })
            .collect(),
        elevations: Some(
            coords
                .iter()
                .enumerate()
                .map(|(i, _)| i as f32 - 0.25)
                .collect(),
        ),
    }
}
#[test]
fn subtiles_preserve_crossing_geometry_heights_and_empty_coverage() {
    let root = root();
    let path = root.join("world.1ka");
    let mut writer = Writer::create(&path).unwrap();
    let features = vec![
        feature(1, &[(-0.95, -0.95), (-0.05, -0.05)], false),
        feature(
            2,
            &[
                (-0.9, -0.9),
                (-0.1, -0.9),
                (-0.1, -0.1),
                (-0.9, -0.1),
                (-0.9, -0.9),
            ],
            true,
        ),
        feature(3, &[(-0.95, -0.95), (-0.94, -0.94)], false),
    ];
    vector::pack_cell(&mut writer, *b"ROAD", -1, -1, &features).unwrap();
    vector::pack_cell(&mut writer, *b"WATR", -1, -1, &[]).unwrap();
    writer.finish().unwrap();
    let reader = Reader::open(&path).unwrap();
    assert!(vector::prefer_subtiles(-1, -1, [-0.6, -0.5, -0.6, -0.5]));
    assert!(!vector::prefer_subtiles(-1, -1, [-1.0, 0.0, -1.0, 0.0]));
    let mut actual = vector::read_cell(&reader, *b"ROAD", -1, -1, [-0.6, -0.4, -0.6, -0.4])
        .unwrap()
        .unwrap();
    actual.sort_by_key(|f| f.way_id);
    assert_eq!(
        write_cell(-1, -1, &[(*b"ROAD", &actual)]),
        write_cell(-1, -1, &[(*b"ROAD", &features[..2])])
    );
    assert!(
        vector::read_cell(&reader, *b"WATR", -1, -1, [-1.0, 0.0, -1.0, 0.0])
            .unwrap()
            .unwrap()
            .is_empty()
    );
    assert!(
        vector::read_cell(&reader, *b"ROAD", 0, 0, [0.0, 1.0, 0.0, 1.0])
            .unwrap()
            .is_none()
    );
    // Separate reader connections can concurrently read the published snapshot.
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let path = &path;
            scope.spawn(move || {
                let r = Reader::open(path).unwrap();
                assert_eq!(
                    vector::read_cell(&r, *b"ROAD", -1, -1, [-1.0, 0.0, -1.0, 0.0])
                        .unwrap()
                        .unwrap()
                        .len(),
                    3
                );
            });
        }
    });
    drop(reader);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn contours_are_exact_body_scoped_and_corruption_is_rejected() {
    let root = root();
    let path = root.join("world.1ka");
    let mut writer = Writer::create(&path).unwrap();
    let rows = vec![(0.0, vec![0, 1, 2, 3]), (-10.5, vec![9, 8])];
    let bytes = contours::encode(&rows).unwrap();
    let key = Key {
        body: 1,
        layer: *b"CNTR",
        grid: 2,
        level: 10,
        y: 2,
        x: 3,
    };
    writer.put_batch(&[(key, bytes.clone())], None).unwrap();
    writer.finish().unwrap();
    let reader = Reader::open(&path).unwrap();
    assert!(reader.get(Key { body: 0, ..key }).unwrap().is_none());
    assert_eq!(reader.get(key).unwrap().unwrap(), bytes);
    let decoded = contours::decode(&bytes).unwrap();
    for ((e, g), (de, dg)) in rows.iter().zip(decoded) {
        assert_eq!(e.to_bits(), de.to_bits());
        assert_eq!(
            tile_archive::gpkg::parse_gpkg_lines(g, |x, y| (x, y)),
            contours::decode_lines(dg, |x, y| (x, y)).unwrap()
        );
    }
    assert!(contours::decode(&bytes[..bytes.len() - 1]).is_err());
    let mut invalid = b"CTP1".to_vec();
    invalid.extend_from_slice(&u32::MAX.to_le_bytes());
    assert!(contours::decode(&invalid).is_err());
    drop(reader);
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute("UPDATE tiles SET payload=x'00'", []).unwrap();
    drop(conn);
    assert!(Reader::open(&path).unwrap().get(key).is_err());
    assert!(Writer::create(&path).is_err());
    std::fs::remove_dir_all(root).unwrap();
}
