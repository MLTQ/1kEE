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
        drop(session);
        drop(lookup);
        drop(_lock);
        fs::remove_dir_all(root).unwrap();
    }
}
