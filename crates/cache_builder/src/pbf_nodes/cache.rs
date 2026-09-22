use super::{Node, PbfNodes};
use std::collections::HashMap;

const CACHE_BYTES: usize = 16 * 1024 * 1024;
const MAX_PREFETCH_REFS: usize = 512 * 1024;
type Coordinate = Option<(f32, f32)>;
struct Entry {
    nodes: Vec<Node>,
    used: u64,
}

pub struct Session<'a> {
    source: &'a PbfNodes,
    cache: HashMap<usize, Entry>,
    bytes: usize,
    tick: u64,
    budget: usize,
    prefetched: Vec<(i64, Coordinate)>,
    pub decoded_blocks: u64,
}

impl<'a> Session<'a> {
    pub(super) fn new(source: &'a PbfNodes) -> Self {
        Self {
            source,
            cache: HashMap::new(),
            bytes: 0,
            tick: 0,
            budget: CACHE_BYTES,
            prefetched: Vec::new(),
            decoded_blocks: 0,
        }
    }

    #[cfg(test)]
    pub(super) fn set_budget(&mut self, bytes: usize) {
        assert!(self.cache.is_empty());
        self.budget = bytes;
    }

    /// Resolve a way blob together, so even evicted blocks are decoded at most
    /// once for the blob. Extremely large dependency lists fall back to LRU only.
    pub fn prefetch(&mut self, refs: impl Iterator<Item = i64>) -> Result<(), String> {
        self.prefetched = Vec::new();
        let mut ids: Vec<_> = refs.take(MAX_PREFETCH_REFS + 1).collect();
        if ids.len() > MAX_PREFETCH_REFS {
            return Ok(());
        }
        ids.sort_unstable();
        ids.dedup();
        let coordinates = self.lookup_many(&ids)?;
        self.prefetched = ids.into_iter().zip(coordinates).collect();
        Ok(())
    }

    pub fn lookup_many(&mut self, ids: &[i64]) -> Result<Vec<Option<(f32, f32)>>, String> {
        let mut result = vec![None; ids.len()];
        let mut groups = Vec::with_capacity(ids.len());
        for (i, &id) in ids.iter().enumerate() {
            if let Ok(n) = self.prefetched.binary_search_by_key(&id, |n| n.0) {
                result[i] = self.prefetched[n].1;
                continue;
            }
            let b = self.source.blocks.partition_point(|b| b.max < id);
            if self.source.blocks.get(b).is_some_and(|b| b.min <= id) {
                groups.push((b, i));
            }
        }
        groups.sort_unstable_by_key(|&(b, _)| b);
        let mut start = 0;
        while start < groups.len() {
            let b = groups[start].0;
            let end = start + groups[start..].partition_point(|&(other, _)| other == b);
            self.tick += 1;
            let mut uncached = None;
            if !self.cache.contains_key(&b) {
                let nodes = self.source.read_block(b)?;
                self.decoded_blocks += 1;
                let size = nodes.capacity() * std::mem::size_of::<Node>();
                if size <= self.budget {
                    while self.bytes + size > self.budget {
                        let oldest = *self.cache.iter().min_by_key(|(_, e)| e.used).unwrap().0;
                        let old = self.cache.remove(&oldest).unwrap();
                        self.bytes -= old.nodes.capacity() * std::mem::size_of::<Node>();
                    }
                    self.bytes += size;
                    self.cache.insert(
                        b,
                        Entry {
                            nodes,
                            used: self.tick,
                        },
                    );
                } else {
                    uncached = Some(nodes);
                }
            }
            let nodes = if let Some(entry) = self.cache.get_mut(&b) {
                entry.used = self.tick;
                &entry.nodes
            } else {
                uncached.as_ref().unwrap()
            };
            for &(_, i) in &groups[start..end] {
                if let Ok(n) = nodes.binary_search_by_key(&ids[i], |n| n.id) {
                    result[i] = Some((nodes[n].lat, nodes[n].lon));
                }
            }
            start = end;
        }
        Ok(result)
    }
}
