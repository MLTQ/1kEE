//! Merge native Earth contour databases into a new, independently checked SQLite file.
mod layer;
mod retire;
#[cfg(test)]
mod tests;
use anyhow::{Context, Result, anyhow, ensure};
use layer::{Hash, Key, LAYERS, Layer};
use rusqlite::{Connection, OpenFlags, params};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Instant,
};
use tile_archive::contour_grid::SRTM_NATIVE_DB_NAME as NATIVE;

#[derive(Debug, Clone, Default)]
pub(crate) struct Command {
    pub caches: Vec<PathBuf>,
    pub repairs: Vec<PathBuf>,
    pub output: PathBuf,
    pub remove_inputs: bool,
    pub cleanup_only: bool,
}

pub(crate) fn parse(args: impl Iterator<Item = String>) -> std::result::Result<Command, String> {
    let mut it = args;
    let mut cmd = Command::default();
    while let Some(flag) = it.next() {
        if flag == "--remove-inputs" {
            cmd.remove_inputs = true;
            continue;
        }
        if flag == "--cleanup-only" {
            cmd.cleanup_only = true;
            continue;
        }
        let val = it
            .next()
            .ok_or_else(|| format!("Expected a path after {flag}"))?;
        match flag.as_str() {
            "--cache" => cmd.caches.push(val.into()),
            "--repairs" => cmd.repairs.push(val.into()),
            "--output" => cmd.output = val.into(),
            _ => return Err(format!("Unknown recovery option {flag}")),
        }
    }
    if cmd.output.as_os_str().is_empty() || (cmd.caches.is_empty() && cmd.repairs.is_empty()) {
        return Err("Usage: recover-contours --cache FILE and/or --repairs DIRECTORY --output FILE [--remove-inputs | --cleanup-only]".into());
    }
    Ok(cmd)
}

#[derive(Clone)]
struct Input {
    path: PathBuf,
    repair: Option<Key>,
}
fn tile_name(path: &Path) -> Option<Key> {
    let nums: Vec<_> = path
        .file_name()?
        .to_str()?
        .split('_')
        .map(str::parse::<i32>)
        .collect::<std::result::Result<_, _>>()
        .ok()?;
    nums.try_into().ok()
}
fn collect_repairs(dir: &Path, list: &mut Vec<Input>) -> Result<()> {
    // Recognize both sibling .tile-repairs and isolated, path-hashed namespaces.
    // A directory marker without a database contributes no cache data.
    if let Some(key) = tile_name(dir) {
        let db = dir.join(NATIVE);
        if db.symlink_metadata().is_ok_and(|m| m.is_file()) {
            list.push(Input {
                path: db,
                repair: Some(key),
            });
        }
        return Ok(());
    }
    for e in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let e = e?;
        if !e.file_type()?.is_dir() {
            continue;
        }
        let p = e.path();
        if tile_name(&p).is_some()
            || e.file_name() == NATIVE
            || e.file_name()
                .to_str()
                .is_some_and(|s| s.len() == 16 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            collect_repairs(&p, list)?;
        }
    }
    Ok(())
}
fn inputs(cmd: &Command) -> Result<Vec<Input>> {
    let mut result = Vec::new();
    for root in &cmd.repairs {
        if cmd.cleanup_only && !root.try_exists()? {
            continue;
        }
        let mut found = Vec::new();
        collect_repairs(&root.canonicalize()?, &mut found)?;
        found.sort_by(|a, b| a.path.cmp(&b.path));
        result.extend(found);
    }
    for path in &cmd.caches {
        if cmd.cleanup_only && !path.try_exists()? {
            continue;
        }
        ensure!(
            !path.symlink_metadata()?.is_symlink(),
            "choose the actual cache file instead of a link: {}",
            path.display()
        );
        result.push(Input {
            path: path.canonicalize()?,
            repair: None,
        })
    }
    let mut seen = std::collections::HashSet::new();
    result.retain(|s| seen.insert(s.path.clone()));
    ensure!(
        cmd.cleanup_only || !result.is_empty(),
        "no native cache files found"
    );
    for src in &result {
        if cmd.output.exists() {
            ensure!(
                !same_file::is_same_file(&cmd.output, &src.path)?,
                "output is also an input"
            )
        }
    }
    Ok(result)
}

