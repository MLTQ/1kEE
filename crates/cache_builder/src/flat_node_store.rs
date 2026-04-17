//! Disk-backed node position store for planet-scale OSM processing.
//!
//! # Layout
//! Records are 16 bytes on disk: `i64 node_id LE | f32 lat LE | f32 lon LE`.
//! Pass 1 streams records into a flat binary file in whatever order the PBF
//! delivers them.  After collection, `sort_in_place` performs an external
//! k-way merge sort (512 MiB chunks) so Pass 2 can do O(log N) lookups.
//!
//! # Lookup
//! A sparse in-memory index (one entry per `INDEX_STRIDE` records, ~32 MiB
//! for 8 B nodes) narrows each lookup to a ≤4096-record linear scan.
//! `read_at` (POSIX `pread64`) is used for all disk access so `NodeLookup`
//! is `Sync` and can be shared across Rayon threads without locking.
//!
//! # Sorted-check optimisation
//! Planet PBF files from openstreetmap.org are emitted with nodes in
//! ascending ID order.  `sort_in_place` first checks whether the file is
//! already sorted (O(N) scan, no extra disk space) and skips the sort if so.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

pub const RECORD_BYTES: u64 = 16; // i64 id + f32 lat + f32 lon

/// Number of records to load into RAM per sort chunk (512 MiB).
const SORT_CHUNK_RECORDS: usize = 512 * 1024 * 1024 / RECORD_BYTES as usize;

/// One sparse-index entry covers this many consecutive records.
const INDEX_STRIDE: u64 = 4096;

// ── Pass-1 writer ─────────────────────────────────────────────────────────────

pub struct NodeWriter {
    writer: BufWriter<File>,
    pub path: PathBuf,
    pub count: u64,
}

impl NodeWriter {
    pub fn create(path: &Path) -> Result<Self, String> {
        let file = File::create(path)
            .map_err(|e| format!("Cannot create node file {}: {e}", path.display()))?;
        Ok(Self {
            writer: BufWriter::with_capacity(4 * 1024 * 1024, file),
            path: path.to_path_buf(),
            count: 0,
        })
    }

    #[inline]
    pub fn write(&mut self, id: i64, lat: f32, lon: f32) -> Result<(), String> {
        self.writer.write_all(&id.to_le_bytes()).map_err(|e| e.to_string())?;
        self.writer.write_all(&lat.to_le_bytes()).map_err(|e| e.to_string())?;
        self.writer.write_all(&lon.to_le_bytes()).map_err(|e| e.to_string())?;
        self.count += 1;
        Ok(())
    }

    /// Open an existing node file for appending (Pass 1 resume).
    pub fn append(path: &Path) -> Result<Self, String> {
        let existing_count = std::fs::metadata(path)
            .map(|m| m.len() / RECORD_BYTES)
            .unwrap_or(0);
        let file = std::fs::OpenOptions::new()
            .write(true)
            .append(true)
            .open(path)
            .map_err(|e| format!("Cannot open node file for append {}: {e}", path.display()))?;
        Ok(Self {
            writer: BufWriter::with_capacity(4 * 1024 * 1024, file),
            path: path.to_path_buf(),
            count: existing_count,
        })
    }

    pub fn finish(mut self) -> Result<(), String> {
        self.writer.flush().map_err(|e| e.to_string())
    }
}

// ── Sort ──────────────────────────────────────────────────────────────────────

