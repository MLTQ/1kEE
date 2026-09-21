//! Bounded-memory, atomic publication of downloaded build rasters.
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const MAX_BYTES: u64 = 64 * 1024 * 1024;
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Partial {
    path: PathBuf,
    file: Option<fs::File>,
}
impl Drop for Partial {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

pub(super) fn save(
    mut source: impl Read,
    destination: &Path,
    total: Option<u64>,
    mut on_bytes: impl FnMut(u64, Option<u64>),
) -> io::Result<()> {
    if total.is_some_and(|n| n > MAX_BYTES) {
        return Err(io::Error::other(
            "3DEP response exceeds the raster size budget",
        ));
    }
    // Preserve the existing TIFF magic/minimum-size contract before creating
    // a partial file. The remaining response never accumulates in a Vec.
    let mut prefix = [0u8; 1025];
    source.read_exact(&mut prefix)?;
    if !super::looks_like_tiff(&prefix) {
        return Err(io::Error::other("3DEP response is not a TIFF"));
    }
    if let Some(parent) = destination.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let name = destination
        .file_name()
        .ok_or_else(|| io::Error::other("missing raster filename"))?;
    let mut partial_name = name.to_os_string();
    partial_name.push(format!(
        ".download-{}-{}.part",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let path = destination.with_file_name(partial_name);
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let mut partial = Partial {
        path,
        file: Some(output),
    };
    let output = partial.file.as_mut().unwrap();
    output.write_all(&prefix)?;
    let mut done = prefix.len() as u64;
    on_bytes(done, total);
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = source.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        done += count as u64;
        if done > MAX_BYTES {
            return Err(io::Error::other(
                "3DEP response exceeds the raster size budget",
            ));
        }
        output.write_all(&buffer[..count])?;
        on_bytes(done, total);
    }
    if total.is_some_and(|expected| expected != done) {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "incomplete 3DEP raster response",
        ));
    }
    output.flush()?;
    drop(partial.file.take());
    fs::rename(&partial.path, destination)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "1kee-raster-stream-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        path
    }
    fn tiff() -> Vec<u8> {
        let mut bytes = vec![42; 140_000];
        bytes[..4].copy_from_slice(b"II*\0");
        bytes
    }
    #[test]
    fn streams_exact_bytes_and_progress_with_known_or_unknown_length() {
        let root = root();
        let bytes = tiff();
        for total in [None, Some(bytes.len() as u64)] {
            let mut updates = Vec::new();
            let destination = root.join("tile.tif");
            save(&bytes[..], &destination, total, |done, length| {
                updates.push((done, length))
            })
            .unwrap();
            assert_eq!(fs::read(destination).unwrap(), bytes);
            assert!(updates.windows(2).all(|w| w[0].0 < w[1].0));
            assert_eq!(updates.last(), Some(&(bytes.len() as u64, total)));
        }
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn failures_preserve_existing_raster_and_remove_partial_files() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("disconnected"))
            }
        }
        let root = root();
        let destination = root.join("tile.tif");
        fs::write(&destination, b"existing raster").unwrap();
        let bytes = tiff();
        assert!(save(&bytes[..2048], &destination, Some(4096), |_, _| {}).is_err());
        assert!(
            save(
                (&bytes[..2048]).chain(Broken),
                &destination,
                None,
                |_, _| {}
            )
            .is_err()
        );
        assert!(save(&b"<html>Error</html>"[..], &destination, None, |_, _| {}).is_err());
        assert!(save(&bytes[..], &destination, Some(MAX_BYTES + 1), |_, _| {}).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"existing raster");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}
