//! External node/way join. Original node blocks are decoded at most once per
//! successful join, independent of the number of downstream way batches.
use super::selection;
use crate::{
    args::PlanetAllCommand,
    build_monitor::Metrics,
    pbf_nodes::{Node, PbfNodes, Prepared},
    roads::{RoadBuildProgress, open_planet_at},
};
use osmpbf::BlobDecode;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};
#[path = "planet_join_io.rs"]
mod io;
use io::{Record, Records, Result, Sinks, Spool};
const BLOCKS_PER_SHARD: usize = 128;
const MAX_SHARDS: usize = 512;
const GROUP_BYTES: u64 = 32 * 1024 * 1024;
const GROUP_REFS: usize = 8 * 1024 * 1024;
const SORT_RECORDS: usize = 4 * 1024 * 1024;
fn default_blocks_per_shard() -> usize {
    BLOCKS_PER_SHARD
}
fn selected_refs(blob: &osmpbf::Blob, cmd: &PlanetAllCommand) -> Result<Vec<i64>> {
    let BlobDecode::OsmData(block) = blob.decode().map_err(|e| e.to_string())? else {
        return Ok(Vec::new());
    };
    Ok(block
        .groups()
        .flat_map(|g| g.ways())
        .filter(|w| selection::classify(w, cmd).is_some())
        .flat_map(|w| w.refs())
        .collect())
}
#[derive(Serialize, Deserialize)]
struct Group {
    start: u64,
    end: u64,
    spool: Spool,
}
#[derive(Serialize, Deserialize)]
struct State {
    version: u32,
    identity: String,
    options: String,
    start: u64,
    scan: u64,
    collected: bool,
    joined: usize,
    #[serde(default = "default_blocks_per_shard")]
    blocks_per_shard: usize,
    requests: Vec<Spool>,
    groups: Vec<Group>,
}
pub(super) struct Join {
    root: PathBuf,
    state: State,
}
fn save(root: &Path, state: &State) -> Result<()> {
    let tmp = root.join("state.tmp");
    let mut file = File::create(&tmp).map_err(|e| e.to_string())?;
    file.write_all(&serde_json::to_vec(state).map_err(|e| e.to_string())?)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    fs::rename(tmp, root.join("state.json")).map_err(|e| e.to_string())?;
    File::open(root)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}
fn report(
    progress: &mut dyn FnMut(RoadBuildProgress),
    nodes: &PbfNodes,
    phase: &str,
    done: u64,
    total: u64,
    message: String,
) {
    let (blocks, bytes) = nodes.activity();
    progress(RoadBuildProgress {
        stage: phase.into(),
        message,
        fraction: 0.4,
        metrics: Some(Metrics {
            phase: phase.into(),
            scanned_bytes: done,
            total_bytes: total,
            checkpoint_bytes: done,
            nodes: nodes.node_count,
            lookup_blocks: blocks,
            lookup_bytes: bytes,
            ..Default::default()
        }),
    });
}
fn require_space(root: &Path, extra: u64) -> Result<()> {
    if let Some(free) = crate::archive_space::available(root) {
        let required = extra.saturating_add(1024 * 1024 * 1024);
        if free < required {
            return Err(format!(
                "Coordinate join needs {:.2} GB additional scratch headroom; only {:.2} GB free. Free space and resume with the same working folder; checkpoints preserved.",
                required as f64 / 1e9,
                free as f64 / 1e9
            ));
        }
    }
    Ok(())
}