#[derive(Clone, PartialEq, Eq)]
struct Proof {
    count: i64,
    hash: Hash,
    repair: bool,
    built: i64,
}
type Proofs = BTreeMap<(Layer, Key), Proof>;
fn open_read(path: &Path) -> Result<Connection> {
    let src = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    src.execute_batch("PRAGMA query_only=ON; PRAGMA cache_size=-32768; BEGIN")?;
    ensure!(
        tile_archive::native_contours::is_native(&src)?,
        "missing native SRTM provenance"
    );
    Ok(src)
}
fn keep_error(e: &anyhow::Error) -> bool {
    // Destination I/O, space and lock failures abort the run with its inputs intact.
    match e
        .downcast_ref::<rusqlite::Error>()
        .and_then(|e| e.sqlite_error_code())
    {
        Some(c) => matches!(
            c,
            rusqlite::ErrorCode::DatabaseCorrupt
                | rusqlite::ErrorCode::NotADatabase
                | rusqlite::ErrorCode::ConstraintViolation
        ),
        None => true,
    }
}
fn ingest(
    src: &Input,
    dst: &mut Connection,
    proofs: &mut Proofs,
    log: &mut dyn FnMut(String),
) -> Result<(usize, bool)> {
    let db = open_read(&src.path)?;
    let mut partial = false;
    let mut copied = 0;
    let mut last = Instant::now();
    for layer in LAYERS {
        let (tiles, complete) = match layer::inventory(&db, layer) {
            Ok(inventory) => inventory,
            Err(e) if keep_error(&e) => {
                partial = true;
                log(format!(
                    "Unreadable {} in {}: {e}",
                    layer.manifest(),
                    src.path.display()
                ));
                continue;
            }
            Err(e) => return Err(e),
        };
        partial |= !complete;
        for (n, t) in tiles.iter().enumerate() {
            if let Some(key) = src.repair {
                ensure!(t.key == key, "repair namespace disagrees with its manifest")
            }
            let slot = (layer, t.key);
            let previous = proofs.get(&slot);
            let repair = src.repair.is_some();
            if previous.is_some_and(|p| (p.repair, p.built) > (repair, t.built)) {
                continue;
            }
            match layer::copy_tile(&db, dst, layer, t, previous.is_some(), complete) {
                Ok(hash) => {
                    proofs.insert(
                        slot,
                        Proof {
                            count: t.count,
                            hash,
                            repair,
                            built: t.built,
                        },
                    );
                    copied += 1;
                }
                Err(e) if keep_error(&e) => {
                    partial = true;
                }
                Err(e) => return Err(e),
            }
            if last.elapsed().as_secs() >= 10 {
                log(format!(
                    "{}: {}/{} {} manifests",
                    src.path.display(),
                    n + 1,
                    tiles.len(),
                    layer.table()
                ));
                last = Instant::now();
            }
        }
    }
    Ok((copied, partial))
}
fn verify(db: &Connection, proofs: &Proofs, log: &mut dyn FnMut(String)) -> Result<()> {
    ensure!(
        tile_archive::native_contours::is_native(db)?,
        "output lost native provenance"
    );
    let check: String = db.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    ensure!(check == "ok", "SQLite validation failed: {check}");
    let mut verified = 0;
    let mut last = Instant::now();
    for layer in LAYERS {
        let (tiles, complete) = layer::inventory(db, layer)?;
        ensure!(complete, "damaged output manifest");
        let mut expected_total = 0i64;
        for t in tiles {
            let p = proofs
                .get(&(layer, t.key))
                .ok_or_else(|| anyhow!("unexpected output tile {:?}", t.key))?;
            ensure!(
                t.count == p.count
                    && t.built == p.built
                    && layer::transfer(db, None, layer, &t, false)? == p.hash,
                "output verification failed for {:?}",
                t.key
            );
            expected_total += t.count;
            verified += 1;
            if last.elapsed().as_secs() >= 10 {
                log(format!("Verified {verified}/{} tile layers", proofs.len()));
                last = Instant::now();
            }
        }
        let total: i64 = db.query_row(
            &format!("SELECT COUNT(*) FROM {} NOT INDEXED", layer.table()),
            [],
            |r| r.get(0),
        )?;
        ensure!(total == expected_total, "orphan output geometry");
    }
    ensure!(verified == proofs.len(), "missing output manifest rows");
    Ok(())
}

