use super::*;
use std::fs;
use std::io::{Cursor, SeekFrom};
use std::sync::atomic::{AtomicU64, Ordering};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "1kee-pbf-nodes-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn source(&self, bytes: &[u8]) -> PathBuf {
        let p = self.0.join("source.pbf");
        fs::write(&p, bytes).unwrap();
        p
    }
    fn index(&self) -> PathBuf {
        self.0.join("compact")
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn varint(mut n: u64) -> Vec<u8> {
    let mut b = Vec::new();
    while n >= 128 {
        b.push(n as u8 | 128);
        n >>= 7;
    }
    b.push(n as u8);
    b
}
fn bytes_field(field: u64, bytes: &[u8]) -> Vec<u8> {
    [
        varint(field * 8 + 2),
        varint(bytes.len() as u64),
        bytes.to_vec(),
    ]
    .concat()
}
fn header() -> Vec<u8> {
    let fixture = include_bytes!("../../tests/fixtures/planet-tiny-plain.osm.pbf");
    let mut reader = BlobReader::new_seekable(Cursor::new(fixture)).unwrap();
    reader.next().unwrap().unwrap();
    let n = reader.seek_raw(SeekFrom::Current(0)).unwrap() as usize;
    fixture[..n].to_vec()
}
fn node_block(id: i64) -> Vec<u8> {
    // Ordinary node with zero coordinates, in a raw uncompressed PBF blob.
    let node = [vec![8], varint((id << 1) as u64), vec![64, 0, 72, 0]].concat();
    let group = bytes_field(1, &node);
    let primitive = [bytes_field(1, &bytes_field(1, b"")), bytes_field(2, &group)].concat();
    let blob = bytes_field(1, &primitive);
    let blob_header = [
        bytes_field(1, b"OSMData"),
        vec![24],
        varint(blob.len() as u64),
    ]
    .concat();
    [
        (blob_header.len() as u32).to_be_bytes().to_vec(),
        blob_header,
        blob,
    ]
    .concat()
}

#[test]
fn plain_and_dense_lookups_match_flat_and_cache_shared_references() {
    for fixture in [
        include_bytes!("../../tests/fixtures/planet-tiny-plain.osm.pbf").as_slice(),
        include_bytes!("../../tests/fixtures/planet-tiny-dense.osm.pbf").as_slice(),
    ] {
        let temp = Scratch::new();
        let source = temp.source(fixture);
        let (lookup, state) = PbfNodes::open(&source, &temp.index(), &mut |_| {}).unwrap();
        assert_eq!(lookup.node_count, 4);
        let flat_path = temp.0.join("flat.bin");
        let mut writer = crate::flat_node_store::NodeWriter::create(&flat_path).unwrap();
        for blob in BlobReader::new(fixture) {
            for n in decode(&blob.unwrap()).unwrap().0 {
                writer.write(n.id, n.lat, n.lon).unwrap();
            }
        }
        writer.finish().unwrap();
        let flat = crate::flat_node_store::NodeLookup::open(&flat_path, 4).unwrap();
        let ids = [40, 10, 20, 20, 30, -1, 15, 99];
        let expected = flat.session().lookup_many(&ids).unwrap();
        let mut session = lookup.session();
        assert_eq!(session.lookup_many(&ids).unwrap(), expected);
        let decodes = session.decoded_blocks;
        assert_eq!(session.lookup_many(&ids).unwrap(), expected);
        assert_eq!(session.decoded_blocks, decodes);
        assert!(PbfNodes::open(&source, &temp.index(), &mut |_| {}).is_err()); // build lock
        drop(session);
        drop(lookup);
        drop(state);
        let (reused, mut state) = PbfNodes::open(&source, &temp.index(), &mut |_| {
            panic!("must reuse completed index")
        })
        .unwrap();
        assert_eq!(reused.session().lookup_many(&ids).unwrap(), expected);
        assert!(!state.bind_build("settings-a".into()).unwrap());
        state.finish().unwrap();
        assert!(state.bind_build("settings-a".into()).unwrap());
        assert!(state.bind_build("settings-b".into()).is_err());
    }
}

#[test]
fn evicted_blocks_are_reread_and_failed_reads_never_become_missing_nodes() {
    let temp = Scratch::new();
    let bytes = [header(), node_block(10), node_block(20), node_block(30)].concat();
    let source = temp.source(&bytes);
    let (lookup, _state) = PbfNodes::open(&source, &temp.index(), &mut |_| {}).unwrap();
    let mut session = lookup.session();
    session.set_budget(64);
    for id in [10, 20, 10] {
        assert_eq!(session.lookup_many(&[id]).unwrap(), vec![Some((0.0, 0.0))]);
    }
    assert_eq!(session.decoded_blocks, 3);
    fs::write(&source, &bytes[..5]).unwrap();
    assert!(lookup.validate_source().is_err());
    assert!(session.lookup_many(&[30]).is_err());
    assert_eq!(session.decoded_blocks, 3);
}

#[test]
fn grouped_way_dependencies_avoid_redecoding_evicted_blocks() {
    let temp = Scratch::new();
    let bytes = [header(), node_block(10), node_block(20), node_block(30)].concat();
    let source = temp.source(&bytes);
    let (lookup, _state) = PbfNodes::open(&source, &temp.index(), &mut |_| {}).unwrap();
    let mut session = lookup.session();
    session.set_budget(64);
    session.prefetch([10, 20, 30, 10, 99].into_iter()).unwrap();
    assert_eq!(session.decoded_blocks, 3);
    for id in [30, 10, 20, 10] {
        assert_eq!(
            session.lookup_many(&[id, 99]).unwrap(),
            vec![Some((0.0, 0.0)), None]
        );
    }
    assert_eq!(session.decoded_blocks, 3);
    assert_eq!(session.lookup_many(&[15]).unwrap(), vec![None]);
    session.prefetch([20].into_iter()).unwrap();
    // A new blob discards the old dependency table; unprepared IDs still resolve.
    assert_eq!(session.lookup_many(&[10]).unwrap(), vec![Some((0.0, 0.0))]);
    assert_eq!(session.decoded_blocks, 5);
    session
        .prefetch(std::iter::repeat_n(20, 512 * 1024 + 1))
        .unwrap();
    assert_eq!(session.lookup_many(&[20]).unwrap(), vec![Some((0.0, 0.0))]);
    assert_eq!(session.decoded_blocks, 6);
}

#[test]
fn interrupted_index_resumes_durably_and_rejects_changed_or_unordered_sources() {
    let temp = Scratch::new();
    let mut bytes = header();
    for id in 1..=40 {
        bytes.extend(node_block(id));
    }
    let source = temp.source(&bytes);
    let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = PbfNodes::open(&source, &temp.index(), &mut |_| {
            panic!("simulated interruption after durable batch")
        });
    }));
    assert!(interrupted.is_err());
    let (lookup, state) = PbfNodes::open(&source, &temp.index(), &mut |_| {}).unwrap();
    assert_eq!(lookup.node_count, 40);
    assert_eq!(lookup.block_count(), 40);
    assert_eq!(
        lookup.session().lookup_many(&[1, 32, 40, 41]).unwrap(),
        vec![Some((0.0, 0.0)), Some((0.0, 0.0)), Some((0.0, 0.0)), None]
    );
    drop(lookup);
    drop(state);
    let db = fs::read(temp.index().join("nodes.sqlite")).unwrap();
    fs::write(&source, [bytes, vec![0]].concat()).unwrap();
    assert!(PbfNodes::open(&source, &temp.index(), &mut |_| {}).is_err());
    assert_eq!(fs::read(temp.index().join("nodes.sqlite")).unwrap(), db);
    for ids in [[20, 10], [10, 10]] {
        let t = Scratch::new();
        let source = t.source(&[header(), node_block(ids[0]), node_block(ids[1])].concat());
        let error = match PbfNodes::open(&source, &t.index(), &mut |_| {}) {
            Ok(_) => panic!("accepted unordered nodes"),
            Err(e) => e,
        };
        assert!(error.contains("increasing node IDs"), "{error}");
    }
}

#[test]
fn corrupt_descriptor_cannot_silently_hide_nodes() {
    let temp = Scratch::new();
    let source = temp.source(&[header(), node_block(10), node_block(20)].concat());
    let (lookup, state) = PbfNodes::open(&source, &temp.index(), &mut |_| {}).unwrap();
    drop(lookup);
    drop(state);
    let db = rusqlite::Connection::open(temp.index().join("nodes.sqlite")).unwrap();
    db.execute(
        "UPDATE blocks SET length=length+1000000 WHERE min_id=10",
        [],
    )
    .unwrap();
    drop(db);
    let error = match PbfNodes::open(&source, &temp.index(), &mut |_| {}) {
        Ok(_) => panic!("accepted invalid source span"),
        Err(e) => e,
    };
    assert!(error.contains("Invalid compact PBF block index"), "{error}");
}