impl Join {
    pub fn prepare(
        cmd: &PlanetAllCommand,
        nodes: &PbfNodes,
        start: u64,
        options: &str,
        progress: &mut dyn FnMut(RoadBuildProgress),
    ) -> Result<Option<Self>> {
        if !selection::has_way_layers(cmd) {
            return Ok(None);
        }
        nodes.validate_source()?;
        let root = cmd.tmp_dir.join("indexed-pbf-v1/way-join-v1");
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let state = if root.join("state.json").exists() {
            let state: State = serde_json::from_slice(
                &fs::read(root.join("state.json")).map_err(|e| e.to_string())?,
            )
            .map_err(|e| format!("Invalid way join checkpoint: {e}"))?;
            let expected_count = if state.blocks_per_shard == 0 {
                0
            } else {
                nodes.block_count().div_ceil(state.blocks_per_shard)
            };
            if state.version != 1
                || state.identity != nodes.join_identity()
                || state.options != options
                || state.blocks_per_shard == 0
                || state.requests.len() != expected_count
                || state.joined > expected_count
            {
                return Err(
                    "Way join source/settings mismatch; existing checkpoints preserved".into(),
                );
            }
            state
        } else {
            // Cap shard count at MAX_SHARDS so planet builds avoid thousands of
            // spool files and giant state.json rewrites.
            let blocks_per_shard = nodes
                .block_count()
                .div_ceil(MAX_SHARDS)
                .max(BLOCKS_PER_SHARD);
            let count = nodes.block_count().div_ceil(blocks_per_shard);
            // The outer compact-build lock excludes concurrent writers.
            let state = State {
                version: 1,
                identity: nodes.join_identity(),
                options: options.into(),
                start,
                scan: start,
                collected: false,
                joined: 0,
                blocks_per_shard,
                requests: vec![Spool::default(); count],
                groups: Vec::new(),
            };
            save(&root, &state)?;
            state
        };
        let mut join = Self { root, state };
        join.validate(
            start,
            nodes.first_way,
            fs::metadata(&cmd.planet_path)
                .map_err(|e| e.to_string())?
                .len(),
        )?;
        // Discard only bytes beyond the last durable stage checkpoint.
        for (n, spool) in join
            .state
            .requests
            .iter()
            .enumerate()
            .skip(join.state.joined)
        {
            spool.restore(&Sinks::path(&join.root.join("requests"), n))?;
        }
        for (n, group) in join.state.groups.iter().enumerate() {
            group
                .spool
                .restore(&Sinks::path(&join.root.join("coordinates"), n))?;
        }
        io::flush_fs_before_writes(&join.root)?;
        join.collect(cmd, nodes, progress, GROUP_BYTES)?;
        join.resolve(nodes, progress)?;
        nodes.validate_source()?;
        Ok(Some(join))
    }
    fn validate(&self, resume: u64, first_way: u64, length: u64) -> Result<()> {
        let s = &self.state;
        let mut end = s.start;
        for group in &s.groups {
            if group.start != end || group.end <= end || group.end > length {
                return Err("Invalid way join group boundaries".into());
            }
            end = group.end;
        }
        if s.start < first_way
            || s.scan != end
            || s.scan > length
            || (s.collected && s.scan != length)
            || (!s.collected && s.joined != 0)
            || !(resume == first_way
                || resume == s.start
                || s.groups.iter().any(|g| g.end == resume))
        {
            return Err(
                "Way checkpoint does not match the saved coordinate join; previous work preserved"
                    .into(),
            );
        }
        Ok(())
    }
    fn collect(
        &mut self,
        cmd: &PlanetAllCommand,
        nodes: &PbfNodes,
        progress: &mut dyn FnMut(RoadBuildProgress),
        group_bytes: u64,
    ) -> Result<()> {
        if self.state.collected {
            return Ok(());
        }
        let length = fs::metadata(&cmd.planet_path)
            .map_err(|e| e.to_string())?
            .len();
        let (mut reader, pos) = open_planet_at(&cmd.planet_path, self.state.scan)?;
        let bps = self.state.blocks_per_shard;
        loop {
            nodes.validate_source()?;
            let start = self.state.scan;
            let mut ids = Vec::new();
            let mut eof = false;
            while pos.load(Ordering::Relaxed).saturating_sub(start) < group_bytes
                && ids.len() < GROUP_REFS
            {
                let mut batch = Vec::with_capacity(32);
                while batch.len() < 32
                    && pos.load(Ordering::Relaxed).saturating_sub(start) < group_bytes
                {
                    match reader.next() {
                        Some(blob) => batch.push(blob.map_err(|e| e.to_string())?),
                        None => {
                            eof = true;
                            break;
                        }
                    }
                }
                if batch.is_empty() {
                    break;
                }
                let batch_refs: Vec<Vec<i64>> = batch
                    .par_iter()
                    .map(|blob| selected_refs(blob, cmd))
                    .collect::<Result<Vec<_>>>()?;
                let added: usize = batch_refs.iter().map(Vec::len).sum();
                ids.reserve(added);
                for r in batch_refs {
                    ids.extend(r);
                }
                if eof {
                    break;
                }
            }
            let end = pos.load(Ordering::Relaxed);
            if end > start {
                ids.par_sort_unstable();
                ids.dedup();
                let refs = ids.len();
                require_space(&self.root, (refs as u64).saturating_mul(16))?;
                let group = self.state.groups.len();
                let mut sinks = Sinks::new(&self.root.join("requests"), self.state.requests.len())?;
                let mut block_cursor = 0usize;
                for id in ids {
                    if let Some(block) = nodes.block_for_sorted(id, &mut block_cursor) {
                        sinks.push(
                            block / bps,
                            Record {
                                id,
                                value: group as u64,
                            },
                        )?;
                    }
                }
                sinks.finish(&mut self.state.requests)?;
                self.state.groups.push(Group {
                    start,
                    end,
                    spool: Spool::default(),
                });
                self.state.scan = end;
                nodes.validate_source()?;
                save(&self.root, &self.state)?;
                let scratch: u64 = self.state.requests.iter().map(Spool::len).sum();
                report(
                    progress,
                    nodes,
                    "Collect way dependencies",
                    end,
                    length,
                    format!(
                        "{} groups planned; {refs} unique references in this group; {:.2} GB request scratch",
                        group + 1,
                        scratch as f64 / 1e9
                    ),
                );
            }
            if eof {
                if end != length {
                    return Err("Way dependency scan ended before source EOF".into());
                }
                self.state.collected = true;
                nodes.validate_source()?;
                save(&self.root, &self.state)?;
                break;
            }
        }
        Ok(())
    }
    fn resolve(
        &mut self,
        nodes: &PbfNodes,
        progress: &mut dyn FnMut(RoadBuildProgress),
    ) -> Result<()> {
        self.resolve_window(nodes, progress, 1024 * 1024)
    }
    fn resolve_window(
        &mut self,
        nodes: &PbfNodes,
        progress: &mut dyn FnMut(RoadBuildProgress),
        window: usize,
    ) -> Result<()> {
        let count = self.state.requests.len();
        let bps = self.state.blocks_per_shard;
        let total = nodes.block_scan_position(nodes.block_count());
        for shard in self.state.joined..count {
            let spools = match self.resolve_partition(nodes, progress, window, shard) {
                Ok(spools) => spools,
                Err(error) if io::is_sort_error(&error) => {
                    report(
                        progress,
                        nodes,
                        "Join coordinates",
                        nodes.block_scan_position(shard * bps),
                        total,
                        format!(
                            "Rebuilding temporary sort for partition {}/{} (retry 1/1): {error}. Completed partitions are preserved.",
                            shard + 1,
                            count
                        ),
                    );
                    // A failed merge can have appended coordinates before it
                    // detects corruption. Restore all files to the last commit
                    // before rebuilding this partition from verified requests.
                    for (n, group) in self.state.groups.iter().enumerate() {
                        group
                            .spool
                            .restore(&Sinks::path(&self.root.join("coordinates"), n))?;
                    }
                    self.resolve_partition(nodes, progress, window, shard).map_err(|e| {
                        format!("Coordinate partition {} failed again after rebuilding temporary sort: {e}. Completed partitions and request files are preserved; resume using the same working folder.", shard + 1)
                    })?
                }
                Err(error) => return Err(error),
            };
            for (group, spool) in self.state.groups.iter_mut().zip(spools) {
                group.spool = spool;
            }
            // Drop finished request segment metadata so state.json shrinks as shards finish.
            self.state.requests[shard].segments.clear();
            nodes.validate_source()?;
            self.state.joined = shard + 1;
            save(&self.root, &self.state)?;
            // Commit first; interrupted cleanup is harmless and retried below.
            // Keep the `sort` directory itself alive across shards (only removing
            // flat run files inside it) so we never `rmdir` directory clusters
            // while appending coordinate spools.
            let sort = self.root.join("sort");
            let request_path = Sinks::path(&self.root.join("requests"), shard);
            if sort.exists() {
                io::clear_work_dir(&sort)?;
            }
            if request_path.exists() {
                fs::remove_file(request_path).map_err(|e| e.to_string())?;
            }
        }
        // Include cleanup left over from a crash immediately after commit.
        for n in 0..self.state.joined {
            let p = Sinks::path(&self.root.join("requests"), n);
            if p.exists() {
                fs::remove_file(p).map_err(|e| e.to_string())?;
            }
        }
        let sort = self.root.join("sort");
        if sort.exists() {
            fs::remove_dir_all(&sort).map_err(|e| e.to_string())?;
        }
        report(
            progress,
            nodes,
            "Coordinates ready",
            total,
            total,
            format!(
                "{} way groups prepared; {:.2} GB coordinate scratch. Building cells without node-PBF lookups.",
                self.state.groups.len(),
                self.state.groups.iter().map(|g| g.spool.len()).sum::<u64>() as f64 / 1e9
            ),
        );
        Ok(())
    }
    fn resolve_partition(
        &self,
        nodes: &PbfNodes,
        progress: &mut dyn FnMut(RoadBuildProgress),
        window: usize,
        shard: usize,
    ) -> Result<Vec<Spool>> {
        let count = self.state.requests.len();
        let bps = self.state.blocks_per_shard;
        let max_decode_blocks = (rayon::current_num_threads() * 2).max(16);
        let total = nodes.block_scan_position(nodes.block_count());
        require_space(
            &self.root,
            self.state.requests[shard].len().saturating_mul(3),
        )?;
        let done = nodes.block_scan_position(shard * bps);
        nodes.validate_source()?;
        report(
            progress,
            nodes,
            "Join coordinates",
            done,
            total,
            format!(
                "Sorting coordinate partition {}/{}; original node blocks are read once",
                shard + 1,
                count
            ),
        );
        let request_path = Sinks::path(&self.root.join("requests"), shard);
        let sort = self.root.join("sort");
        let mut input = io::sorted(
            &request_path,
            &self.state.requests[shard],
            &sort,
            SORT_RECORDS,
            &mut |bytes| {
                report(
                    progress,
                    nodes,
                    "Join coordinates",
                    done,
                    total,
                    format!(
                        "Sorting partition {}/{}: {:.2} GB sorted",
                        shard + 1,
                        count,
                        bytes as f64 / 1e9
                    ),
                );
            },
        )?;
        let mut sinks = Sinks::new(&self.root.join("coordinates"), self.state.groups.len())?;
        let mut carry: Option<(usize, Vec<crate::pbf_nodes::Node>)> = None;
        let mut pending = None;
        let mut previous = None;
        let mut last_report = Instant::now();
        let mut joined = 0u64;
        let mut block_lookup_cursor = shard * bps;
        loop {
            let mut requests = Vec::new();
            let mut blocks = Vec::new();
            while requests.len() < window {
                let next = if let Some(request) = pending.take() {
                    Some(request)
                } else {
                    input.next()?
                };
                let Some(request) = next else {
                    break;
                };
                if previous == Some(request) {
                    continue;
                }
                let block = nodes
                    .block_for_sorted(request.id, &mut block_lookup_cursor)
                    .or_else(|| nodes.block_for(request.id))
                    .ok_or_else(|| {
                        io::sort_error(format!(
                            "node ID {} in partition {} has no indexed block",
                            request.id,
                            shard + 1
                        ))
                    })?;
                if request.value >= self.state.groups.len() as u64 {
                    return Err(io::sort_error(format!(
                        "way group {} in partition {} is out of range",
                        request.value,
                        shard + 1
                    )));
                }
                if block / bps != shard {
                    return Err(io::sort_error(format!(
                        "node ID {} belongs to partition {}, expected {}",
                        request.id,
                        block / bps + 1,
                        shard + 1
                    )));
                }
                if blocks.last() != Some(&block) {
                    if blocks.len() == max_decode_blocks {
                        pending = Some(request);
                        break;
                    }
                    blocks.push(block);
                }
                previous = Some(request);
                requests.push((block, request));
            }
            if requests.is_empty() {
                break;
            }
            // Decode a bounded window in parallel; retain its last block if
            // another window continues that block's requests. Never reread it.
            let cached = carry.take();
            let mut decoded = Vec::new();
            let mut first = 0;
            if let Some((block, values)) = cached
                && blocks.first() == Some(&block)
            {
                decoded.push((block, values));
                first = 1;
            }
            decoded.extend(
                blocks[first..]
                    .par_iter()
                    .map(|&b| nodes.read_block(b).map(|values| (b, values)))
                    .collect::<Result<Vec<_>>>()?,
            );
            let mut block_cursor = 0;
            let mut node_cursor = 0;
            for (block, request) in requests {
                if decoded[block_cursor].0 != block {
                    block_cursor += 1;
                    node_cursor = 0;
                }
                let values = &decoded[block_cursor].1;
                while node_cursor < values.len() && values[node_cursor].id < request.id {
                    node_cursor += 1;
                }
                if let Some(node) = values
                    .get(node_cursor)
                    .filter(|n| n.id == request.id)
                    .copied()
                {
                    let group = usize::try_from(request.value).map_err(|e| e.to_string())?;
                    sinks.push(
                        group,
                        Record {
                            id: node.id,
                            value: node.lat.to_bits() as u64 | ((node.lon.to_bits() as u64) << 32),
                        },
                    )?;
                    joined += 1;
                }
            }
            carry = decoded.pop();
            if last_report.elapsed() >= Duration::from_secs(1) {
                report(
                    progress,
                    nodes,
                    "Join coordinates",
                    done,
                    total,
                    format!(
                        "Partition {}/{}: {joined} coordinates joined; {} node blocks decoded",
                        shard + 1,
                        count,
                        nodes.activity().0
                    ),
                );
                last_report = Instant::now();
            }
        }
        let mut spools: Vec<_> = self.state.groups.iter().map(|g| g.spool.clone()).collect();
        sinks.finish(&mut spools)?;
        Ok(spools)
    }
    pub fn start(&self) -> u64 {
        self.state.start
    }
    pub fn group(&self, start: u64) -> Result<Option<(u64, Arc<Prepared>)>> {
        let Some((n, group)) = self
            .state
            .groups
            .iter()
            .enumerate()
            .find(|(_, g)| g.start == start)
        else {
            if self
                .state
                .groups
                .last()
                .map(|g| g.end)
                .unwrap_or(self.state.start)
                == start
            {
                return Ok(None);
            }
            return Err("Emission offset is not a prepared way-group boundary".into());
        };
        let mut input = Records::open(
            &Sinks::path(&self.root.join("coordinates"), n),
            &group.spool,
        )?;
        let mut values = Vec::with_capacity((group.spool.len() / 16) as usize);
        while let Some(r) = input.next()? {
            values.push(Node {
                id: r.id,
                lat: f32::from_bits(r.value as u32),
                lon: f32::from_bits((r.value >> 32) as u32),
            });
        }
        Ok(Some((group.end, Prepared::sorted_nodes(values)?)))
    }
}

#[cfg(test)]
#[path = "planet_join_tests.rs"]
mod tests;
