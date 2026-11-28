//! Bounded, checksummed append spools and external sorting for the node join.
use hashlink::lru_cache::LruCache;
use rayon::prelude::*;
use ring::digest::{Context, SHA256};
use serde::{Deserialize, Serialize};
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, binary_heap::PeekMut},
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
};

const OPEN_HANDLE_POOL: usize = 128;

#[cfg(target_os = "linux")]
const EXT4_SUPER_MAGIC: u64 = 0x0000_EF53;
#[cfg(target_os = "linux")]
const BTRFS_SUPER_MAGIC: u64 = 0x9123_683E;
#[cfg(target_os = "linux")]
const XFS_SUPER_MAGIC: u64 = 0x5846_5342;
#[cfg(target_os = "linux")]
const F2FS_SUPER_MAGIC: u64 = 0xF2F5_2010;
#[cfg(target_os = "linux")]
const TMPFS_MAGIC: u64 = 0x0102_1994;
#[cfg(target_os = "linux")]
const ZFS_SUPER_MAGIC: u64 = 0x2FC1_2FC1;
#[cfg(target_os = "linux")]
const BCACHEFS_SUPER_MAGIC: u64 = 0xCA45_1A4E;
#[cfg(target_os = "linux")]
const EXFAT_SUPER_MAGIC: u64 = 0x2011_BAB0;
#[cfg(target_os = "linux")]
const MSDOS_SUPER_MAGIC: u64 = 0x0000_4D44;

/// Returns true only for native Linux filesystems where `syncfs()` does not
/// suffer from stale block-device `buffer_head` aliases (unlike `exfat`/`vfat`,
/// where `syncfs()` runs `sync_inodes_sb()` before `sync_blockdev()` and can
/// overwrite newly allocated file clusters with dirty freed-directory sectors).
#[cfg(target_os = "linux")]
fn is_syncfs_safe_fs_type(fs_type: u64) -> bool {
    matches!(
        fs_type,
        EXT4_SUPER_MAGIC
            | BTRFS_SUPER_MAGIC
            | XFS_SUPER_MAGIC
            | F2FS_SUPER_MAGIC
            | TMPFS_MAGIC
            | ZFS_SUPER_MAGIC
            | BCACHEFS_SUPER_MAGIC
    )
}

#[cfg(target_os = "linux")]
fn is_fat_family_fs_type(fs_type: u64) -> bool {
    matches!(fs_type, EXFAT_SUPER_MAGIC | MSDOS_SUPER_MAGIC)
}

#[cfg(target_os = "linux")]
fn dir_fs_type(dir: &File) -> Option<u64> {
    use std::os::fd::AsRawFd;
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    let rc = unsafe { libc::fstatfs(dir.as_raw_fd(), stat.as_mut_ptr()) };
    if rc == 0 {
        let stat = unsafe { stat.assume_init() };
        Some(stat.f_type as u64)
    } else {
        None
    }
}

/// On exFAT/vFAT, flush any pre-existing dirty block-device directory buffers
/// *before* allocating new spool clusters so stale freed-directory `buffer_head`s
/// cannot later be written back on top of newly written spool data.
pub(super) fn flush_fs_before_writes(dir_path: &Path) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        if let Ok(dir) = File::open(dir_path)
            && dir_fs_type(&dir).is_some_and(is_fat_family_fs_type)
        {
            let _ = unsafe { libc::syncfs(dir.as_raw_fd()) };
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = dir_path;
    Ok(())
}

