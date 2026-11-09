//! Bounded, checksummed append spools and external sorting for the node join.
use ring::digest::{Context, SHA256};
use serde::{Deserialize, Serialize};
use std::{
    cmp::Reverse,
    collections::BinaryHeap,
    fs::{self, File, OpenOptions},
    io::{self, BufReader, Read, Write},
    path::{Path, PathBuf},
};

pub(super) type Result<T> = std::result::Result<T, String>;
const SORT_ERROR: &str = "Temporary coordinate sort failed: ";
pub(super) fn sort_error(message: impl std::fmt::Display) -> String {
    format!("{SORT_ERROR}{message}")
}
pub(super) fn is_sort_error(error: &str) -> bool {
    error.starts_with(SORT_ERROR)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct Spool {
    pub segments: Vec<Segment>,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Segment {
    len: u64,
    digest: String,
}
impl Spool {
    pub fn len(&self) -> u64 {
        self.segments.iter().map(|s| s.len).sum()
    }
    pub fn restore(&self, path: &Path) -> Result<()> {
        let expected = self.len();
        if !path.exists() && expected == 0 {
            return Ok(());
        }
        let file = OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let len = file.metadata().map_err(|e| e.to_string())?.len();
        if len < expected {
            return Err(format!(
                "Join spool {} is shorter than its checkpoint",
                path.display()
            ));
        }
        if len > expected {
            file.set_len(expected)
                .and_then(|_| file.sync_all())
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(super) struct Record {
    pub id: i64,
    pub value: u64,
}
impl Record {
    fn bytes(self) -> [u8; 16] {
        let mut b = [0; 16];
        b[..8].copy_from_slice(&self.id.to_le_bytes());
        b[8..].copy_from_slice(&self.value.to_le_bytes());
        b
    }
}
struct Sink {
    buffer: Vec<u8>,
    hash: Context,
    len: u64,
}
pub(super) struct Sinks {
    root: PathBuf,
    entries: Vec<Option<Sink>>,
}
impl Sinks {
    pub fn new(root: &Path, count: usize) -> Result<Self> {
        fs::create_dir_all(root).map_err(|e| e.to_string())?;
        Ok(Self {
            root: root.into(),
            entries: (0..count).map(|_| None).collect(),
        })
    }
    pub fn path(root: &Path, n: usize) -> PathBuf {
        root.join(format!("{n:06}.bin"))
    }
    pub fn push(&mut self, n: usize, record: Record) -> Result<()> {
        let sink = self
            .entries
            .get_mut(n)
            .ok_or("Join spool group out of bounds")?
            .get_or_insert_with(|| Sink {
                buffer: Vec::with_capacity(65536),
                hash: Context::new(&SHA256),
                len: 0,
            });
        sink.buffer.extend_from_slice(&record.bytes());
        if sink.buffer.len() >= 65536 {
            Self::flush(&self.root, n, sink)?;
        }
        Ok(())
    }
    fn flush(root: &Path, n: usize, sink: &mut Sink) -> Result<()> {
        if sink.buffer.is_empty() {
            return Ok(());
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(Self::path(root, n))
            .map_err(|e| e.to_string())?;
        file.write_all(&sink.buffer).map_err(|e| e.to_string())?;
        sink.hash.update(&sink.buffer);
        sink.len += sink.buffer.len() as u64;
        sink.buffer.clear();
        Ok(())
    }
    pub fn finish(mut self, spools: &mut [Spool]) -> Result<()> {
        for (n, entry) in self.entries.iter_mut().enumerate() {
            if let Some(mut sink) = entry.take() {
                Self::flush(&self.root, n, &mut sink)?;
                File::open(Self::path(&self.root, n))
                    .and_then(|f| f.sync_all())
                    .map_err(|e| e.to_string())?;
                spools[n].segments.push(Segment {
                    len: sink.len,
                    digest: hex(sink.hash.finish().as_ref()),
                });
            }
        }
        Ok(())
    }
}
struct Verified {
    file: File,
    spool: Spool,
    segment: usize,
    read: u64,
    hash: Context,
}
impl Read for Verified {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let Some(segment) = self.spool.segments.get(self.segment) else {
            return Ok(0);
        };
        let len = buf.len().min((segment.len - self.read) as usize);
        let n = self.file.read(&mut buf[..len])?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Truncated join spool",
            ));
        }
        self.hash.update(&buf[..n]);
        self.read += n as u64;
        if self.read == segment.len {
            let hash = std::mem::replace(&mut self.hash, Context::new(&SHA256)).finish();
            if hex(hash.as_ref()) != segment.digest {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Join spool checksum mismatch",
                ));
            }
            self.segment += 1;
            self.read = 0;
        }
        Ok(n)
    }
}
pub(super) struct Records(Option<BufReader<Verified>>);
impl Records {
    pub fn open(path: &Path, spool: &Spool) -> Result<Self> {
        if spool.len() == 0 {
            return Ok(Self(None));
        }
        let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if file.metadata().map_err(|e| e.to_string())?.len() != spool.len()
            || spool.segments.iter().any(|s| s.len == 0 || s.len % 16 != 0)
        {
            return Err("Invalid join spool length".into());
        }
        Ok(Self(Some(BufReader::with_capacity(
            1024 * 1024,
            Verified {
                file,
                spool: spool.clone(),
                segment: 0,
                read: 0,
                hash: Context::new(&SHA256),
            },
        ))))
    }
    pub fn next(&mut self) -> Result<Option<Record>> {
        let Some(reader) = &mut self.0 else {
            return Ok(None);
        };
        let mut bytes = [0u8; 16];
        if reader.read(&mut bytes[..1]).map_err(|e| e.to_string())? == 0 {
            return Ok(None);
        }
        reader
            .read_exact(&mut bytes[1..])
            .map_err(|e| e.to_string())?;
        Ok(Some(Record {
            id: i64::from_le_bytes(bytes[..8].try_into().unwrap()),
            value: u64::from_le_bytes(bytes[8..].try_into().unwrap()),
        }))
    }
}
struct Run {
    dir: PathBuf,
    spool: Spool,
}
pub(super) struct Merge {
    readers: Vec<Records>,
    heap: BinaryHeap<Reverse<(Record, usize)>>,
    paths: Vec<PathBuf>,
}
impl Merge {
    fn new(runs: &[Run]) -> Result<Self> {
        let mut result = Self {
            readers: Vec::new(),
            heap: BinaryHeap::new(),
            paths: Vec::new(),
        };
        for (i, run) in runs.iter().enumerate() {
            let path = Sinks::path(&run.dir, 0);
            let contextualize = |e| sort_error(format!("{}: {e}", path.display()));
            let mut reader = Records::open(&path, &run.spool).map_err(contextualize)?;
            if let Some(record) = reader.next().map_err(contextualize)? {
                result.heap.push(Reverse((record, i)));
            }
            result.readers.push(reader);
            result.paths.push(path);
        }
        Ok(result)
    }
    pub fn next(&mut self) -> Result<Option<Record>> {
        let Some(Reverse((record, i))) = self.heap.pop() else {
            return Ok(None);
        };
        if let Some(next) = self.readers[i]
            .next()
            .map_err(|e| sort_error(format!("{}: {e}", self.paths[i].display())))?
        {
            if next < record {
                return Err(sort_error(format!(
                    "{}: records are out of order",
                    self.paths[i].display()
                )));
            }
            self.heap.push(Reverse((next, i)));
        }
        Ok(Some(record))
    }
}
/// 64 MiB sort chunks, at most 32 input streams per merge. Temporary runs are
/// reconstructible and never constitute durable progress.
pub(super) fn sorted(
    path: &Path,
    spool: &Spool,
    work: &Path,
    chunk: usize,
    progress: &mut dyn FnMut(u64),
) -> Result<Merge> {
    if work.exists() {
        fs::remove_dir_all(work).map_err(|e| e.to_string())?;
    }
    fs::create_dir_all(work).map_err(|e| e.to_string())?;
    let mut input = Records::open(path, spool)?;
    let mut runs = Vec::new();
    let mut number = 0;
    let mut bytes = 0;
    loop {
        let mut records = Vec::with_capacity(chunk);
        while records.len() < chunk {
            match input.next()? {
                Some(r) => records.push(r),
                None => break,
            }
        }
        if records.is_empty() {
            break;
        }
        records.sort_unstable();
        let dir = work.join(number.to_string());
        number += 1;
        let mut sink = Sinks::new(&dir, 1)?;
        let mut spools = vec![Spool::default()];
        for record in records {
            sink.push(0, record)?;
            bytes += 16;
        }
        sink.finish(&mut spools)?;
        runs.push(Run {
            dir,
            spool: spools.remove(0),
        });
        progress(bytes);
    }
    while runs.len() > 32 {
        let mut merged = Vec::new();
        for group in runs.chunks(32) {
            let dir = work.join(number.to_string());
            number += 1;
            let mut sink = Sinks::new(&dir, 1)?;
            let mut spools = vec![Spool::default()];
            let mut input = Merge::new(group)?;
            let mut merged_bytes = 0u64;
            while let Some(record) = input.next()? {
                sink.push(0, record)?;
                merged_bytes += 16;
                if merged_bytes.is_multiple_of(1024 * 1024) {
                    progress(bytes + merged_bytes);
                }
            }
            sink.finish(&mut spools)?;
            merged.push(Run {
                dir,
                spool: spools.remove(0),
            });
            for run in group {
                fs::remove_dir_all(&run.dir).map_err(|e| e.to_string())?;
            }
            progress(bytes);
        }
        runs = merged;
    }
    Merge::new(&runs)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn merge_rejects_out_of_order_records_even_with_valid_checksum() {
        let path = std::env::temp_dir().join(format!(
            "1kee-sort-order-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut sink = Sinks::new(&path, 1).unwrap();
        sink.push(0, Record { id: 2, value: 0 }).unwrap();
        sink.push(0, Record { id: 1, value: 0 }).unwrap();
        let mut spools = vec![Spool::default()];
        sink.finish(&mut spools).unwrap();
        let mut merge = Merge::new(&[Run {
            dir: path.clone(),
            spool: spools.remove(0),
        }])
        .unwrap();
        let error = merge.next().err().unwrap();
        assert!(is_sort_error(&error));
        assert!(error.contains("out of order"));
        drop(merge);
        fs::remove_dir_all(path).unwrap();
    }
}
