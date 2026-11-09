use super::*;
use osmpbf::{BlobDecode, BlobReader};
use std::io::{Cursor, SeekFrom};
use std::sync::atomic::{AtomicU64, Ordering};
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "1kee-join-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture(root: &Path, repeats: usize) -> PlanetAllCommand {
    let fixture = include_bytes!("../tests/fixtures/planet-tiny-plain.osm.pbf");
    let mut reader = BlobReader::new_seekable(Cursor::new(fixture)).unwrap();
    let mut start = 0;
    let mut bytes = Vec::new();
    while let Some(blob) = reader.next() {
        let blob = blob.unwrap();
        let end = reader.seek_raw(SeekFrom::Current(0)).unwrap() as usize;
        let ways = matches!(blob.decode().unwrap(),BlobDecode::OsmData(b) if b.groups().flat_map(|g|g.ways()).next().is_some());
        for _ in 0..if ways { repeats } else { 1 } {
            bytes.extend_from_slice(&fixture[start..end]);
        }
        start = end;
    }
    let source = root.join("source.pbf");
    fs::write(&source, bytes).unwrap();
    let crate::args::Command::PlanetAll(cmd) = crate::args::parse([
        "planet-all".into(),
        "--planet".into(),
        source.display().to_string(),
        "--out-dir".into(),
        root.join("out").display().to_string(),
        "--tmp-dir".into(),
        root.join("tmp").display().to_string(),
        "--features".into(),
        "all".into(),
        "--node-storage".into(),
        "indexed-pbf".into(),
    ])
    .unwrap() else {
        panic!()
    };
    cmd
}
fn empty(cmd: &PlanetAllCommand, nodes: &PbfNodes, start: u64) -> Join {
    let root = cmd.tmp_dir.join("indexed-pbf-v1/way-join-v1");
    fs::create_dir_all(&root).unwrap();
    let state = State {
        version: 1,
        identity: nodes.join_identity(),
        options: "test".into(),
        start,
        scan: start,
        collected: false,
        joined: 0,
        requests: vec![Spool::default(); nodes.block_count().div_ceil(BLOCKS_PER_SHARD)],
        groups: Vec::new(),
    };
    save(&root, &state).unwrap();
    Join { root, state }
}
#[test]
fn join_decodes_once_across_groups_and_matches_legacy_geometry() {
    let temp = Temp::new();
    let cmd = fixture(&temp.0, 24);
    let (nodes, _guard) = PbfNodes::open(
        &cmd.planet_path,
        &cmd.tmp_dir.join("indexed-pbf-v1"),
        &mut |_| {},
    )
    .unwrap();
    let mut join = empty(&cmd, &nodes, nodes.first_way);
    join.collect(&cmd, &nodes, &mut |_| {}, 1).unwrap();
    assert_eq!(join.state.groups.len(), 24);
    let before = nodes.activity();
    join.resolve_window(&nodes, &mut |_| {}, 3).unwrap();
    assert_eq!(
        nodes.activity().0 - before.0,
        1,
        "all 24 groups share one original node decode"
    );
    let lookup = crate::planet_lookup::Lookup::Compact(nodes);
    for group in &join.state.groups {
        let (_, table) = join.group(group.start).unwrap().unwrap();
        let blob = open_planet_at(&cmd.planet_path, group.start)
            .unwrap()
            .0
            .next()
            .unwrap()
            .unwrap();
        let a = super::super::process_blob(&blob, &mut lookup.session(), &cmd).unwrap();
        let prepared = Some(table);
        let mut session = lookup.prepared_session(&prepared);
        let before = lookup.activity();
        let b = super::super::process_blob(&blob, &mut session, &cmd).unwrap();
        assert_eq!(lookup.activity(), before);
        assert_eq!(a.feature_count, b.feature_count);
        assert_eq!(a.scanned_ways, b.scanned_ways);
        for (cell, roads) in &a.roads {
            for (a, b) in roads.iter().zip(&b.roads[cell]) {
                assert_eq!(a.way_id, b.way_id);
                assert_eq!(a.points.len(), b.points.len());
                for (a, b) in a.points.iter().zip(&b.points) {
                    assert_eq!((a.lat, a.lon), (b.lat, b.lon));
                }
            }
        }
        assert_eq!(
            session.lookup_many(&[-99, 99999999]).unwrap(),
            vec![None, None]
        );
        assert_eq!(lookup.activity(), before);
    }
}
#[test]
fn interrupted_collection_and_join_restore_only_uncommitted_tails() {
    let temp = Temp::new();
    let cmd = fixture(&temp.0, 4);
    let (nodes, _guard) = PbfNodes::open(
        &cmd.planet_path,
        &cmd.tmp_dir.join("indexed-pbf-v1"),
        &mut |_| {},
    )
    .unwrap();
    let mut join = empty(&cmd, &nodes, nodes.first_way);
    let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        join.collect(
            &cmd,
            &nodes,
            &mut |_| panic!("injected stop after group commit"),
            1,
        )
    }));
    assert!(stopped.is_err());
    let saved: State =
        serde_json::from_slice(&fs::read(join.root.join("state.json")).unwrap()).unwrap();
    assert_eq!(saved.groups.len(), 1);
    let request = Sinks::path(&join.root.join("requests"), 0);
    std::fs::OpenOptions::new()
        .append(true)
        .open(&request)
        .unwrap()
        .write_all(b"uncommitted tail")
        .unwrap();
    let resumed = Join::prepare(&cmd, &nodes, nodes.first_way, "test", &mut |_| {})
        .unwrap()
        .unwrap();
    assert_eq!(resumed.state.joined, resumed.state.requests.len());
    assert!(!request.exists());
    let group_path = Sinks::path(&resumed.root.join("coordinates"), 0);
    let bytes = fs::read(&group_path).unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&group_path)
        .unwrap()
        .write_all(b"uncommitted tail")
        .unwrap();
    let before = nodes.activity();
    let again = Join::prepare(&cmd, &nodes, nodes.first_way, "test", &mut |_| {})
        .unwrap()
        .unwrap();
    assert_eq!(nodes.activity(), before);
    assert_eq!(fs::read(&group_path).unwrap(), bytes);
    let mut corrupt = bytes;
    corrupt[8] ^= 1;
    fs::write(&group_path, &corrupt).unwrap();
    assert!(
        again
            .group(again.start())
            .err()
            .unwrap()
            .contains("checksum")
    );
    assert!(
        Join::prepare(
            &cmd,
            &nodes,
            nodes.first_way,
            "different settings",
            &mut |_| {}
        )
        .is_err()
    );
}
#[test]
fn external_sort_merges_many_runs_and_rejects_corruption() {
    let temp = Temp::new();
    let mut sinks = Sinks::new(&temp.0.join("input"), 1).unwrap();
    let mut spools = vec![Spool::default()];
    let mut expected = Vec::new();
    for n in (0..300).rev() {
        let r = Record {
            id: n % 37,
            value: n as u64,
        };
        expected.push(r);
        sinks.push(0, r).unwrap();
    }
    sinks.finish(&mut spools).unwrap();
    expected.sort_unstable();
    let path = Sinks::path(&temp.0.join("input"), 0);
    let mut sorted = io::sorted(&path, &spools[0], &temp.0.join("sort"), 3, &mut |_| {}).unwrap();
    let mut actual = Vec::new();
    while let Some(r) = sorted.next().unwrap() {
        actual.push(r);
    }
    assert_eq!(actual, expected);
    let mut bytes = fs::read(&path).unwrap();
    bytes[0] ^= 1;
    fs::write(&path, bytes).unwrap();
    assert!(io::sorted(&path, &spools[0], &temp.0.join("sort2"), 3, &mut |_| {}).is_err());
    fs::write(&path, b"short").unwrap();
    assert!(spools[0].restore(&path).is_err());
}