/// Sort the flat node file in-place by node ID using an external k-way merge.
///
/// `tmp_dir` receives the sorted chunk files; they are removed after merge.
/// If the file is already sorted (typical for official planet exports), the
/// sort is skipped entirely.
pub fn sort_in_place(
    path: &Path,
    tmp_dir: &Path,
    total_records: u64,
    progress: &mut dyn FnMut(String),
) -> Result<(), String> {
    if total_records == 0 {
        return Ok(());
    }

    // Fast pre-check: single O(N) scan to test if already sorted.
    if is_sorted(path, total_records)? {
        progress("Node file already sorted — skipping sort pass.".to_owned());
        return Ok(());
    }

    progress(format!(
        "Sorting {total_records} node records via external merge sort…"
    ));
    fs::create_dir_all(tmp_dir).map_err(|e| e.to_string())?;

    // ── Phase 1: produce sorted chunks ───────────────────────────────────────
    let num_chunks =
        (total_records as usize).div_ceil(SORT_CHUNK_RECORDS);
    let mut chunk_paths: Vec<PathBuf> = Vec::with_capacity(num_chunks);

    {
        let mut reader = BufReader::with_capacity(
            8 * 1024 * 1024,
            File::open(path).map_err(|e| e.to_string())?,
        );
        let mut buf = vec![[0u8; 16]; SORT_CHUNK_RECORDS];

        for chunk_idx in 0..num_chunks {
            let start_rec = chunk_idx as u64 * SORT_CHUNK_RECORDS as u64;
            let count = ((total_records - start_rec) as usize).min(SORT_CHUNK_RECORDS);
            let chunk = &mut buf[..count];

            for rec in chunk.iter_mut() {
                reader.read_exact(rec).map_err(|e| e.to_string())?;
            }

            chunk.sort_unstable_by_key(|r| i64::from_le_bytes(r[..8].try_into().unwrap()));

            let chunk_path = tmp_dir.join(format!("node_chunk_{chunk_idx:06}.bin"));
            {
                let mut out = BufWriter::with_capacity(
                    4 * 1024 * 1024,
                    File::create(&chunk_path).map_err(|e| e.to_string())?,
                );
                for rec in chunk.iter() {
                    out.write_all(rec).map_err(|e| e.to_string())?;
                }
                out.flush().map_err(|e| e.to_string())?;
            }
            chunk_paths.push(chunk_path);
            progress(format!(
                "Sort phase 1: chunk {}/{num_chunks}",
                chunk_idx + 1
            ));
        }
    }

    if chunk_paths.len() == 1 {
        // Only one chunk — rename directly, no merge needed.
        fs::rename(&chunk_paths[0], path).map_err(|e| e.to_string())?;
        return Ok(());
    }

    // ── Phase 2: k-way merge ─────────────────────────────────────────────────
    progress(format!("Sort phase 2: merging {} chunks…", chunk_paths.len()));

    let mut readers: Vec<BufReader<File>> = chunk_paths
        .iter()
        .map(|p| {
            File::open(p)
                .map(|f| BufReader::with_capacity(256 * 1024, f))
                .map_err(|e| e.to_string())
        })
        .collect::<Result<_, _>>()?;

    // Min-heap entry: (node_id, chunk_index, raw_record)
    #[derive(Eq, PartialEq)]
    struct Entry(i64, usize, [u8; 16]);
    impl Ord for Entry {
        fn cmp(&self, other: &Self) -> Ordering {
            // Reverse so BinaryHeap becomes a min-heap by node_id
            other.0.cmp(&self.0).then(other.1.cmp(&self.1))
        }
    }
    impl PartialOrd for Entry {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            Some(self.cmp(other))
        }
    }

    let mut heap: BinaryHeap<Entry> = BinaryHeap::with_capacity(chunk_paths.len());
    for (idx, rdr) in readers.iter_mut().enumerate() {
        let mut rec = [0u8; 16];
        if rdr.read_exact(&mut rec).is_ok() {
            let id = i64::from_le_bytes(rec[..8].try_into().unwrap());
            heap.push(Entry(id, idx, rec));
        }
    }

    let tmp_out = path.with_extension("merging");
    {
        let mut out = BufWriter::with_capacity(
            8 * 1024 * 1024,
            File::create(&tmp_out).map_err(|e| e.to_string())?,
        );
        while let Some(Entry(_, idx, rec)) = heap.pop() {
            out.write_all(&rec).map_err(|e| e.to_string())?;
            let mut next = [0u8; 16];
            if readers[idx].read_exact(&mut next).is_ok() {
                let id = i64::from_le_bytes(next[..8].try_into().unwrap());
                heap.push(Entry(id, idx, next));
            }
        }
        out.flush().map_err(|e| e.to_string())?;
    }

    // Drop readers before renaming (Windows compatibility, no-op on POSIX).
    drop(readers);
    fs::rename(&tmp_out, path).map_err(|e| e.to_string())?;
    for p in &chunk_paths {
        let _ = fs::remove_file(p);
    }

    Ok(())
}

