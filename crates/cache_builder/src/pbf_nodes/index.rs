//! Small resumable block index; each transaction publishes descriptors + scan offset.
use super::{Block, IndexProgress, MAX_BLOCK_BYTES, MAX_BLOCK_NODES, PbfNodes, Stamp, decode};
use crate::roads::open_planet_at;
use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::path::Path;
use std::sync::atomic::Ordering;

const APP_ID: i32 = 0x314b504e;
const MAX_BLOCKS: usize = 8_000_000;
const BATCH: usize = 32;

#[derive(Serialize, Deserialize)]
struct State {
    source: Stamp,
    offset: u64,
    nodes: u64,
    first_way: Option<u64>,
    indexed: bool,
    build_options: Option<String>,
    build_done: bool,
}

pub struct BuildState {
    connection: Connection,
    state: State,
    _lock: File,
}
fn save(connection: &Connection, state: &State) -> Result<(), String> {
    let text = serde_json::to_string(state).map_err(|e| e.to_string())?;
    connection
        .execute("INSERT OR REPLACE INTO state VALUES (1,?1)", [text])
        .map_err(|e| e.to_string())?;
    Ok(())
}
impl BuildState {
    pub fn bind_build(&mut self, options: String) -> Result<bool, String> {
        if let Some(old) = &self.state.build_options {
            if old != &options {
                return Err("Compact build settings/output changed. Choose a new working directory; previous resume state was preserved".into());
            }
        } else {
            self.state.build_options = Some(options);
            save(&self.connection, &self.state)?;
        }
        Ok(self.state.build_done)
    }
    pub fn finish(&mut self) -> Result<(), String> {
        self.state.build_done = true;
        save(&self.connection, &self.state)
    }
}

fn validate(blocks: &[Block], state: &State) -> Result<(), String> {
    let mut last = None;
    let mut end = 0;
    let mut nodes = 0u64;
    for b in blocks {
        if b.length == 0
            || b.length > MAX_BLOCK_BYTES
            || b.count == 0
            || b.count > MAX_BLOCK_NODES
            || b.min > b.max
            || last.is_some_and(|v| v >= b.min)
            || b.offset < end
            || b.offset
                .checked_add(b.length)
                .is_none_or(|v| v > state.offset)
        {
            return Err("Invalid compact PBF block index; choose a new working directory".into());
        }
        last = Some(b.max);
        end = b.offset + b.length;
        nodes += b.count as u64;
    }
    if state.offset > state.source.len
        || nodes != state.nodes
        || state.first_way.is_some_and(|o| o >= state.offset)
        || (state.indexed && state.offset != state.source.len)
    {
        return Err("Invalid compact PBF scan checkpoint".into());
    }
    Ok(())
}

