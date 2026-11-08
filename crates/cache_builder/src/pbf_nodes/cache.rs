use super::{Node, PbfNodes};
use rayon::prelude::*;
use std::{collections::HashMap, sync::Arc};

const CACHE_BYTES: usize = 16 * 1024 * 1024;
const MAX_PREFETCH_REFS: usize = 512 * 1024;
type Coordinate = Option<(f32, f32)>;
/// Selected coordinates shared by all workers in one bounded way group.
pub struct Prepared(Vec<(i64, Coordinate)>);

impl PbfNodes {
    pub fn prepare_batch(&self, mut ids: Vec<i64>) -> Result<Arc<Prepared>, String> {
        ids.sort_unstable();
        ids.dedup();
        let mut groups = Vec::new();
        let mut start = 0;
        while start < ids.len() {
            let b = self.blocks.partition_point(|b| b.max < ids[start]);
            let end = if let Some(block) = self.blocks.get(b) {
                start + ids[start..].partition_point(|&id| id <= block.max)
            } else {
                ids.len()
            };
            groups.push((b, start, end));
            start = end;
        }
        // One decode per required block for the whole group, not per way or
        // per blob. Missing IDs stay in the table so they never trigger rereads.
        let coordinates = groups
            .par_iter()
            .map(|&(b, start, end)| {
                let nodes = if b < self.blocks.len() && ids[end - 1] >= self.blocks[b].min {
                    self.read_block(b)?
                } else {
                    Vec::new()
                };
                Ok(ids[start..end]
                    .iter()
                    .map(|id| {
                        nodes
                            .binary_search_by_key(id, |n| n.id)
                            .ok()
                            .map(|i| (nodes[i].lat, nodes[i].lon))
                    })
                    .collect::<Vec<_>>())
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Arc::new(Prepared(
            ids.into_iter()
                .zip(coordinates.into_iter().flatten())
                .collect(),
        )))
    }
    pub fn prepared_session(&self, prepared: Arc<Prepared>) -> Session<'_> {
        let mut session = Session::new(self);
        session.shared = Some(prepared);
        session
    }
}

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
    shared: Option<Arc<Prepared>>,
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
            shared: None,
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
        if self.shared.is_some() {
            return Ok(());
        }
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
            if let Some(shared) = &self.shared
                && let Ok(n) = shared.0.binary_search_by_key(&id, |n| n.0)
            {
                result[i] = shared.0[n].1;
                continue;
            }
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
