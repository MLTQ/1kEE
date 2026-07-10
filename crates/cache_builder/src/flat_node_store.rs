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
    pub count: u64,
}

impl NodeWriter {
    pub fn create(path: &Path) -> Result<Self, String> {
        let file = File::create(path)
            .map_err(|e| format!("Cannot create node file {}: {e}", path.display()))?;
        Ok(Self {
            writer: BufWriter::with_capacity(4 * 1024 * 1024, file),
            count: 0,
        })
    }

    #[inline]
    pub fn write(&mut self, id: i64, lat: f32, lon: f32) -> Result<(), String> {
        self.writer
            .write_all(&id.to_le_bytes())
            .map_err(|e| e.to_string())?;
        self.writer
            .write_all(&lat.to_le_bytes())
            .map_err(|e| e.to_string())?;
        self.writer
            .write_all(&lon.to_le_bytes())
            .map_err(|e| e.to_string())?;
        self.count += 1;
        Ok(())
    }

    /// Restore an existing node file to an exact Pass 1 checkpoint before
    /// appending. Records that reached the OS page cache after the last saved
    /// checkpoint must be discarded so replaying the source blobs cannot
    /// duplicate them.
    pub fn append(path: &Path, checkpoint_record_count: u64) -> Result<Self, String> {
        let checkpoint_length = checkpoint_record_count
            .checked_mul(RECORD_BYTES)
            .ok_or_else(|| "Node checkpoint record count overflowed file length".to_owned())?;
        let actual_length = std::fs::metadata(path)
            .map_err(|e| format!("Cannot inspect node file {}: {e}", path.display()))?
            .len();
        if actual_length < checkpoint_length {
            return Err(format!(
                "Node file {} is shorter than its checkpoint ({actual_length} < {checkpoint_length} bytes)",
                path.display()
            ));
        }
        let file = std::fs::OpenOptions::new()
            .write(true)
            .append(true)
            .open(path)
            .map_err(|e| format!("Cannot open node file for append {}: {e}", path.display()))?;
        file.set_len(checkpoint_length)
            .map_err(|e| format!("Cannot truncate node file {}: {e}", path.display()))?;
        Ok(Self {
            writer: BufWriter::with_capacity(4 * 1024 * 1024, file),
            count: checkpoint_record_count,
        })
    }

    pub fn finish(mut self) -> Result<(), String> {
        self.checkpoint()
    }

    /// Flush node records and make them durable before advancing a resume
    /// checkpoint.  A checkpoint must never point beyond data that can be
    /// reopened by `NodeWriter::append` after an interrupted build.
    pub fn checkpoint(&mut self) -> Result<(), String> {
        self.writer.flush().map_err(|e| e.to_string())?;
        self.writer.get_ref().sync_data().map_err(|e| e.to_string())
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
    let num_chunks = (total_records as usize).div_ceil(SORT_CHUNK_RECORDS);
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
    progress(format!(
        "Sort phase 2: merging {} chunks…",
        chunk_paths.len()
    ));

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
        let file = File::open(path).map_err(|e| format!("Cannot open node file: {e}"))?;

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

    fn block_bounds(&self, target_id: i64) -> Option<(u64, u64)> {
        if self.record_count == 0 {
            return None;
        }
        // Narrow to the index block that must contain target_id (if present).
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

        (block_start < block_end).then_some((block_start, block_end))
    }

    fn read_block(&self, block_start: u64, block_end: u64, buffer: &mut Vec<u8>) -> Option<()> {
        let block_len = (block_end - block_start) as usize;
        buffer.resize(block_len * RECORD_BYTES as usize, 0);
        let bytes_read = self.file.read_at(buffer, block_start * RECORD_BYTES).ok()?;
        (bytes_read == buffer.len()).then_some(())
    }

    fn search_block(buffer: &[u8], target_id: i64) -> Option<(f32, f32)> {
        let block_len = buffer.len() / RECORD_BYTES as usize;

        // Binary search within the block (records are sorted by node_id).
        let mut lo = 0usize;
        let mut hi = block_len;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let base = mid * RECORD_BYTES as usize;
            let id = i64::from_le_bytes(buffer[base..base + 8].try_into().unwrap());
            match id.cmp(&target_id) {
                Ordering::Equal => {
                    let lat = f32::from_le_bytes(buffer[base + 8..base + 12].try_into().unwrap());
                    let lon = f32::from_le_bytes(buffer[base + 12..base + 16].try_into().unwrap());
                    return Some((lat, lon));
                }
                Ordering::Less => lo = mid + 1,
                Ordering::Greater => hi = mid,
            }
        }

        None
    }