#[test]
#[ignore = "operator benchmark; read-only planet and disposable copied index"]
fn benchmark_operator_sequential_join() {
    let source = PathBuf::from(std::env::var("ONEKEE_BENCH_SOURCE").unwrap());
    let index = PathBuf::from(std::env::var("ONEKEE_BENCH_INDEX").unwrap());
    let offset = std::env::var("ONEKEE_BENCH_OFFSET")
        .unwrap()
        .parse()
        .unwrap();
    let (nodes, _guard) = PbfNodes::open(&source, &index, &mut |_| {}).unwrap();
    let samples: Vec<Vec<i64>> = open_planet_at(&source, offset)
        .unwrap()
        .0
        .take(4)
        .map(|b| {
            let BlobDecode::OsmData(block) = b.unwrap().decode().unwrap() else {
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
    // Repeated dependency sets model independent way groups revisiting the
    // same node blocks; this is a coordinate-stage benchmark, not a planet ETA.
    let groups: Vec<_> = (0..16).map(|n| samples[n % 4].clone()).collect();
    let mut reference = None;
    for sequential in [false, true, true, false] {
        let before = nodes.activity();
        let start = Instant::now();
        let result = if sequential {
            let temp = Temp::new();
            let mut cmd = fixture(&temp.0, 1);
            cmd.planet_path = source.clone();
            let mut join = empty(&cmd, &nodes, nodes.first_way);
            let mut sinks =
                Sinks::new(&join.root.join("requests"), join.state.requests.len()).unwrap();
            for (n, ids) in groups.iter().enumerate() {
                let mut ids = ids.clone();
                ids.sort_unstable();
                ids.dedup();
                for id in ids {
                    if let Some(block) = nodes.block_for(id) {
                        sinks
                            .push(
                                block / BLOCKS_PER_SHARD,
                                Record {
                                    id,
                                    value: n as u64,
                                },
                            )
                            .unwrap();
                    }
                }
                join.state.groups.push(Group {
                    start: nodes.first_way + n as u64,
                    end: nodes.first_way + n as u64 + 1,
                    spool: Spool::default(),
                });
            }
            sinks.finish(&mut join.state.requests).unwrap();
            join.state.collected = true;
            let scratch = join.state.requests.iter().map(Spool::len).sum::<u64>();
            join.resolve(&nodes, &mut |_| {}).unwrap();
            println!(
                "request scratch={scratch}; coordinate scratch={}",
                join.state.groups.iter().map(|g| g.spool.len()).sum::<u64>()
            );
            groups
                .iter()
                .enumerate()
                .map(|(n, ids)| {
                    let (_, table) = join.group(nodes.first_way + n as u64).unwrap().unwrap();
                    nodes.prepared_session(table).lookup_many(ids).unwrap()
                })
                .collect::<Vec<_>>()
        } else {
            // Match the currently shipped shared-per-batch preparation: each
            // batch decodes its required blocks in parallel, then the next
            // batch prepares independently.
            groups
                .iter()
                .map(|ids| {
                    let table = nodes.prepare_batch(ids.clone()).unwrap();
                    nodes.prepared_session(table).lookup_many(ids).unwrap()
                })
                .collect::<Vec<_>>()
        };
        let after = nodes.activity();
        println!(
            "sequential={sequential} seconds={:.3} blocks={} bytes={}",
            start.elapsed().as_secs_f64(),
            after.0 - before.0,
            after.1 - before.1
        );
        if let Some(expected) = &reference {
            assert_eq!(&result, expected);
        } else {
            reference = Some(result);
        }
    }
}

#[test]
fn join_replays_uncommitted_output_and_emission_resumes_at_saved_group() {
    let temp = Temp::new();
    let cmd = fixture(&temp.0, 4);
    let (nodes, _guard) = PbfNodes::open(
        &cmd.planet_path,
        &cmd.tmp_dir.join("indexed-pbf-v1"),
        &mut |_| {},
    )
    .unwrap();
    let first_way = nodes.first_way;
    let mut join = empty(&cmd, &nodes, first_way);
    join.collect(&cmd, &nodes, &mut |_| {}, 1).unwrap();
    let coordinate_dir = join.root.join("coordinates");
    fs::create_dir(&coordinate_dir).unwrap();
    fs::write(
        Sinks::path(&coordinate_dir, 0),
        b"incomplete join output before checkpoint",
    )
    .unwrap();
    let joined = Join::prepare(&cmd, &nodes, first_way, "test", &mut |_| {})
        .unwrap()
        .unwrap();
    let first_end = joined.state.groups[0].end;
    assert!(joined.group(first_way).is_ok());
    assert!(Join::prepare(&cmd, &nodes, first_way + 1, "test", &mut |_| {}).is_err());
    let lookup = Arc::new(crate::planet_lookup::Lookup::Compact(nodes));
    let mut checkpoint = super::super::Checkpoint {
        pass2_offset: first_way,
        ..Default::default()
    };
    let checkpoint_path = temp.0.join("way-checkpoint.txt");
    fs::create_dir_all(&cmd.out_dir).unwrap();
    let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        super::super::run_pass2_prepared(
            &cmd,
            &lookup,
            &mut checkpoint,
            &checkpoint_path,
            &mut |p| {
                if p.stage == "Scanning Ways"
                    && p.metrics
                        .as_ref()
                        .is_some_and(|m| m.checkpoint_bytes == first_end)
                {
                    panic!("injected stop after cell checkpoint");
                }
            },
            Some(&joined),
        )
    }));
    assert!(stopped.is_err());
    checkpoint = super::super::Checkpoint::load(&checkpoint_path);
    assert_eq!(checkpoint.pass2_offset, first_end);
    let before = lookup.activity();
    let stats = super::super::run_pass2_prepared(
        &cmd,
        &lookup,
        &mut checkpoint,
        &checkpoint_path,
        &mut |_| {},
        Some(&joined),
    )
    .unwrap();
    assert_eq!(stats.feature_count, 6);
    assert_eq!(lookup.activity(), before);
    assert_eq!(checkpoint.pass2_offset, 0);
}