/// Clears temporary sort run files inside `work` without `rmdir`-ing `work`
/// itself, avoiding directory-cluster churn on exFAT/FAT volumes.
pub(super) fn clear_work_dir(work: &Path) -> Result<()> {
    if !work.exists() {
        fs::create_dir_all(work).map_err(|e| e.to_string())?;
        return Ok(());
    }
    let mut removed_dirs = false;
    for entry in fs::read_dir(work).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            fs::remove_dir_all(&path).map_err(|e| e.to_string())?;
            removed_dirs = true;
        } else {
            fs::remove_file(&path).map_err(|e| e.to_string())?;
        }
    }
    if removed_dirs {
        flush_fs_before_writes(work)?;
    }
    Ok(())
}

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
    handles: LruCache<usize, File>,
}
impl Sinks {
    pub fn new(root: &Path, count: usize) -> Result<Self> {
        fs::create_dir_all(root).map_err(|e| e.to_string())?;
        Ok(Self {
            root: root.into(),
            entries: (0..count).map(|_| None).collect(),
            handles: LruCache::new(OPEN_HANDLE_POOL),
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
            Self::flush_with_pool(&self.root, &mut self.handles, n, sink)?;
        }
        Ok(())
    }
    fn flush_with_pool(
        root: &Path,
        handles: &mut LruCache<usize, File>,
        n: usize,
        sink: &mut Sink,
    ) -> Result<()> {
        if sink.buffer.is_empty() {
            return Ok(());
        }
        let file = if let Some(f) = handles.get_mut(&n) {
            f
        } else {
            let f = OpenOptions::new()
                .create(true)
                .append(true)
                .open(Self::path(root, n))
                .map_err(|e| e.to_string())?;
            handles.insert(n, f);
            handles.get_mut(&n).unwrap()
        };
        file.write_all(&sink.buffer).map_err(|e| e.to_string())?;
        sink.hash.update(&sink.buffer);
        sink.len += sink.buffer.len() as u64;
        sink.buffer.clear();
        Ok(())
    }
    pub fn finish(mut self, spools: &mut [Spool]) -> Result<()> {
        let mut touched: Vec<usize> = Vec::new();
        for (n, entry) in self.entries.iter_mut().enumerate() {
            if let Some(mut sink) = entry.take() {
                Self::flush_with_pool(&self.root, &mut self.handles, n, &mut sink)?;
                spools[n].segments.push(Segment {
                    len: sink.len,
                    digest: hex(sink.hash.finish().as_ref()),
                });
                touched.push(n);
            }
        }
        // Drop pooled write handles before syncing so all file descriptors are cleanly closed.
        self.handles.clear();
        if !touched.is_empty() {
            #[cfg(target_os = "linux")]
            {
                use std::os::fd::AsRawFd;
                let dir = File::open(&self.root).map_err(|e| e.to_string())?;
                if dir_fs_type(&dir).is_some_and(is_syncfs_safe_fs_type) {
                    // Single filesystem-wide barrier replaces hundreds/thousands of per-file fsync()s
                    // on native Linux filesystems (ext4/btrfs/xfs/f2fs/tmpfs/zfs/bcachefs).
                    let rc = unsafe { libc::syncfs(dir.as_raw_fd()) };
                    if rc == 0 {
                        return Ok(());
                    }
                }
            }
            let root = &self.root;
            touched.par_iter().try_for_each(|&n| {
                File::open(Self::path(root, n))
                    .and_then(|f| f.sync_data())
                    .map_err(|e| e.to_string())
            })?;
        }
        Ok(())
    }
}

/// Single-file buffered, checksummed writer for ephemeral reconstructible sort runs.
struct RunSink {
    file: File,
    buffer: Vec<u8>,
    hash: Context,
    len: u64,
}
impl RunSink {
    fn create(path: &Path) -> Result<Self> {
        let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self {
            file,
            buffer: Vec::with_capacity(65536),
            hash: Context::new(&SHA256),
            len: 0,
        })
    }
    #[inline]
    fn push(&mut self, record: Record) -> Result<()> {
        self.buffer.extend_from_slice(&record.bytes());
        if self.buffer.len() >= 65536 {
            self.flush()?;
        }
        Ok(())
    }
    fn flush(&mut self) -> Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        self.file
            .write_all(&self.buffer)
            .map_err(|e| e.to_string())?;
        self.hash.update(&self.buffer);
        self.len += self.buffer.len() as u64;
        self.buffer.clear();
        Ok(())
    }
    fn finish(mut self) -> Result<Spool> {
        self.flush()?;
        let mut spool = Spool::default();
        if self.len > 0 {
            spool.segments.push(Segment {
                len: self.len,
                digest: hex(self.hash.finish().as_ref()),
            });
        }
        Ok(spool)
    }
}
struct Verified {
    path: PathBuf,
    file: File,
    spool: Spool,
    segment: usize,
    offset: u64,
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
                format!(
                    "Truncated join spool {} (segment {}/{} at byte {}, read {}/{})",
                    self.path.display(),
                    self.segment + 1,
                    self.spool.segments.len(),
                    self.offset,
                    self.read,
                    segment.len,
                ),
            ));
        }
        self.hash.update(&buf[..n]);
        self.read += n as u64;
        if self.read == segment.len {
            let hash = std::mem::replace(&mut self.hash, Context::new(&SHA256)).finish();
            let actual = hex(hash.as_ref());
            if actual != segment.digest {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "Join spool checksum mismatch in {} (segment {}/{} at byte {}, len {}): expected {}.., got {}..",
                        self.path.display(),
                        self.segment + 1,
                        self.spool.segments.len(),
                        self.offset,
                        segment.len,
                        &segment.digest[..segment.digest.len().min(12)],
                        &actual[..actual.len().min(12)],
                    ),
                ));
            }
            self.offset += segment.len;
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
        let actual_len = file.metadata().map_err(|e| e.to_string())?.len();
        if actual_len != spool.len() || spool.segments.iter().any(|s| s.len == 0 || s.len % 16 != 0)
        {
            return Err(format!(
                "Invalid join spool length in {} (disk {} bytes, checkpoint {} bytes)",
                path.display(),
                actual_len,
                spool.len(),
            ));
        }
        Ok(Self(Some(BufReader::with_capacity(
            1024 * 1024,
            Verified {
                path: path.to_path_buf(),
                file,
                spool: spool.clone(),
                segment: 0,
                offset: 0,
                read: 0,
                hash: Context::new(&SHA256),
            },
        ))))
    }
    #[inline]
    pub fn next(&mut self) -> Result<Option<Record>> {
        let Some(reader) = &mut self.0 else {
            return Ok(None);
        };
        let avail = reader.fill_buf().map_err(|e| e.to_string())?;
        if avail.is_empty() {
            return Ok(None);
        }
        if avail.len() >= 16 {
            let id = i64::from_le_bytes(avail[..8].try_into().unwrap());
            let value = u64::from_le_bytes(avail[8..16].try_into().unwrap());
            reader.consume(16);
            return Ok(Some(Record { id, value }));
        }
        let mut bytes = [0u8; 16];
        reader.read_exact(&mut bytes).map_err(|e| e.to_string())?;
        Ok(Some(Record {
            id: i64::from_le_bytes(bytes[..8].try_into().unwrap()),
            value: u64::from_le_bytes(bytes[8..].try_into().unwrap()),
        }))
    }
}
struct Run {
    path: PathBuf,
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
            let path = run.path.clone();
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
        let Some(mut top) = self.heap.peek_mut() else {
            return Ok(None);
        };
        let Reverse((record, i)) = *top;
        match self.readers[i]
            .next()
            .map_err(|e| sort_error(format!("{}: {e}", self.paths[i].display())))?
        {
            Some(next) => {
                if next < record {
                    return Err(sort_error(format!(
                        "{}: records are out of order",
                        self.paths[i].display()
                    )));
                }
                *top = Reverse((next, i));
            }
            None => {
                PeekMut::pop(top);
            }
        }
        Ok(Some(record))
    }
}
/// 64 MiB sort chunks, at most 32 input streams per merge. Temporary runs are
/// stored as flat files inside `work` (never creating/removing subdirectories
/// per run) and are reconstructible, never constituting durable progress.
pub(super) fn sorted(
    path: &Path,
    spool: &Spool,
    work: &Path,
    chunk: usize,
    progress: &mut dyn FnMut(u64),
) -> Result<Merge> {
    clear_work_dir(work)?;
    let mut input = Records::open(path, spool)?;
    let mut runs = Vec::new();
    let mut number = 0usize;
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
        records.par_sort_unstable();
        let run_path = Sinks::path(work, number);
        number += 1;
        let mut sink = RunSink::create(&run_path)?;
        for record in records {
            sink.push(record)?;
            bytes += 16;
        }
        let run_spool = sink.finish()?;
        runs.push(Run {
            path: run_path,
            spool: run_spool,
        });
        progress(bytes);
    }
    while runs.len() > 32 {
        let mut merged = Vec::new();
        for group in runs.chunks(32) {
            let run_path = Sinks::path(work, number);
            number += 1;
            let mut sink = RunSink::create(&run_path)?;
            let mut input = Merge::new(group)?;
            let mut merged_bytes = 0u64;
            while let Some(record) = input.next()? {
                sink.push(record)?;
                merged_bytes += 16;
                if merged_bytes.is_multiple_of(1024 * 1024) {
                    progress(bytes + merged_bytes);
                }
            }
            let run_spool = sink.finish()?;
            merged.push(Run {
                path: run_path,
                spool: run_spool,
            });
            // Drop open readers before unlinking the consumed flat run files.
            drop(input);
            for run in group {
                fs::remove_file(&run.path).map_err(|e| e.to_string())?;
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
            path: Sinks::path(&path, 0),
            spool: spools.remove(0),
        }])
        .unwrap();
        let error = merge.next().err().unwrap();
        assert!(is_sort_error(&error));
        assert!(error.contains("out of order"));
        drop(merge);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn sorted_uses_flat_run_files_without_subdirectories_and_reports_corrupt_path() {
        let root = std::env::temp_dir().join(format!(
            "1kee-flat-sort-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut sink = Sinks::new(&root.join("in"), 1).unwrap();
        for id in (0..200i64).rev() {
            sink.push(0, Record { id, value: id as u64 }).unwrap();
        }
        let mut spools = vec![Spool::default()];
        sink.finish(&mut spools).unwrap();
        let input_path = Sinks::path(&root.join("in"), 0);
        let work = root.join("sort");
        let mut merge = sorted(&input_path, &spools[0], &work, 4, &mut |_| {}).unwrap();
        for entry in fs::read_dir(&work).unwrap() {
            let entry = entry.unwrap();
            assert!(
                !entry.file_type().unwrap().is_dir(),
                "temporary sort must not create subdirectories: {}",
                entry.path().display()
            );
        }
        let mut out = Vec::new();
        while let Some(r) = merge.next().unwrap() {
            out.push(r.id);
        }
        assert_eq!(out, (0..200i64).collect::<Vec<_>>());
        drop(merge);

        // Corrupt input spool and verify diagnostic message includes path, segment, and offset.
        let mut bytes = fs::read(&input_path).unwrap();
        bytes[16] ^= 0xff;
        fs::write(&input_path, bytes).unwrap();
        let err = sorted(&input_path, &spools[0], &work, 4, &mut |_| {})
            .err()
            .unwrap();
        assert!(err.contains("checksum mismatch"), "{err}");
        assert!(err.contains("000000.bin"), "{err}");
        assert!(err.contains("segment 1/1"), "{err}");
        assert!(err.contains("at byte 0"), "{err}");

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn syncfs_safety_rejects_exfat_vfat_and_fuse() {
        assert!(is_syncfs_safe_fs_type(EXT4_SUPER_MAGIC));
        assert!(is_syncfs_safe_fs_type(BTRFS_SUPER_MAGIC));
        assert!(is_syncfs_safe_fs_type(XFS_SUPER_MAGIC));
        assert!(is_syncfs_safe_fs_type(TMPFS_MAGIC));
        assert!(!is_syncfs_safe_fs_type(EXFAT_SUPER_MAGIC));
        assert!(!is_syncfs_safe_fs_type(MSDOS_SUPER_MAGIC));
        assert!(!is_syncfs_safe_fs_type(0x6573_5546)); // FUSE
        assert!(is_fat_family_fs_type(EXFAT_SUPER_MAGIC));
        assert!(is_fat_family_fs_type(MSDOS_SUPER_MAGIC));
        assert!(!is_fat_family_fs_type(EXT4_SUPER_MAGIC));
    }
}
