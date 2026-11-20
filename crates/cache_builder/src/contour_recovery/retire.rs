use super::*;
use rusqlite::TransactionBehavior;
use same_file::Handle;
use std::time::Duration;

pub(super) fn run(output: &Path, inputs: &[Input], log: &mut dyn FnMut(String)) -> Result<String> {
    let published = Handle::from_path(output)?;
    let mut db = Connection::open_with_flags(output, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    db.busy_timeout(Duration::from_secs(2))?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let proofs = receipts(&tx)?;
    log(format!(
        "Checking the published cache before input cleanup: {}",
        output.display()
    ));
    verify(&tx, &proofs, log)?;
    let mut removed = 0;
    let mut retained = 0;
    let mut last = Instant::now();
    for source in inputs {
        ensure!(
            Handle::from_path(output)? == published,
            "published cache was replaced during cleanup"
        );
        match remove_verified(source, &proofs, &published) {
            Ok(()) => removed += 1,
            Err(e) => {
                retained += 1;
                log(format!("Retained {}: {e}", source.path.display()));
            }
        }
        if last.elapsed().as_secs() >= 10 {
            log(format!("Retired {removed} inputs, retained {retained}"));
            last = Instant::now();
        }
    }
    tx.commit()?;
    Ok(format!(
        "Verified {}. Retired {removed} input databases, retained {retained}.",
        output.display()
    ))
}

fn remove_verified(source: &Input, proofs: &Proofs, output: &Handle) -> Result<()> {
    let identity = Handle::from_path(&source.path)?;
    ensure!(&identity != output, "input is the published output");
    // Fail closed before opening an unreadable or non-native source for writing.
    {
        let check = open_read(&source.path)?;
        ensure!(
            check.query_row("PRAGMA quick_check(1)", [], |r| r.get::<_, String>(0))? == "ok",
            "damaged source"
        );
    }
    let src = Connection::open_with_flags(&source.path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    src.busy_timeout(Duration::ZERO)?;
    src.execute_batch("PRAGMA locking_mode=EXCLUSIVE; BEGIN EXCLUSIVE")?;
    ensure!(
        tile_archive::native_contours::is_native(&src)?,
        "different cache format"
    );
    let mut schema =
        src.prepare("SELECT name FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*'")?;
    for item in schema.query_map([], |r| r.get::<_, String>(0))? {
        let name = item?;
        ensure!(
            matches!(
                name.as_str(),
                "contour_tiles"
                    | "coastline_tiles"
                    | "contour_tile_manifest"
                    | "coastline_tile_manifest"
                    | "contour_source_metadata"
                    | "idx_contour_tiles_lookup"
                    | "idx_coastline_tiles_lookup"
            ) || name == RECEIPTS,
            "unaccounted schema object {name}"
        );
    }
    drop(schema);
    for (table, expected) in [
        (
            "contour_tile_manifest",
            "zoom_bucket,lat_bucket,lon_bucket,contour_count,built_at",
        ),
        (
            "coastline_tile_manifest",
            "zoom_bucket,lat_bucket,lon_bucket,line_count,built_at",
        ),
        (
            "contour_tiles",
            "zoom_bucket,lat_bucket,lon_bucket,fid,elevation_m,geom",
        ),
        (
            "coastline_tiles",
            "zoom_bucket,lat_bucket,lon_bucket,fid,geom",
        ),
        ("contour_source_metadata", "key,value"),
    ] {
        let columns: String = src.query_row(
            "SELECT group_concat(name, ',') FROM (SELECT name FROM pragma_table_xinfo(?1) ORDER BY cid)",
            [table], |row|row.get(0))?;
        ensure!(columns == expected, "unaccounted columns in {table}");
    }
    if src.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name=?1)",
        [RECEIPTS],
        |r| r.get::<_, bool>(0),
    )? {
        let prior = receipts(&src)?;
        for (key, previous) in &prior {
            let current = proofs
                .get(key)
                .ok_or_else(|| anyhow!("missing prior receipt"))?;
            ensure!(
                previous.hash == current.hash && previous.count == current.count,
                "prior recovery receipt differs"
            );
        }
    }
    ensure!(
        src.query_row("SELECT count(*) FROM contour_source_metadata", [], |r| r
            .get::<_, i64>(0))?
            == 1,
        "additional source metadata"
    );
    let mut layers = 0;
    for layer in LAYERS {
        let (tiles, complete) = layer::inventory(&src, layer)?;
        ensure!(complete, "unreadable manifest");
        let mut total = 0i64;
        for t in tiles {
            if let Some(key) = source.repair {
                ensure!(t.key == key, "unexpected additional tile")
            }
            let p = proofs
                .get(&(layer, t.key))
                .ok_or_else(|| anyhow!("tile {:?} absent from output", t.key))?;
            ensure!(
                p.count == t.count
                    && p.built >= t.built
                    && layer::transfer(&src, None, layer, &t, false)? == p.hash,
                "tile {:?} differs from the output",
                t.key
            );
            total += t.count;
            layers += 1;
        }
        ensure!(
            src.query_row(
                &format!("SELECT COUNT(*) FROM {} NOT INDEXED", layer.table()),
                [],
                |r| r.get::<_, i64>(0)
            )? == total,
            "unaccounted geometry"
        );
    }
    ensure!(layers > 0, "no tile manifests");
    // Exclusive locking mode keeps the writer lock after COMMIT. Leave SQLite
    // to checkpoint/remove its own sidecars before unlinking the exact main file.
    src.execute_batch("COMMIT")?;
    let mode: String = src.query_row("PRAGMA journal_mode=DELETE", [], |r| r.get(0))?;
    ensure!(
        mode.eq_ignore_ascii_case("delete"),
        "source is still in WAL mode"
    );
    src.execute_batch("BEGIN EXCLUSIVE")?;
    ensure!(
        Handle::from_path(&source.path)? == identity,
        "source file changed during cleanup"
    );
    fs::remove_file(&source.path)?;
    drop(src);
    if source.repair.is_some()
        && let Some(dir) = source.path.parent()
    {
        let _ = fs::remove_dir(dir);
    }
    Ok(())
}
