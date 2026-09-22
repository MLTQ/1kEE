//! Bounded per-worker reuse of immutable node-store blocks across OSM ways.
use super::{INDEX_STRIDE, NodeLookup};

const BLOCKS: usize = 64; // At most 4 MiB of node bytes per active Rayon task.

pub struct LookupSession<'a> {
    lookup: &'a NodeLookup,
    blocks: Vec<Option<(u64, Vec<u8>)>>,
    queries: Vec<(u64, u64, usize)>,
    #[cfg(test)]
    reads: usize,
}

impl<'a> LookupSession<'a> {
    pub(super) fn new(lookup: &'a NodeLookup) -> Self {
        Self {
            lookup,
            blocks: (0..BLOCKS).map(|_| None).collect(),
            queries: Vec::new(),
            #[cfg(test)]
            reads: 0,
        }
    }

    pub fn lookup_many(&mut self, ids: &[i64]) -> Result<Vec<Option<(f32, f32)>>, String> {
        let mut results = vec![None; ids.len()];
        self.queries.clear();
        for (index, &id) in ids.iter().enumerate() {
            if let Some((start, end)) = self.lookup.block_bounds(id) {
                self.queries.push((start, end, index));
            }
        }
        self.queries.sort_unstable_by_key(|&(start, _, _)| start);
        let mut first = 0;
        while first < self.queries.len() {
            let (start, end, _) = self.queries[first];
            let mut last = first + 1;
            while last < self.queries.len() && self.queries[last].0 == start {
                last += 1;
            }
            // Direct mapping bounds memory and avoids shared cache locks or
            // an LRU scan. Collisions replace a block; correctness is unchanged.
            let slot = (start / INDEX_STRIDE) as usize % BLOCKS;
            if self.blocks[slot]
                .as_ref()
                .is_none_or(|(key, _)| *key != start)
            {
                let mut bytes = self.blocks[slot].take().map(|(_, b)| b).unwrap_or_default();
                #[cfg(test)]
                {
                    self.reads += 1;
                }
                self.lookup.read_block(start, end, &mut bytes)?;
                self.blocks[slot] = Some((start, bytes));
            }
            if let Some((_, bytes)) = &self.blocks[slot] {
                for &(_, _, index) in &self.queries[first..last] {
                    results[index] = NodeLookup::search_block(bytes, ids[index]);
                }
            }
            first = last;
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flat_node_store::NodeWriter;

    #[test]
    #[ignore = "read-only real planet/node-store benchmark; requires ONEKEE_PLANET_BENCH_PBF, NODES, and OFFSET"]
    fn benchmark_real_planet_node_lookups() {
        use std::{path::PathBuf, time::Instant};
        let source = PathBuf::from(std::env::var_os("ONEKEE_PLANET_BENCH_PBF").expect("PBF path"));
        let nodes =
            PathBuf::from(std::env::var_os("ONEKEE_PLANET_BENCH_NODES").expect("node-store path"));
        let offset = std::env::var("ONEKEE_PLANET_BENCH_OFFSET")
            .expect("blob offset")
            .parse()
            .unwrap();
        let (reader, _) = crate::roads::open_planet_at(&source, offset).unwrap();
        let mut queries = Vec::new();
        for blob in reader.take(64) {
            if let osmpbf::BlobDecode::OsmData(block) = blob.unwrap().decode().unwrap() {
                for element in block.elements() {
                    if let osmpbf::Element::Way(way) = element {
                        queries.push(way.refs().collect::<Vec<_>>());
                        if queries.len() == 10_000 {
                            break;
                        }
                    }
                }
            }
            if queries.len() == 10_000 {
                break;
            }
        }
        assert!(!queries.is_empty(), "benchmark offset must include ways");
        let scratch = std::env::temp_dir().join(format!("1kee-index-bench-{}", std::process::id()));
        std::fs::create_dir(&scratch).unwrap();
        let index_path = scratch.join("nodes.index");
        let count = std::fs::metadata(&nodes).unwrap().len() / 16;
        let start = Instant::now();
        let lookup = NodeLookup::open_cached(&nodes, count, &index_path).unwrap();
        eprintln!(
            "PLANET_INDEX seconds={:.3} ways={} refs={}",
            start.elapsed().as_secs_f64(),
            queries.len(),
            queries.iter().map(Vec::len).sum::<usize>()
        );
        let start = Instant::now();
        let reused = NodeLookup::open_cached(&nodes, count, &index_path).unwrap();
        eprintln!(
            "PLANET_INDEX_REUSED seconds={:.3} bytes={}",
            start.elapsed().as_secs_f64(),
            std::fs::metadata(&index_path).unwrap().len()
        );
        assert_eq!(lookup.index, reused.index);
        drop(reused);
        let mut reference = None;
        for cached in [false, true, false, true] {
            let start = Instant::now();
            let mut session = lookup.session();
            let result: Vec<_> = queries
                .iter()
                .map(|ids| {
                    if cached {
                        session.lookup_many(ids).unwrap()
                    } else {
                        lookup.lookup_many(ids)
                    }
                })
                .collect();
            let seconds = start.elapsed().as_secs_f64();
            if let Some(expected) = &reference {
                assert_eq!(&result, expected);
            } else {
                reference = Some(result);
            }
            eprintln!(
                "PLANET_LOOKUP cached={cached} seconds={seconds:.3} cached_block_reads={}",
                session.reads
            );
        }
        std::fs::remove_dir_all(scratch).unwrap();
    }
    #[test]
    fn repeated_ways_reuse_blocks_and_collisions_preserve_exact_coordinates() {
        let root = std::env::temp_dir().join(format!(
            "1kee-node-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("nodes.bin");
        let count = (BLOCKS as u64 + 1) * INDEX_STRIDE + 3;
        let mut writer = NodeWriter::create(&path).unwrap();
        for n in 0..count {
            writer
                .write(n as i64 * 2, n as f32 * 0.125, -(n as f32))
                .unwrap();
        }
        writer.finish().unwrap();
        let lookup = NodeLookup::open(&path, count).unwrap();
        let mut session = lookup.session();
        let ids = [10, 3, 2, 10, INDEX_STRIDE as i64 * 2, -2];
        let expected = lookup.lookup_many(&ids);
        for _ in 0..5 {
            assert_eq!(session.lookup_many(&ids).unwrap(), expected);
        }
        assert_eq!(session.reads, 2);
        let collision = [BLOCKS as i64 * INDEX_STRIDE as i64 * 2, 2, count as i64 * 2];
        for _ in 0..3 {
            assert_eq!(
                session.lookup_many(&collision).unwrap(),
                lookup.lookup_many(&collision)
            );
        }
        assert!(
            session
                .blocks
                .iter()
                .flatten()
                .map(|(_, b)| b.len())
                .sum::<usize>()
                <= BLOCKS * INDEX_STRIDE as usize * 16
        );
        // A failing block must abort, not be cached as missing coordinates.
        let original = std::fs::read(&path).unwrap();
        let mut session = lookup.session();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(8)
            .unwrap();
        assert!(session.lookup_many(&ids).is_err());
        std::fs::write(&path, original).unwrap();
        assert_eq!(session.lookup_many(&ids).unwrap(), expected);
        drop(lookup);
        std::fs::remove_dir_all(root).unwrap();
    }
}
