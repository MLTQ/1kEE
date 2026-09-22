//! Disposable, validated sparse-index sidecar for an immutable node store.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const MAGIC: &[u8; 8] = b"1KNIDX01";
static NEXT: AtomicU64 = AtomicU64::new(0);
pub(super) type Stamp = [u64; 6];

pub(super) fn stamp(file: &File, records: u64) -> io::Result<Stamp> {
    let m = file.metadata()?;
    Ok([
        records,
        m.len(),
        m.mtime() as u64,
        m.mtime_nsec() as u64,
        m.dev(),
        m.ino(),
    ])
}

fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ *byte as u64).wrapping_mul(0x100_0000_01b3)
    })
}

fn decode(bytes: &[u8], expected: Stamp) -> Option<Vec<(i64, u64)>> {
    let count = usize::try_from(expected[0].div_ceil(super::INDEX_STRIDE)).ok()?;
    if bytes.len() != count.checked_mul(16)?.checked_add(64)? || bytes.get(..8)? != MAGIC {
        return None;
    }
    let word = |offset| u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
    for (i, value) in expected.into_iter().enumerate() {
        if word(8 + i * 8) != value {
            return None;
        }
    }
    if checksum(&bytes[..bytes.len() - 8]) != word(bytes.len() - 8) {
        return None;
    }
    let mut index = Vec::with_capacity(count);
    for i in 0..count {
        let id = word(56 + i * 16) as i64;
        let record = word(64 + i * 16);
        if record != i as u64 * super::INDEX_STRIDE
            || index.last().is_some_and(|&(last, _)| last > id)
        {
            return None;
        }
        index.push((id, record));
    }
    Some(index)
}

pub(super) fn load(path: &Path, expected: Stamp) -> Option<Vec<(i64, u64)>> {
    let length = expected[0]
        .div_ceil(super::INDEX_STRIDE)
        .checked_mul(16)?
        .checked_add(64)?;
    if fs::metadata(path).ok()?.len() != length {
        return None;
    }
    decode(&fs::read(path).ok()?, expected)
}

struct Partial(PathBuf);
impl Drop for Partial {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub(super) fn save(path: &Path, key: Stamp, index: &[(i64, u64)]) -> io::Result<()> {
    let mut bytes = Vec::with_capacity(64 + index.len() * 16);
    bytes.extend_from_slice(MAGIC);
    for word in key {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    for &(id, record) in index {
        bytes.extend_from_slice(&id.to_le_bytes());
        bytes.extend_from_slice(&record.to_le_bytes());
    }
    bytes.extend_from_slice(&checksum(&bytes).to_le_bytes());
    let suffix = format!(
        "part-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let temporary = path.with_extension(suffix);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let partial = Partial(temporary);
    file.write_all(&bytes)?;
    file.sync_data()?;
    drop(file);
    fs::rename(&partial.0, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_rejects_changed_sources_corruption_and_truncation() {
        let root = std::env::temp_dir().join(format!(
            "1kee-index-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let path = root.join("nodes.index");
        let key = [4097, 4097 * 16, 100, 200, 1, 2];
        let entries = vec![(10, 0), (9000, 4096)];
        save(&path, key, &entries).unwrap();
        assert_eq!(load(&path, key), Some(entries));
        for field in 0..key.len() {
            let mut changed = key;
            changed[field] += 1;
            assert!(load(&path, changed).is_none());
        }
        let mut bytes = fs::read(&path).unwrap();
        bytes[56] ^= 1;
        assert!(decode(&bytes, key).is_none());
        assert!(decode(&bytes[..bytes.len() - 1], key).is_none());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}