#[test]
fn parallel_decode_windows_and_partitions_never_reread_blocks() {
    fn varint(mut n: u64) -> Vec<u8> {
        let mut out = Vec::new();
        while n >= 128 {
            out.push(n as u8 | 128);
            n >>= 7;
        }
        out.push(n as u8);
        out
    }
    fn field(n: u64, bytes: &[u8]) -> Vec<u8> {
        [
            varint(n * 8 + 2),
            varint(bytes.len() as u64),
            bytes.to_vec(),
        ]
        .concat()
    }
    let temp = Temp::new();
    let cmd = fixture(&temp.0, 1);
    let original = fs::read(&cmd.planet_path).unwrap();
    let mut reader = BlobReader::new_seekable(Cursor::new(&original)).unwrap();
    reader.next().unwrap().unwrap();
    let header_end = reader.seek_raw(SeekFrom::Current(0)).unwrap() as usize;
    let mut source = original[..header_end].to_vec();
    for id in 1..=270u64 {
        let node = [vec![8], varint(id * 2), vec![64, 0, 72, 0]].concat();
        let primitive = [field(1, &field(1, b"")), field(2, &field(1, &node))].concat();
        let blob = field(1, &primitive);
        let header = [field(1, b"OSMData"), vec![24], varint(blob.len() as u64)].concat();
        source.extend((header.len() as u32).to_be_bytes());
        source.extend(header);
        source.extend(blob);
    }
    fs::write(&cmd.planet_path, source).unwrap();
    let (nodes, _guard) = PbfNodes::open(
        &cmd.planet_path,
        &cmd.tmp_dir.join("indexed-pbf-v1"),
        &mut |_| {},
    )
    .unwrap();
    let mut join = empty(&cmd, &nodes, nodes.first_way);
    let mut sinks = Sinks::new(&join.root.join("requests"), join.state.requests.len()).unwrap();
    for group in 0..4 {
        join.state.groups.push(Group {
            start: group,
            end: group + 1,
            spool: Spool::default(),
        });
        for id in 1..=270 {
            sinks
                .push(
                    nodes.block_for(id).unwrap() / BLOCKS_PER_SHARD,
                    Record { id, value: group },
                )
                .unwrap();
        }
    }
    sinks.finish(&mut join.state.requests).unwrap();
    join.state.collected = true;
    let before = nodes.activity();
    join.resolve_window(&nodes, &mut |_| {}, 1000).unwrap();
    assert_eq!(nodes.activity().0 - before.0, 270);
    for group in 0..4 {
        let (_, table) = join.group(group).unwrap().unwrap();
        let ids: Vec<_> = (1..=270).collect();
        assert_eq!(
            nodes.prepared_session(table).lookup_many(&ids).unwrap(),
            vec![Some((0., 0.)); 270]
        );
    }
    let before = nodes.activity();
    join.resolve(&nodes, &mut |_| {}).unwrap();
    assert_eq!(nodes.activity(), before);
}

