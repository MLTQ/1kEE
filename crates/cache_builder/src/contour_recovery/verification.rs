//! Read scattered SQLite pages on fast storage after proving both files identical.
use super::*;
use same_file::Handle;
use std::io::{Read, Write};

const CHUNK: usize = 4 * 1024 * 1024;

pub(super) fn distinct_destination(output: &Path, fast: &Path) -> Result<()> {
    let location = |p: &Path| -> Result<PathBuf> {
        let dir = p
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(dir)?;
        Ok(dir
            .canonicalize()?
            .join(p.file_name().context("choose a database filename")?))
    };
    ensure!(
        location(output)? != location(fast)?,
        "choose a separate verification copy"
    );
    if output.try_exists()? && fast.try_exists()? {
        ensure!(
            !same_file::is_same_file(output, fast)?,
            "choose an independent verification copy"
        );
    }
    Ok(())
}

fn standalone(db: &Connection) -> Result<()> {
    let mode: String = db.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
    ensure!(
        mode.eq_ignore_ascii_case("delete"),
        "a verification copy requires a checkpointed DELETE-mode archive"
    );
    Ok(())
}

pub(super) fn check(
    path: &Path,
    db: &Connection,
    proofs: &Proofs,
    fast: Option<&Path>,
    log: &mut dyn FnMut(String),
) -> Result<()> {
    let Some(fast) = fast else {
        return verify(db, proofs, log);
    };
    standalone(db)?;
    if !fast.try_exists()? {
        let parent = fast
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        log(format!("Writing verification copy to {}", fast.display()));
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        std::io::copy(&mut fs::File::open(path)?, temp.as_file_mut())?;
        temp.flush()?;
        temp.as_file().sync_all()?;
        temp.persist_noclobber(fast)?;
    }
    ensure!(
        !same_file::is_same_file(path, fast)?,
        "choose an independent verification copy"
    );
    let reference = open_read(fast)?;
    standalone(&reference)?;
    // Both SQLite snapshots hold a read lock on their rollback-mode file.
    // Compare the complete byte stream, including pages outside the manifests,
    // overlapping reads across both devices concurrently.
    let mut archive = fs::File::open(path)?;
    let mut copy = fs::File::open(fast)?;
    let total_len = archive.metadata()?.len();
    ensure!(
        total_len == copy.metadata()?.len(),
        "verification copy has a different length"
    );
    log(format!(
        "Comparing every byte of {} with {}",
        path.display(),
        fast.display()
    ));
    std::thread::scope(|scope| -> Result<()> {
        let (tx, rx) = std::sync::mpsc::sync_channel::<std::io::Result<(Vec<u8>, usize)>>(2);
        let (recycle_tx, recycle_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(3);
        let _ = recycle_tx.send(vec![0u8; CHUNK]);
        let _ = recycle_tx.send(vec![0u8; CHUNK]);
        let reader = scope.spawn(move || {
            let mut rem = total_len;
            while rem > 0 {
                let n = rem.min(CHUNK as u64) as usize;
                let mut buf = recycle_rx.recv().unwrap_or_else(|_| vec![0u8; CHUNK]);
                match copy.read_exact(&mut buf[..n]) {
                    Ok(()) => {
                        if tx.send(Ok((buf, n))).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        break;
                    }
                }
                rem -= n as u64;
            }
        });
        let mut a = vec![0u8; CHUNK];
        let mut rem = total_len;
        let cmp_res: Result<()> = (|| {
            while rem > 0 {
                let n = rem.min(CHUNK as u64) as usize;
                archive.read_exact(&mut a[..n])?;
                let (b, bn) = rx
                    .recv()
                    .map_err(|_| anyhow!("verification reader stopped"))??;
                ensure!(
                    bn == n && a[..n] == b[..n],
                    "verification copy differs from the archive"
                );
                let _ = recycle_tx.try_send(b);
                rem -= n as u64;
            }
            Ok(())
        })();
        drop(rx);
        let _ = reader.join();
        cmp_res
    })?;
    ensure!(
        receipts(&reference)? == *proofs,
        "verification receipts differ from copied tiles"
    );
    log(format!(
        "Checking SQLite and tile geometry on {}",
        fast.display()
    ));
    verify(&reference, proofs, log)
}

pub(super) fn publish_stage(
    stage: &Path,
    output: &Path,
    fast: Option<&Path>,
    log: &mut dyn FnMut(String),
) -> Result<String> {
    ensure!(
        stage
            .file_name()
            .and_then(|p| p.to_str())
            .is_some_and(|p| p.starts_with(".contour-recovery-"))
            && stage.symlink_metadata()?.is_file(),
        "choose a staged recovery file"
    );
    let sibling = |p: &Path| {
        p.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .canonicalize()
    };
    let directory = sibling(stage)?;
    ensure!(
        directory == sibling(output)?,
        "publish the stage in its own directory"
    );
    ensure!(
        !output.try_exists()? && fast != Some(output),
        "choose an unused output path"
    );
    let identity = Handle::from_path(stage)?;
    let db = open_read(stage)?;
    standalone(&db)?;
    let proofs = receipts(&db)?;
    check(stage, &db, &proofs, fast, log)?;
    ensure!(
        Handle::from_path(stage)? == identity,
        "stage was replaced during validation"
    );
    fs::File::open(stage)?.sync_all()?;
    fs::hard_link(stage, output)?;
    #[cfg(unix)]
    fs::File::open(&directory)?.sync_all()?;
    ensure!(
        Handle::from_path(output)? == identity,
        "published file identity changed"
    );
    ensure!(
        Handle::from_path(stage)? == identity,
        "stage was replaced after publication"
    );
    fs::remove_file(stage)?;
    drop(db);
    #[cfg(unix)]
    fs::File::open(&directory)?.sync_all()?;
    Ok(format!(
        "Published {} verified tile layers to {}.",
        proofs.len(),
        output.display()
    ))
}