const RECEIPTS: &str = "cache_recovery_digests";
fn receipts(db: &Connection) -> Result<Proofs> {
    let mut result = Proofs::new();
    let mut q =
        db.prepare("SELECT layer,z,lat,lon,n,digest,repair,built FROM cache_recovery_digests")?;
    let mut rows = q.query([])?;
    while let Some(r) = rows.next()? {
        let layer = match r.get::<_, i64>(0)? {
            0 => Layer::Contour,
            1 => Layer::Coast,
            _ => anyhow::bail!("invalid receipt layer"),
        };
        let value = r.get_ref(5)?.as_blob()?;
        result.insert(
            (layer, [r.get(1)?, r.get(2)?, r.get(3)?]),
            Proof {
                count: r.get(4)?,
                hash: value.try_into()?,
                repair: r.get(6)?,
                built: r.get(7)?,
            },
        );
    }
    ensure!(!result.is_empty(), "no verified cache receipts");
    Ok(result)
}

pub(crate) fn run(cmd: &Command, log: &mut dyn FnMut(String)) -> Result<String> {
    let files = inputs(cmd)?;
    if cmd.cleanup_only {
        return retire::run(&cmd.output, &files, log);
    }
    ensure!(
        !cmd.output.try_exists()?,
        "output already exists: {}",
        cmd.output.display()
    );
    let parent = cmd
        .output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let stage = tempfile::Builder::new()
        .prefix(".contour-recovery-")
        .tempfile_in(parent)?;
    Connection::open(stage.path())?.pragma_update(None, "page_size", 16384)?;
    let mut dst = crate::contours::open_cache_db(stage.path())?;
    dst.execute_batch("PRAGMA journal_mode=MEMORY; PRAGMA synchronous=OFF; PRAGMA cache_size=-196608; PRAGMA temp_store=MEMORY;")?;
    tile_archive::native_contours::initialize(&dst)?;
    let mut proofs = Proofs::new();
    let mut partial = 0;
    let mut last = Instant::now();
    log(format!("Reading {} source databases", files.len()));
    for (i, src) in files.iter().enumerate() {
        match ingest(src, &mut dst, &mut proofs, log) {
            Ok((_, incomplete)) => {
                if incomplete {
                    partial += 1;
                    log(format!("Partial source retained: {}", src.path.display()));
                }
            }
            Err(e) if keep_error(&e) => {
                partial += 1;
                log(format!("Unreadable source {}: {e}", src.path.display()));
            }
            Err(e) => return Err(e.context(format!("merge {}", src.path.display()))),
        }
        if last.elapsed().as_secs() >= 10 {
            log(format!(
                "Read {}/{} databases, {} tile layers",
                i + 1,
                files.len(),
                proofs.len()
            ));
            last = Instant::now();
        }
    }
    ensure!(!proofs.is_empty(), "no valid native tiles recovered");
    let tx = dst.transaction()?;
    tx.execute_batch("CREATE TABLE cache_recovery_digests(layer INTEGER,z INTEGER,lat INTEGER,lon INTEGER,n INTEGER,digest BLOB,repair INTEGER,built INTEGER,PRIMARY KEY(layer,z,lat,lon)) WITHOUT ROWID")?;
    {
        let mut s = tx.prepare("INSERT INTO cache_recovery_digests VALUES(?,?,?,?,?,?,?,?)")?;
        for ((layer, k), p) in &proofs {
            s.execute(params![
                *layer as i32,
                k[0],
                k[1],
                k[2],
                p.count,
                &p.hash[..],
                p.repair,
                p.built
            ])?;
        }
    }
    tx.commit()?;
    dst.execute_batch("PRAGMA journal_mode=DELETE")?;
    drop(dst);
    stage.as_file().sync_all()?;
    log(format!("Validating {} recovered tile layers", proofs.len()));
    {
        let reopened = open_read(stage.path())?;
        ensure!(
            receipts(&reopened)? == proofs,
            "published receipts differ from copied tiles"
        );
        verify(&reopened, &proofs, log)?;
    }
    stage.persist_noclobber(&cmd.output)?;
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    let summary = format!(
        "Published {} verified tile layers to {}. {} partial or unreadable inputs.",
        proofs.len(),
        cmd.output.display(),
        partial
    );
    if cmd.remove_inputs {
        log(summary.clone());
        return retire::run(&cmd.output, &files, log);
    }
    Ok(summary)
}