fn is_sorted(path: &Path, total_records: u64) -> Result<bool, String> {
    let mut reader = BufReader::with_capacity(
        4 * 1024 * 1024,
        File::open(path).map_err(|e| e.to_string())?,
    );
    let mut prev_id = i64::MIN;
    let mut buf = [0u8; 16];
    for _ in 0..total_records {
        reader.read_exact(&mut buf).map_err(|e| e.to_string())?;
        let id = i64::from_le_bytes(buf[..8].try_into().unwrap());
        if id < prev_id {
            return Ok(false);
        }
        prev_id = id;
    }
    Ok(true)
}

// ── Lookup ────────────────────────────────────────────────────────────────────

/// Thread-safe read-only lookup into the sorted flat node file.
///
/// Shared via `Arc<NodeLookup>` across Rayon threads.
pub struct NodeLookup {
    file: File,
    record_count: u64,
    /// Sparse index: `(node_id_at_record_i, record_i)`, one entry per
    /// `INDEX_STRIDE` records.  Fits in ~32 MiB for 8 B nodes.
    index: Vec<(i64, u64)>,
}

impl NodeLookup {
    pub fn open(path: &Path, record_count: u64) -> Result<Self, String> {
        let file =
            File::open(path).map_err(|e| format!("Cannot open node file: {e}"))?;

        let mut index = Vec::with_capacity((record_count / INDEX_STRIDE + 1) as usize);
        let mut id_buf = [0u8; 8];
        let mut i = 0u64;
        while i < record_count {
            file.read_at(&mut id_buf, i * RECORD_BYTES)
                .map_err(|e| e.to_string())?;
            let id = i64::from_le_bytes(id_buf);
            index.push((id, i));
            i += INDEX_STRIDE;
        }

        Ok(Self {
            file,
            record_count,
            index,
        })
    }

    /// Look up `target_id`.  Returns `(lat, lon)` or `None` if not found.
    pub fn lookup(&self, target_id: i64) -> Option<(f32, f32)> {
        if self.record_count == 0 {
            return None;
        }

        // Narrow to the block that must contain target_id (if present).
        let idx_pos = self.index.partition_point(|(id, _)| *id <= target_id);
        let block_start = if idx_pos == 0 {
            0
        } else {
            self.index[idx_pos - 1].1
        };
        let block_end = if idx_pos < self.index.len() {
            self.index[idx_pos].1.min(self.record_count)
        } else {
            self.record_count
        };

        // Linear scan within the block (≤ INDEX_STRIDE = 4 096 records).
        let mut buf = [0u8; 16];
        for rec in block_start..block_end {
            self.file
                .read_at(&mut buf, rec * RECORD_BYTES)
                .ok()?;
            let id = i64::from_le_bytes(buf[..8].try_into().unwrap());
            match id.cmp(&target_id) {
                Ordering::Equal => {
                    let lat = f32::from_le_bytes(buf[8..12].try_into().unwrap());
                    let lon = f32::from_le_bytes(buf[12..16].try_into().unwrap());
                    return Some((lat, lon));
                }
                Ordering::Greater => break, // sorted — target absent
                Ordering::Less => {}
            }
        }
        None
    }
}

// SAFETY: `File::read_at` is implemented as `pread64` on POSIX, which is
// thread-safe — it does not read or modify the file cursor.
unsafe impl Sync for NodeLookup {}