    /// Resolve a batch of node IDs while retaining one output slot per input.
    ///
    /// References are grouped by sparse-index block, so every block is read at
    /// most once. This preserves the exact `lookup` result for each ID while
    /// avoiding a temporary 64 KiB allocation and positional read per ref.
    pub fn lookup_many(&self, target_ids: &[i64]) -> Vec<Option<(f32, f32)>> {
        let mut results = vec![None; target_ids.len()];
        if target_ids.is_empty() || self.record_count == 0 {
            return results;
        }

        let mut queries = Vec::with_capacity(target_ids.len());
        for (result_index, &target_id) in target_ids.iter().enumerate() {
            if let Some((block_start, block_end)) = self.block_bounds(target_id) {
                queries.push((block_start, block_end, result_index));
            }
        }
        queries.sort_unstable_by_key(|(block_start, _, _)| *block_start);

        let mut buffer = Vec::new();
        let mut query_start = 0usize;
        while query_start < queries.len() {
            let (block_start, block_end, _) = queries[query_start];
            let mut query_end = query_start + 1;
            while query_end < queries.len() && queries[query_end].0 == block_start {
                query_end += 1;
            }

            if self
                .read_block(block_start, block_end, &mut buffer)
                .is_some()
            {
                for &(_, _, result_index) in &queries[query_start..query_end] {
                    results[result_index] = Self::search_block(&buffer, target_ids[result_index]);
                }
            }
            query_start = query_end;
        }

        results
    }
}

// SAFETY: `File::read_at` is implemented as `pread64` on POSIX, which is
// thread-safe — it does not read or modify the file cursor.
unsafe impl Sync for NodeLookup {}

#[cfg(test)]
mod tests {
    use super::{INDEX_STRIDE, NodeLookup, NodeWriter};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn lookup_many_preserves_input_order_across_blocks() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "one_thousand_electric_eye_node_lookup_{}_{}",
            std::process::id(),
            unique
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("nodes.bin");

        let record_count = INDEX_STRIDE + 3;
        let mut writer = NodeWriter::create(&path).unwrap();
        for index in 0..record_count {
            writer
                .write(index as i64 * 10 + 10, index as f32 + 0.25, -(index as f32))
                .unwrap();
        }
        writer.finish().unwrap();

        let lookup = NodeLookup::open(&path, record_count).unwrap();
        let first_block_last_id = (INDEX_STRIDE - 1) as i64 * 10 + 10;
        let second_block_first_id = INDEX_STRIDE as i64 * 10 + 10;
        let results = lookup.lookup_many(&[
            second_block_first_id,
            7,
            10,
            first_block_last_id,
            second_block_first_id,
        ]);

        assert_eq!(results.len(), 5);
        assert_eq!(
            results[0],
            Some((INDEX_STRIDE as f32 + 0.25, -(INDEX_STRIDE as f32)))
        );
        assert_eq!(results[1], None);
        assert_eq!(results[2], Some((0.25, 0.0)));
        assert_eq!(
            results[3],
            Some((INDEX_STRIDE as f32 - 0.75, -((INDEX_STRIDE - 1) as f32)))
        );
        assert_eq!(results[4], results[0]);

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn append_discards_records_written_after_the_checkpoint() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "one_thousand_electric_eye_node_resume_{}_{}",
            std::process::id(),
            unique
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("nodes.bin");

        let mut writer = NodeWriter::create(&path).unwrap();
        for id in 1..=5 {
            writer.write(id, id as f32, -(id as f32)).unwrap();
        }
        writer.finish().unwrap();

        let mut resumed = NodeWriter::append(&path, 3).unwrap();
        assert_eq!(resumed.count, 3);
        resumed.write(4, 4.0, -4.0).unwrap();
        resumed.finish().unwrap();

        assert_eq!(fs::metadata(&path).unwrap().len(), 4 * super::RECORD_BYTES);
        let lookup = NodeLookup::open(&path, 4).unwrap();
        assert_eq!(
            lookup.lookup_many(&[1, 2, 3, 4, 5]),
            vec![
                Some((1.0, -1.0)),
                Some((2.0, -2.0)),
                Some((3.0, -3.0)),
                Some((4.0, -4.0)),
                None,
            ]
        );

        fs::remove_dir_all(dir).unwrap();
    }
}
