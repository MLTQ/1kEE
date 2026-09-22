//! Coordinate lookups in the original PBF, with no expanded planet node file.
use osmpbf::{BlobDecode, BlobReader, Element};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::os::unix::fs::{FileExt, MetadataExt};
use std::path::{Path, PathBuf};

mod cache;
mod index;
pub use cache::Session;
pub use index::BuildState;

const MAX_BLOCK_BYTES: u64 = 33 * 1024 * 1024;
const MAX_BLOCK_NODES: usize = 2_000_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Stamp {
    len: u64,
    device: u64,
    inode: u64,
    seconds: i64,
    nanos: i64,
}
impl Stamp {
    fn read(path: &Path) -> Result<Self, String> {
        let m = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self {
            len: m.len(),
            device: m.dev(),
            inode: m.ino(),
            seconds: m.mtime(),
            nanos: m.mtime_nsec(),
        })
    }
}

#[derive(Clone, Copy, Debug)]
struct Block {
    offset: u64,
    length: u64,
    min: i64,
    max: i64,
    count: usize,
}
#[derive(Clone, Copy, Debug, PartialEq)]
struct Node {
    id: i64,
    lat: f32,
    lon: f32,
}

fn decode(blob: &osmpbf::Blob) -> Result<(Vec<Node>, bool), String> {
    let mut nodes = Vec::new();
    let mut ways = false;
    if let BlobDecode::OsmData(block) = blob.decode().map_err(|e| e.to_string())? {
        for e in block.elements() {
            let node = match e {
                Element::Node(n) => Node {
                    id: n.id(),
                    lat: n.lat() as f32,
                    lon: n.lon() as f32,
                },
                Element::DenseNode(n) => Node {
                    id: n.id(),
                    lat: n.lat() as f32,
                    lon: n.lon() as f32,
                },
                Element::Way(_) => {
                    ways = true;
                    continue;
                }
                _ => continue,
            };
            if nodes.last().is_some_and(|last: &Node| last.id >= node.id) {
                return Err("Compact lookup requires strictly increasing node IDs; use flat node storage for unordered/history PBFs".into());
            }
            if nodes.len() == MAX_BLOCK_NODES {
                return Err("PBF node block exceeds the compact decoder limit".into());
            }
            nodes.push(node);
        }
    }
    Ok((nodes, ways))
}

pub struct IndexProgress {
    pub bytes: u64,
    pub total: u64,
    pub nodes: u64,
    pub blocks: usize,
}

pub struct PbfNodes {
    file: File,
    path: PathBuf,
    stamp: Stamp,
    blocks: Vec<Block>,
    pub first_way: u64,
    pub node_count: u64,
}

impl PbfNodes {
    pub fn open(
        source: &Path,
        directory: &Path,
        progress: &mut dyn FnMut(IndexProgress),
    ) -> Result<(Self, BuildState), String> {
        index::open(source, directory, progress)
    }

    pub fn validate_source(&self) -> Result<(), String> {
        if Stamp::read(&self.path)? != self.stamp {
            return Err("PBF source changed during compact build; stop and use a new working directory for the new source".into());
        }
        Ok(())
    }

    pub fn session(&self) -> Session<'_> {
        Session::new(self)
    }
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    fn read_block(&self, number: usize) -> Result<Vec<Node>, String> {
        let b = self.blocks[number];
        let mut bytes = vec![0; b.length as usize];
        self.file
            .read_exact_at(&mut bytes, b.offset)
            .map_err(|e| format!("PBF node block at byte {}: {e}", b.offset))?;
        let mut reader = BlobReader::new(bytes.as_slice());
        let blob = reader
            .next()
            .ok_or("Missing PBF node block")?
            .map_err(|e| e.to_string())?;
        let (nodes, _) = decode(&blob)?;
        if reader.next().is_some()
            || nodes.len() != b.count
            || nodes.first().map(|n| n.id) != Some(b.min)
            || nodes.last().map(|n| n.id) != Some(b.max)
        {
            return Err(format!("PBF node index mismatch at byte {}", b.offset));
        }
        Ok(nodes)
    }
}

#[cfg(test)]
mod tests;
