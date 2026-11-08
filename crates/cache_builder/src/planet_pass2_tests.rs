//! Exercise the coordinate-I/O boundary with a real indexed PBF.
use super::*;

#[test]
fn unselected_way_blobs_do_not_decompress_nodes_and_still_report_progress() {
    for fixture in [
        include_bytes!("../tests/fixtures/planet-tiny-plain.osm.pbf").as_slice(),
        include_bytes!("../tests/fixtures/planet-tiny-dense.osm.pbf").as_slice(),
    ] {
        let root = std::env::temp_dir().join(format!(
            "1kee-selected-prefetch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let source = root.join("source.pbf");
        fs::write(&source, fixture).unwrap();
        let (nodes, _lock) =
            crate::pbf_nodes::PbfNodes::open(&source, &root.join("index"), &mut |_| {}).unwrap();
        let lookup = Arc::new(Lookup::Compact(nodes));
        let crate::args::Command::PlanetAll(mut cmd) = crate::args::parse([
            "planet-all".into(),
            "--planet".into(),
            source.display().to_string(),
            "--out-dir".into(),
            root.join("out").display().to_string(),
            "--features".into(),
            "waterways".into(),
        ])
        .unwrap() else {
            panic!("planet command")
        };
        let mut session = lookup.session();
        let mut scanned = 0;
        for blob in open_planet_at(&source, 0).unwrap().0 {
            let out = process_blob(&blob.unwrap(), &mut session, &cmd).unwrap();
            scanned += out.scanned_ways;
            assert_eq!(out.feature_count, 0);
        }
        assert_eq!(scanned, 2, "rejected ways still count as scanned");
        let LookupSession::Compact(nodes) = &session else {
            unreachable!()
        };
        assert_eq!(
            nodes.decoded_blocks, 0,
            "unselected ways must not prepare coordinates"
        );

        let mut messages = Vec::new();
        let mut cp = Checkpoint::default();
        let stats = run_pass2(&cmd, &lookup, &mut cp, &root.join("ways.txt"), &mut |p| {
            messages.push(p)
        })
        .unwrap();
        assert_eq!(stats.written_cells, 0);
        assert!(
            messages.iter().any(|p| p.stage == "Scanning Ways"
                && p.message.contains("2 ways scanned; 0 selected features")),
            "progress must not wait for a feature flush"
        );

        cmd.build_roads = true;
        let mut features = 0;
        for blob in open_planet_at(&source, 0).unwrap().0 {
            features += process_blob(&blob.unwrap(), &mut session, &cmd)
                .unwrap()
                .feature_count;
        }
        assert_eq!(features, 1);
        let LookupSession::Compact(nodes) = &session else {
            unreachable!()
        };
        assert!(
            nodes.decoded_blocks > 0,
            "selected roads still resolve coordinates"
        );
        let (blocks, bytes, nodes) = lookup.activity();
        assert!(blocks > 0 && bytes > 0 && nodes > 0);
        drop(session);
        drop(lookup);
        drop(_lock);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn planet_monitor_records_success_and_failure_without_changing_cache_contract() {
    let root = std::env::temp_dir().join(format!("1kee-monitor-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let source = root.join("source.pbf");
    fs::write(
        &source,
        include_bytes!("../tests/fixtures/planet-tiny-plain.osm.pbf"),
    )
    .unwrap();
    let crate::args::Command::PlanetAll(mut cmd) = crate::args::parse([
        "planet-all".into(),
        "--planet".into(),
        source.display().to_string(),
        "--out-dir".into(),
        root.join("out").display().to_string(),
        "--features".into(),
        "roads".into(),
    ])
    .unwrap() else {
        panic!("planet command")
    };
    let mut metrics = Vec::new();
    build_planet_cache_with_progress(cmd.clone(), &mut |p| {
        if let Some(m) = p.metrics {
            metrics.push(m);
        }
    })
    .unwrap();
    assert!(
        metrics
            .iter()
            .all(|m| m.checkpoint_bytes <= m.scanned_bytes && m.scanned_bytes <= m.total_bytes)
    );
    let final_metrics = metrics.last().unwrap();
    assert_eq!(final_metrics.ways, 2);
    assert_eq!(final_metrics.features, 1);
    assert!(final_metrics.cell_writes > 0);
    let report = || {
        serde_json::from_slice::<crate::build_monitor::Report>(
            &fs::read(cmd.tmp_dir.join("build-status.json")).unwrap(),
        )
        .unwrap()
    };
    assert_eq!(report().state, "completed");
    assert_eq!(report().metrics.unwrap().features, 1);
    cmd.planet_path = root.join("missing.pbf");
    assert!(build_planet_cache_with_progress(cmd.clone(), &mut |_| {}).is_err());
    assert_eq!(report().state, "failed");
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "operator benchmark; copied index in /tmp and read-only original source"]
fn inspect_operator_way_batch_dependencies() {
    let source = std::path::PathBuf::from(std::env::var("ONEKEE_BENCH_SOURCE").unwrap());
    let index = std::path::PathBuf::from(std::env::var("ONEKEE_BENCH_INDEX").unwrap());
    let offset = std::env::var("ONEKEE_BENCH_OFFSET")
        .unwrap()
        .parse()
        .unwrap();
    let (nodes, _guard) = crate::pbf_nodes::PbfNodes::open(&source, &index, &mut |_| {}).unwrap();
    let crate::args::Command::PlanetAll(cmd) = crate::args::parse([
        "planet-all".into(),
        "--planet".into(),
        source.display().to_string(),
        "--out-dir".into(),
        "/tmp/onekee-lookup-unused-output".into(),
        "--features".into(),
        "all".into(),
    ])
    .unwrap() else {
        panic!()
    };
    let mut union = std::collections::HashSet::new();
    let mut sum_bytes = 0;
    let mut sum_blocks = 0;
    let mut oversized = 0;
    for (i, blob) in open_planet_at(&source, offset)
        .unwrap()
        .0
        .take(64)
        .enumerate()
    {
        let BlobDecode::OsmData(block) = blob.unwrap().decode().unwrap() else {
            continue;
        };
        let refs: Vec<_> = block
            .groups()
            .flat_map(|g| g.ways())
            .filter(|w| selection::classify(w, &cmd).is_some())
            .flat_map(|w| w.refs())
            .collect();
        let (blocks, bytes) = nodes.estimated_reads(&refs);
        sum_bytes += bytes;
        sum_blocks += blocks;
        if refs.len() > 512 * 1024 {
            oversized += 1;
        }
        println!(
            "blob {i}: refs={} blocks={blocks} bytes={bytes}",
            refs.len()
        );
        union.extend(refs);
    }
    let (blocks, bytes) = nodes.estimated_reads(&union.into_iter().collect::<Vec<_>>());
    println!(
        "TOTAL per-blob blocks={sum_blocks} bytes={sum_bytes}; union blocks={blocks} bytes={bytes}; oversized={oversized}"
    );
}

#[test]
fn shared_batches_match_legacy_geometry_and_split_without_lru_fallback() {
    let root = std::env::temp_dir().join(format!("1kee-shared-batch-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let source = root.join("source.pbf");
    fs::write(
        &source,
        include_bytes!("../tests/fixtures/planet-tiny-plain.osm.pbf"),
    )
    .unwrap();
    let (nodes, _guard) =
        crate::pbf_nodes::PbfNodes::open(&source, &root.join("index"), &mut |_| {}).unwrap();
    let lookup = Lookup::Compact(nodes);
    let crate::args::Command::PlanetAll(cmd) = crate::args::parse([
        "planet-all".into(),
        "--planet".into(),
        source.display().to_string(),
        "--out-dir".into(),
        root.join("out").display().to_string(),
        "--features".into(),
        "all".into(),
    ])
    .unwrap() else {
        panic!()
    };
    let blobs: Vec<_> = (0..3)
        .flat_map(|_| open_planet_at(&source, 0).unwrap().0.map(Result::unwrap))
        .collect();
    let limit = blobs
        .iter()
        .map(|b| selected_refs(b, &cmd).unwrap().len())
        .max()
        .unwrap()
        .max(1);
    let expected: Vec<_> = blobs
        .iter()
        .map(|b| process_blob(b, &mut lookup.session(), &cmd).unwrap())
        .collect();
    let actual = process_batch_bounded(&blobs, &lookup, &cmd, &|_| {}, limit).unwrap();
    for (a, b) in actual.iter().zip(&expected) {
        assert_eq!(a.feature_count, b.feature_count);
        assert_eq!(a.scanned_ways, b.scanned_ways);
        for (cell, roads) in &a.roads {
            let other = &b.roads[cell];
            assert_eq!(roads.len(), other.len());
            for (a, b) in roads.iter().zip(other) {
                assert_eq!(a.way_id, b.way_id);
                assert_eq!(a.points.len(), b.points.len());
                for (a, b) in a.points.iter().zip(&b.points) {
                    assert_eq!((a.lat, a.lon), (b.lat, b.lon));
                }
            }
        }
    }
    assert!(process_batch_bounded(&blobs, &lookup, &cmd, &|_| {}, 0).is_err());
    drop(lookup);
    drop(_guard);
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "operator benchmark; original source is read-only"]
fn benchmark_operator_shared_coordinates() {
    let source = std::path::PathBuf::from(std::env::var("ONEKEE_BENCH_SOURCE").unwrap());
    let index = std::path::PathBuf::from(std::env::var("ONEKEE_BENCH_INDEX").unwrap());
    let offset = std::env::var("ONEKEE_BENCH_OFFSET")
        .unwrap()
        .parse()
        .unwrap();
    let (nodes, _guard) = crate::pbf_nodes::PbfNodes::open(&source, &index, &mut |_| {}).unwrap();
    let groups: Vec<Vec<i64>> = open_planet_at(&source, offset)
        .unwrap()
        .0
        .take(4)
        .map(|blob| {
            let BlobDecode::OsmData(block) = blob.unwrap().decode().unwrap() else {
                return Vec::new();
            };
            block
                .groups()
                .flat_map(|g| g.ways())
                .take(128)
                .flat_map(|w| w.refs())
                .collect()
        })
        .collect();
    let mut reference = None;
    // Repeat in reverse order to expose warm-cache effects, without dropping
    // the host's page cache or modifying the original source/cache.
    for shared in [false, true, true, false] {
        let before = nodes.activity();
        let start = std::time::Instant::now();
        let result = if shared {
            let prepared = nodes
                .prepare_batch(groups.iter().flatten().copied().collect())
                .unwrap();
            groups
                .par_iter()
                .map(|ids| {
                    nodes
                        .prepared_session(prepared.clone())
                        .lookup_many(ids)
                        .unwrap()
                })
                .collect::<Vec<_>>()
        } else {
            groups
                .par_iter()
                .map(|ids| {
                    let mut session = nodes.session();
                    session.prefetch(ids.iter().copied()).unwrap();
                    session.lookup_many(ids).unwrap()
                })
                .collect::<Vec<_>>()
        };
        let after = nodes.activity();
        println!(
            "shared={shared} seconds={:.3} blocks={} bytes={} refs={}",
            start.elapsed().as_secs_f64(),
            after.0 - before.0,
            after.1 - before.1,
            groups.iter().map(Vec::len).sum::<usize>()
        );
        if let Some(expected) = &reference {
            assert_eq!(&result, expected);
        } else {
            reference = Some(result);
        }
    }
}