#[test]
fn damaged_temporary_sort_rebuilds_once_and_discards_uncommitted_output() {
    for persistent in [false, true] {
        let temp = Temp::new();
        let cmd = fixture(&temp.0, 4);
        let (nodes, _guard) = PbfNodes::open(
            &cmd.planet_path,
            &cmd.tmp_dir.join("indexed-pbf-v1"),
            &mut |_| {},
        )
        .unwrap();
        let mut join = empty(&cmd, &nodes, nodes.first_way);
        join.collect(&cmd, &nodes, &mut |_| {}, 1).unwrap();
        let root = join.root.clone();
        let mut injections = 0;
        let mut retries = 0;
        let result = join.resolve(&nodes, &mut |p| {
            if p.message.contains("Rebuilding temporary sort") {
                retries += 1;
            }
            if p.message.contains("GB sorted") && (persistent || injections == 0) {
                injections += 1;
                let path = root.join("sort/0/000000.bin");
                let mut bytes = fs::read(&path).unwrap();
                bytes[0] ^= 1;
                fs::write(path, bytes).unwrap();
                // A failure may be detected after some coordinates were appended.
                let dir = root.join("coordinates");
                fs::create_dir_all(&dir).unwrap();
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(Sinks::path(&dir, 0))
                    .unwrap()
                    .write_all(b"uncommitted tail")
                    .unwrap();
            }
        });
        assert_eq!(retries, 1);
        if persistent {
            assert_eq!(injections, 2);
            let error = result.err().unwrap();
            assert!(error.contains("failed again"));
            assert!(error.contains("same working folder"));
            let state: State =
                serde_json::from_slice(&fs::read(root.join("state.json")).unwrap()).unwrap();
            assert_eq!(state.joined, 0);
            assert!(Sinks::path(&root.join("requests"), 0).exists());
        } else {
            result.unwrap();
            assert_eq!(injections, 1);
            assert_eq!(join.state.joined, 1);
            let (_, table) = join.group(join.start()).unwrap().unwrap();
            let ids = [10, 20, 30, 40];
            assert_eq!(
                nodes.prepared_session(table).lookup_many(&ids).unwrap(),
                nodes.session().lookup_many(&ids).unwrap()
            );
        }
    }
}

