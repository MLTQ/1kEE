use super::*;

fn info() -> Info {
    Info {
        source: "gem-gas".into(),
        source_id: "P123".into(),
        name: "Test route".into(),
        operator: "".into(),
        owner: "Example owner".into(),
        product: "gas".into(),
        status: "operating".into(),
        accuracy: "Approximate".into(),
        historical: false,
        planned: false,
    }
}

#[test]
fn cell_edges_are_continuous_and_reentries_remain_separate() {
    let parts = geometry::split(&[[0.1, 0.1], [0.6, 0.1], [0.1, 0.1]]);
    assert_eq!(
        parts.iter().map(|p| p.0).collect::<Vec<_>>(),
        vec![(0, 0), (0, 1), (0, 2), (0, 1), (0, 0)]
    );
    for pair in parts.windows(2) {
        assert_eq!(pair[0].1.last(), pair[1].1.first());
    }
    assert_eq!(parts[0].1[0].lon, 0.1);
    assert_eq!(parts.last().unwrap().1.last().unwrap().lon, 0.1);
}

#[test]
fn date_line_does_not_create_a_world_spanning_segment() {
    for points in [
        vec![[179.9, 0.0], [-179.9, 0.0]],
        vec![[-179.9, 0.0], [179.9, 0.0]],
    ] {
        let parts = geometry::split(&points);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].1.last().unwrap().lon.abs(), 180.0);
        assert_eq!(parts[1].1.first().unwrap().lon.abs(), 180.0);
        for (cell, line) in parts {
            assert!((-720..720).contains(&cell.1));
            assert!((line[0].lon - line.last().unwrap().lon).abs() < 0.3);
        }
    }
    let poles = geometry::split(&[[179.9, 90.0], [180.0, 90.0]]);
    assert_eq!(poles[0].0, (359, 719));
}

#[test]
fn source_and_status_filters_keep_unknowns_honest() {
    let mut record = info();
    let mut filter = Filter::default();
    record.status = "unknown".into();
    assert!(filter.accepts(&record));
    record.historical = true;
    assert!(!filter.accepts(&record));
    filter.historical = true;
    assert!(filter.accepts(&record));
    filter.gem = false;
    assert!(!filter.accepts(&record));
    filter.gem = true;
    record.planned = true;
    assert!(!filter.accepts(&record));
}

#[test]
fn pack_read_merge_and_failed_publication_preserve_sources() {
    let root = std::env::temp_dir().join(format!(
        "pipeline-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let input = root.join("routes.jsonl");
    let manifest = root.join("manifest.json");
    let archive = root.join("pipelines.1ka");
    let route = serde_json::json!({"info":info(),"points":[[0.1,0.1],[0.3,0.12],[0.6,0.1]]});
    std::fs::write(&input, format!("{route}\n")).unwrap();
    std::fs::write(
        &manifest,
        r#"{"version":1,"sources":[{"id":"gem-gas","license":"CC BY 4.0"}]}"#,
    )
    .unwrap();
    build::run(&input, &manifest, &archive, &mut |_| {}).unwrap();
    let reader = Reader::open(&archive).unwrap();
    let fine = load(&reader, Some([0.09, 0.13, 0.1, 0.6])).unwrap();
    assert_eq!(fine.len(), 3);
    assert_eq!(fine[0].info.owner, "Example owner");
    assert_eq!(fine[0].info.accuracy, "Approximate");
    assert_eq!(fine[0].points[0].lon, 0.1);
    assert_eq!(load(&reader, None).unwrap().len(), 1);
    assert!(
        load(&reader, Some([10.0, 10.1, 10.0, 10.1]))
            .unwrap()
            .is_empty()
    );
    let world = root.join("world.1ka");
    let mut writer = Writer::create(&world).unwrap();
    crate::vector::pack_cell(&mut writer, *b"PIPE", 0, 0, &[]).unwrap();
    assert_eq!(copy_into(&reader, &mut writer).unwrap(), 4);
    writer.finish().unwrap();
    let world = Reader::open(&world).unwrap();
    assert!(world.has_cell(*b"PIPE", 0, 0).unwrap());
    assert_eq!(load(&world, None).unwrap()[0].info.source_id, "P123");
    let original = std::fs::read(&archive).unwrap();
    assert!(build::run(&input, &manifest, &archive, &mut |_| {}).is_err());
    assert_eq!(original, std::fs::read(&archive).unwrap());
    std::fs::write(
        &input,
        format!(
            "{}\n",
            serde_json::json!({"info":info(),"points":[[181.0,0.0],[0.0,0.0]]})
        ),
    )
    .unwrap();
    let failed = root.join("failed.1ka");
    assert!(build::run(&input, &manifest, &failed, &mut |_| {}).is_err());
    assert!(!failed.exists());
    assert!(!std::fs::read_dir(&root).unwrap().any(|p| {
        p.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".pipelines-")
    }));
    drop(reader);
    drop(world);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "read-only real archive QA: set ONEKEE_PIPELINE_ARCHIVE"]
fn verify_real_pipeline_archive_and_measure_reads() {
    let path = std::env::var("ONEKEE_PIPELINE_ARCHIVE").expect("ONEKEE_PIPELINE_ARCHIVE");
    let reader = Reader::open(std::path::Path::new(&path)).unwrap();
    let start = std::time::Instant::now();
    let overview = load(&reader, None).unwrap();
    println!(
        "Overview: {} parts / {} vertices in {:?}",
        overview.len(),
        overview.iter().map(|f| f.points.len()).sum::<usize>(),
        start.elapsed()
    );
    assert!(!overview.is_empty());
    for (name, bounds) in [
        ("Houston", [29.65, 29.85, -95.5, -95.3]),
        ("Louisiana offshore", [28.0, 28.2, -90.2, -90.0]),
        ("Netherlands", [51.8, 52.0, 4.2, 4.4]),
        ("Tokyo", [35.4, 36.0, 139.4, 140.0]),
    ] {
        let start = std::time::Instant::now();
        let features = load(&reader, Some(bounds)).unwrap();
        println!(
            "{name}: {} parts / {} vertices in {:?}",
            features.len(),
            features.iter().map(|f| f.points.len()).sum::<usize>(),
            start.elapsed()
        );
        assert!(!features.is_empty(), "{name}");
    }
    let mut stmt = reader
        .connection
        .prepare("SELECT y,x FROM tiles WHERE layer=?1 AND grid=3 AND level=0")
        .unwrap();
    let cells = stmt
        .query_map([&TAG[..]], |r| {
            Ok((r.get::<_, i32>(0)?, r.get::<_, i32>(1)?))
        })
        .unwrap();
    let mut count = 0;
    let mut vertices = 0;
    for cell in cells {
        let (y, x) = cell.unwrap();
        let features = decode(&reader.get(key(0, y, x)).unwrap().unwrap()).unwrap();
        for feature in features {
            for p in feature.points {
                assert!(
                    p.lat >= y as f32 / 4.0 - 0.00002 && p.lat <= (y + 1) as f32 / 4.0 + 0.00002,
                    "Latitude outside tile"
                );
                assert!(
                    p.lon >= x as f32 / 4.0 - 0.00002 && p.lon <= (x + 1) as f32 / 4.0 + 0.00002,
                    "Longitude outside tile"
                );
                vertices += 1;
            }
        }
        count += 1;
    }
    println!("Verified {count} full-detail tile checksums, attributes and {vertices} vertices");
}