pub(super) fn open(
    source: &Path,
    directory: &Path,
    progress: &mut dyn FnMut(IndexProgress),
) -> Result<(PbfNodes, BuildState), String> {
    let stamp = Stamp::read(source)?;
    fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("build.lock"))
        .map_err(|e| e.to_string())?;
    lock.try_lock().map_err(|e| {
        format!(
            "Cannot lock compact working folder {} (another build may be running): {e}",
            directory.display()
        )
    })?;
    let path = directory.join("nodes.sqlite");
    let existed = path.exists();
    let mut connection = Connection::open(&path).map_err(|e| e.to_string())?;
    if !existed {
        connection.execute_batch(&format!(
            "PRAGMA application_id={APP_ID}; PRAGMA user_version=1; PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;
             CREATE TABLE blocks(offset INTEGER PRIMARY KEY, length INTEGER NOT NULL, min_id INTEGER NOT NULL, max_id INTEGER NOT NULL, count INTEGER NOT NULL);
             CREATE TABLE state(id INTEGER PRIMARY KEY CHECK(id=1), json TEXT NOT NULL);"
        )).map_err(|e| e.to_string())?;
    }
    let app: i32 = connection
        .pragma_query_value(None, "application_id", |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let version: i32 = connection
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if app != APP_ID || version != 1 {
        return Err("Unsupported compact PBF index".into());
    }
    let saved: Option<String> = connection
        .query_row("SELECT json FROM state WHERE id=1", [], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?;
    let mut state = match saved {
        Some(text) => serde_json::from_str::<State>(&text)
            .map_err(|e| format!("Invalid compact checkpoint: {e}"))?,
        None => {
            let s = State {
                source: stamp.clone(),
                offset: 0,
                nodes: 0,
                first_way: None,
                indexed: false,
                build_options: None,
                build_done: false,
            };
            save(&connection, &s)?;
            s
        }
    };
    if state.source != stamp {
        return Err("The PBF source differs from this compact index. Choose a new working directory; existing build files were preserved".into());
    }
    let mut blocks = Vec::new();
    {
        let mut statement = connection
            .prepare("SELECT offset,length,min_id,max_id,count FROM blocks ORDER BY offset")
            .map_err(|e| e.to_string())?;
        let mut rows = statement.query([]).map_err(|e| e.to_string())?;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            if blocks.len() == MAX_BLOCKS {
                return Err("Compact node index exceeds supported block count".into());
            }
            blocks.push(Block {
                offset: row.get(0).map_err(|e| e.to_string())?,
                length: row.get(1).map_err(|e| e.to_string())?,
                min: row.get(2).map_err(|e| e.to_string())?,
                max: row.get(3).map_err(|e| e.to_string())?,
                count: row.get(4).map_err(|e| e.to_string())?,
            });
        }
    }
    validate(&blocks, &state)?;
    if !state.indexed {
        let (mut reader, position) = open_planet_at(source, state.offset)?;
        loop {
            let mut batch = Vec::with_capacity(BATCH);
            for _ in 0..BATCH {
                let start = position.load(Ordering::Relaxed);
                let Some(blob) = reader.next() else {
                    break;
                };
                let blob = blob.map_err(|e| format!("Indexing PBF at byte {start}: {e}"))?;
                if start == 0 && !matches!(blob.get_type(), osmpbf::BlobType::OsmHeader) {
                    return Err("PBF must start with an OSM header".into());
                }
                let length = position.load(Ordering::Relaxed) - start;
                if length > MAX_BLOCK_BYTES {
                    return Err("PBF compressed block exceeds supported size".into());
                }
                batch.push((start, length, blob));
            }
            if batch.is_empty() {
                break;
            }
            // Only descriptors survive a batch; decoded node arrays are released.
            let decoded = batch
                .par_iter()
                .map(|(offset, length, blob)| {
                    let (nodes, has_ways) = decode(blob)?;
                    let descriptor = nodes.first().map(|first| Block {
                        offset: *offset,
                        length: *length,
                        min: first.id,
                        max: nodes.last().unwrap().id,
                        count: nodes.len(),
                    });
                    Ok((descriptor, has_ways))
                })
                .collect::<Result<Vec<_>, String>>()?;
            let mut added = Vec::new();
            let mut last = blocks.last().map(|b| b.max);
            for ((offset, _, _), (descriptor, has_ways)) in batch.iter().zip(decoded) {
                if has_ways && state.first_way.is_none() {
                    state.first_way = Some(*offset);
                }
                if let Some(b) = descriptor {
                    if last.is_some_and(|id| id >= b.min) {
                        return Err("Compact lookup requires globally increasing node IDs; use flat node storage for unordered/history PBFs".into());
                    }
                    last = Some(b.max);
                    state.nodes += b.count as u64;
                    added.push(b);
                }
            }
            if blocks.len() + added.len() > MAX_BLOCKS {
                return Err("Compact node index exceeds supported block count".into());
            }
            if Stamp::read(source)? != stamp {
                return Err(
                    "PBF changed while indexing; previous durable checkpoint retained".into(),
                );
            }
            state.offset = position.load(Ordering::Relaxed);
            let tx = connection.transaction().map_err(|e| e.to_string())?;
            for b in &added {
                tx.execute(
                    "INSERT INTO blocks VALUES (?1,?2,?3,?4,?5)",
                    params![b.offset, b.length, b.min, b.max, b.count],
                )
                .map_err(|e| e.to_string())?;
            }
            save(&tx, &state)?;
            tx.commit().map_err(|e| e.to_string())?;
            blocks.extend(added);
            progress(IndexProgress {
                bytes: state.offset,
                total: stamp.len,
                nodes: state.nodes,
                blocks: blocks.len(),
            });
        }
        if state.offset != stamp.len || Stamp::read(source)? != stamp || state.offset == 0 {
            return Err("PBF scan ended unexpectedly or source changed".into());
        }
        state.indexed = true;
        save(&connection, &state)?;
    }
    validate(&blocks, &state)?;
    let file = File::open(source).map_err(|e| e.to_string())?;
    if Stamp::read(source)? != stamp {
        return Err("PBF changed before opening lookups".into());
    }
    let nodes = PbfNodes {
        file,
        path: source.to_owned(),
        stamp,
        blocks,
        first_way: state.first_way.unwrap_or(state.source.len),
        node_count: state.nodes,
    };
    Ok((
        nodes,
        BuildState {
            connection,
            state,
            _lock: lock,
        },
    ))
}