#[test]
fn damaged_committed_requests_are_not_retried_as_temporary_sort_failures() {
    let temp = Temp::new();
    let cmd = fixture(&temp.0, 1);
    let (nodes, _guard) = PbfNodes::open(
        &cmd.planet_path,
        &cmd.tmp_dir.join("indexed-pbf-v1"),
        &mut |_| {},
    )
    .unwrap();
    let mut join = empty(&cmd, &nodes, nodes.first_way);
    join.collect(&cmd, &nodes, &mut |_| {}, 1).unwrap();
    let path = Sinks::path(&join.root.join("requests"), 0);
    let mut bytes = fs::read(&path).unwrap();
    bytes[0] ^= 1;
    fs::write(&path, bytes).unwrap();
    let mut retries = 0;
    let error = join
        .resolve(&nodes, &mut |p| {
            if p.message.contains("Rebuilding temporary sort") {
                retries += 1;
            }
        })
        .err()
        .unwrap();
    assert!(error.contains("checksum"));
    assert_eq!(retries, 0);
    assert_eq!(join.state.joined, 0);
    assert!(path.exists());
}

#[test]
#[ignore = "operator audit: reads committed requests, recreates sort only in /tmp"]
fn audit_operator_join_partition_sort() {
    let root = PathBuf::from(std::env::var("ONEKEE_JOIN_AUDIT_ROOT").unwrap());
    let source = PathBuf::from(std::env::var("ONEKEE_BENCH_SOURCE").unwrap());
    let index = PathBuf::from(std::env::var("ONEKEE_BENCH_INDEX").unwrap());
    let state: State = serde_json::from_slice(&fs::read(root.join("state.json")).unwrap()).unwrap();
    let (nodes, _guard) = PbfNodes::open(&source, &index, &mut |_| {}).unwrap();
    let shard = state.joined;
    let temp = Temp::new();
    let mut records = io::sorted(
        &Sinks::path(&root.join("requests"), shard),
        &state.requests[shard],
        &temp.0.join("sort"),
        SORT_RECORDS,
        &mut |_| {},
    )
    .unwrap();
    let mut count = 0u64;
    let mut last = None;
    while let Some(record) = records.next().unwrap() {
        assert_eq!(
            nodes.block_for(record.id).unwrap() / BLOCKS_PER_SHARD,
            shard
        );
        assert!(record.value < state.groups.len() as u64);
        assert!(last.is_none_or(|r| r <= record));
        last = Some(record);
        count += 1;
    }
    assert_eq!(count, state.requests[shard].len() / 16);
    assert_eq!(nodes.activity().0, 0);
    println!(
        "Partition {} regenerated and verified: {count} sorted records; original index, source, output and join files unchanged",
        shard + 1
    );
}
