//! Stable resume evidence, separate from strict live-file identity checks.
use ring::digest::{Context, SHA256};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    os::unix::fs::{FileExt, MetadataExt},
    path::Path,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct Stamp {
    pub len: u64,
    pub device: u64,
    pub inode: u64,
    pub seconds: i64,
    pub nanos: i64,
}
impl Stamp {
    pub fn read(path: &Path) -> Result<Self, String> {
        let m = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self::from_metadata(&m))
    }
    fn from_metadata(m: &std::fs::Metadata) -> Self {
        Self {
            len: m.len(),
            device: m.dev(),
            inode: m.ino(),
            seconds: m.mtime(),
            nanos: m.mtime_nsec(),
        }
    }
    /// Device numbers identify a live mount, not a durable filesystem. Keep all
    /// other identity fields strict, and require content evidence before reuse.
    pub fn same_file_metadata(&self, other: &Self) -> bool {
        self.len == other.len
            && self.inode == other.inode
            && self.seconds == other.seconds
            && self.nanos == other.nanos
    }
    pub fn mismatch(&self, current: &Self) -> String {
        let mut differences = Vec::new();
        if self.len != current.len {
            differences.push(format!("size {} → {} bytes", self.len, current.len));
        }
        if self.inode != current.inode {
            differences.push(format!("file identity {} → {}", self.inode, current.inode));
        }
        if (self.seconds, self.nanos) != (current.seconds, current.nanos) {
            differences.push("modification time changed".into());
        }
        format!(
            "The PBF source differs from this compact index ({}). Choose a new working directory; existing build files were preserved",
            differences.join(", ")
        )
    }
}

/// Versioned SHA-256 of at most 64 evenly spaced 64 KiB windows, including both
/// ends. Supplements metadata without reading the entire planet on every launch.
/// This is a sampled identity check, not a whole-file integrity checksum.
pub(super) fn fingerprint(path: &Path, expected: &Stamp) -> Result<String, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let validate = || -> Result<(), String> {
        let m = file.metadata().map_err(|e| e.to_string())?;
        if Stamp::from_metadata(&m) != *expected || Stamp::read(path)? != *expected {
            return Err(
                "PBF changed while verifying source identity; existing build files were preserved"
                    .into(),
            );
        }
        Ok(())
    };
    validate()?;
    let length = expected.len.min(64 * 1024) as usize;
    let last = expected.len - length as u64;
    let count = if last == 0 { 1 } else { 64 };
    let mut digest = Context::new(&SHA256);
    digest.update(b"1kee-pbf-sample-v1");
    digest.update(&expected.len.to_le_bytes());
    let mut buffer = vec![0; length];
    for i in 0..count {
        let offset = (last as u128 * i as u128 / (count - 1).max(1) as u128) as u64;
        file.read_exact_at(&mut buffer, offset)
            .map_err(|e| format!("Verifying PBF at byte {offset}: {e}"))?;
        digest.update(&offset.to_le_bytes());
        digest.update(&buffer);
    }
    validate()?;
    let hex: String = digest
        .finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(format!("sample-v1:{hex}"))
}
