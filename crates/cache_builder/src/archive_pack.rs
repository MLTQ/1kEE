use crate::archive_space::{SpaceMonitor, gb};
use std::fs;
use std::path::{Path, PathBuf};
use tile_archive::{Key, Writer, contours, vector};

#[derive(Debug)]
pub struct Command {
    pub out: PathBuf,
    pub osm: Option<PathBuf>,
    pub pipelines: Option<PathBuf>,
    pub platforms: Option<PathBuf>,
    pub terrain: Vec<(i32, PathBuf)>,
}

pub fn parse(args: impl Iterator<Item = String>) -> Result<Command, String> {
    let mut out = None;
    let mut osm = None;
    let mut pipelines = None;
    let mut platforms = None;
    let mut terrain = Vec::new();
    let mut args = args;
    while let Some(flag) = args.next() {
        let value = PathBuf::from(
            args.next()
                .ok_or_else(|| format!("Missing value for {flag}"))?,
        );
        match flag.as_str() {
            "--out" => out = Some(value),
            "--osm-cache-dir" => osm = Some(value),
            "--pipelines" => pipelines = Some(value),
            "--platforms" => platforms = Some(value),
            "--earth-contours" => terrain.push((0, value)),
            "--moon-contours" => terrain.push((1, value)),
            "--mars-contours" => terrain.push((2, value)),
            _ => return Err(format!("Unknown archive option {flag}")),
        }
    }
    if osm.is_none() && pipelines.is_none() && platforms.is_none() && terrain.is_empty() {
        return Err(
            "Specify --osm-cache-dir, --pipelines, --platforms and/or --earth-contours, --moon-contours, --mars-contours"
                .into(),
        );
    }
    let mut bodies = std::collections::HashSet::new();
    if terrain.iter().any(|(body, _)| !bodies.insert(*body)) {
        return Err("Duplicate contour body".into());
    }
    Ok(Command {
        out: out.ok_or("Missing --out")?,
        osm,
        pipelines,
        platforms,
        terrain,
    })
}

struct Staging(PathBuf);
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
        let _ = fs::remove_file(self.0.with_file_name(format!(
            "{}-journal",
            self.0.file_name().unwrap().to_string_lossy()
        )));
    }
}

pub fn run(cmd: Command) -> Result<(), String> {
    run_with_progress(cmd, &mut |message| println!("{message}"))
        .map(|summary| println!("{summary}"))
}

pub fn run_with_progress(cmd: Command, progress: &mut dyn FnMut(String)) -> Result<String, String> {
    if cmd.out.exists() {
        return Err(format!("Destination already exists: {}", cmd.out.display()));
    }
    let parent = cmd
        .out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let stage_path = parent.join(format!(".1kee-archive-{}-{nonce}.part", std::process::id()));
    let mut space = SpaceMonitor::new(cmd.out.clone(), stage_path.clone());
    space.check(progress).map_err(|e| space.failure(&e))?;
    let mut writer = Writer::create(&stage_path).map_err(|e| space.failure(&e))?;
    let stage = Staging(stage_path);
    let packed = (|| {
        writer.metadata("created_unix_ns", &nonce.to_string())?;
        writer.metadata("vector_grid", "Earth, eighth-degree, full detail, level 0")?;
        writer.metadata(
            "coverage",
            "Imported cache snapshot only; missing tiles/layers are unknown, not empty",
        )?;
        let mut count = 0;
        if let Some(osm) = &cmd.osm {
            count += pack_vectors(&mut writer, osm, progress, &mut space)?;
        }
        let pipelines = cmd.pipelines.clone().or_else(|| {
            let path = cmd.osm.as_ref()?.parent()?.join(tile_archive::pipelines::FILE_NAME);
            path.is_file().then_some(path)
        });
        if let Some(path) = pipelines {
            space.check(progress)?;
            progress(format!("Including public pipelines from {}", path.display()));
            let reader = tile_archive::Reader::open(&path)?;
            count += tile_archive::pipelines::copy_into(&reader, &mut writer)?;
        }
        let platforms = cmd.platforms.clone().or_else(|| {
            let path = cmd.osm.as_ref()?.parent()?.join(tile_archive::platforms::FILE_NAME);
            path.is_file().then_some(path)
        });
        if let Some(path) = platforms {
            space.check(progress)?;
            progress(format!("Including offshore platforms from {}", path.display()));
            count += tile_archive::platforms::copy_into(&tile_archive::Reader::open(&path)?, &mut writer)?;
        }
        for (body, path) in &cmd.terrain {
            count += pack_contours(&mut writer, *body, path, progress, &mut space)?;
        }
        if count == 0 {
            return Err("No source cells/tiles found; archive was not published".into());
        }
        space.check(progress)?;
        writer.finish()?;
        Ok(count)
    })();
    let count = packed.map_err(|e: String| space.failure(&e))?;
    fs::hard_link(&stage.0, &cmd.out).map_err(|e| {
        space.failure(&format!(
            "Cannot publish archive without overwriting {}: {e}",
            cmd.out.display()
        ))
    })?;
    Ok(format!(
        "Published {} source cells/tiles to {} ({} bytes)",
        count,
        cmd.out.display(),
        fs::metadata(&cmd.out).map_err(|e| e.to_string())?.len()
    ))
}

const LAYERS: &[(&str, [u8; 4])] = &[
    ("road", *b"ROAD"),
    ("waterway", *b"WATR"),
    ("building", *b"BLDG"),
    ("tree", *b"TREE"),
    ("power", *b"POWR"),
    ("railway", *b"RAIL"),
    ("pipeline", *b"PIPE"),
    ("aeroway", *b"AERO"),
    ("military", *b"MILT"),
    ("comm", *b"COMM"),
    ("industrial", *b"INDS"),
    ("port", *b"PORT"),
    ("government", *b"GOVT"),
    ("surveillance", *b"SURV"),
];

fn pack_vectors(
    writer: &mut Writer,
    root: &Path,
    progress: &mut dyn FnMut(String),
    space: &mut SpaceMonitor,
) -> Result<usize, String> {
    if !root.is_dir() {
        return Err(format!("Missing OSM cache directory {}", root.display()));
    }
    let mut sources = Vec::new();
    let mut source_bytes = 0u64;
    for &(prefix, tag) in LAYERS {
        let directory = root.join(format!("{prefix}_cells"));
        if !directory.exists() {
            continue;
        }
        let mut paths = fs::read_dir(directory)
            .map_err(|e| e.to_string())?
            .map(|e| e.map(|e| e.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        paths.sort();
        for path in paths {
            if path.extension().is_none_or(|ext| ext != "1kc") {
                continue;
            }
            source_bytes = source_bytes.saturating_add(
                fs::metadata(&path)
                    .map_err(|e| format!("{}: {e}", path.display()))?
                    .len(),
            );
            sources.push((prefix, tag, path));
        }
    }
    progress(format!(
        "Vector input: {} cells, {} before tiling. The additional archive can be larger because features repeat across tile boundaries and the index takes space. Original cells are kept.",
        sources.len(),
        gb(source_bytes)
    ));
    let mut count = 0;
    use rayon::prelude::*;
    for batch in sources.chunks(16) {
        space.check(progress)?;
        let encoded: Result<Vec<_>, String> = batch
            .par_iter()
            .map(|&(prefix, tag, ref path)| {
                let before = contours::fingerprint(path)?;
                if fs::metadata(path).map_err(|e| e.to_string())?.len()
                    > tile_archive::MAX_PAYLOAD as u64
                {
                    return Err(format!("Source cell exceeds 256 MiB: {}", path.display()));
                }
                let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
                let features = cell_format::read::read_single_chunk(&bytes, tag)
                    .ok_or_else(|| format!("Invalid {} cell {}", prefix, path.display()))?;
                let lat = i16::from_le_bytes(bytes[5..7].try_into().unwrap()) as i32;
                let lon = i16::from_le_bytes(bytes[7..9].try_into().unwrap()) as i32;
                if path.file_name().unwrap()
                    != cell_format::cell_filename(prefix, lat, lon).as_str()
                {
                    return Err(format!("Cell filename/header mismatch: {}", path.display()));
                }
                let (tiles, packed) = vector::encode_cell(tag, lat, lon, &features)
                    .map_err(|e| format!("Cannot pack {}: {e}", path.display()))?;
                if before != contours::fingerprint(path)? {
                    return Err(format!(
                        "Source cell changed during packing: {}",
                        path.display()
                    ));
                }
                let meta_key = format!("vector:{}:{lat}:{lon}", String::from_utf8_lossy(&tag));
                Ok((prefix, tag, lat, lon, features.len(), packed, tiles, meta_key, before))
            })
            .collect();
        for (prefix, tag, lat, lon, feature_count, packed, tiles, meta_key, before) in encoded? {
            writer.put_batch_with_meta(&tiles, Some((tag, lat, lon)), Some((&meta_key, &before)))?;
            count += 1;
            progress(format!(
                "Packed {prefix} ({lat},{lon}): {feature_count} features, {packed} bytes"
            ));
        }
    }
    Ok(count)
}

fn pack_contours(
    writer: &mut Writer,
    body: i32,
    path: &Path,
    progress: &mut dyn FnMut(String),
    space: &mut SpaceMonitor,
) -> Result<usize, String> {
    use rusqlite::{Connection, OpenFlags};
    let before = contours::fingerprint(path)?;
    let source = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    let _ = source.pragma_update(None, "cache_size", -131072);
    let _ = source.pragma_update(None, "mmap_size", 268_435_456i64);
    source.execute_batch("BEGIN;").map_err(|e| e.to_string())?;
    let mut manifest_stmt = source
        .prepare(
            "SELECT zoom_bucket,lat_bucket,lon_bucket,contour_count \
             FROM contour_tile_manifest ORDER BY zoom_bucket,lat_bucket,lon_bucket",
        )
        .map_err(|e| e.to_string())?;
    let manifest: Vec<(i32, i32, i32, usize)> = manifest_stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    drop(manifest_stmt);

    // Single ordered scan of `contour_tiles` merge-joined against the manifest
    // instead of N+1 indexed queries, streaming WKB directly into CTF1 bytes.
    let mut geom_stmt = source
        .prepare(
            "SELECT zoom_bucket,lat_bucket,lon_bucket,elevation_m,geom \
             FROM contour_tiles ORDER BY zoom_bucket,lat_bucket,lon_bucket,fid",
        )
        .map_err(|e| e.to_string())?;
    let mut geom_rows = geom_stmt.query([]).map_err(|e| e.to_string())?;

    let mut pending_batch: Vec<(Key, Vec<u8>)> = Vec::with_capacity(256);
    let mut pending_bytes = 0usize;
    let mut count = 0usize;

    // Lookahead buffer holds at most 1 row across tile boundaries so all rows
    // inside a tile stream directly from SQLite `as_blob()` with zero copy.
    let mut carry: Option<((i32, i32, i32), f32, Vec<u8>)> = None;

    for (level, y, x, expected) in manifest {
        space.check(progress)?;
        let target_key = (level, y, x);
        let mut tile_bytes = Vec::with_capacity(8 + expected * 32);
        tile_bytes.extend_from_slice(b"CTF1");
        tile_bytes.extend_from_slice(&(expected as u32).to_le_bytes());
        let mut seen = 0usize;
        let mut raw_size = 8usize;

        if let Some((rk, elev, blob)) = carry.take() {
            match rk.cmp(&target_key) {
                std::cmp::Ordering::Less => {}
                std::cmp::Ordering::Equal => {
                    raw_size = raw_size.saturating_add(8).saturating_add(blob.len());
                    if raw_size > tile_archive::MAX_PAYLOAD {
                        return Err("Contour tile exceeds 256 MiB".into());
                    }
                    contours::append_gpkg_row(elev, &blob, &mut tile_bytes)?;
                    seen += 1;
                }
                std::cmp::Ordering::Greater => {
                    carry = Some((rk, elev, blob));
                }
            }
        }

        if carry.is_none() {
            while let Some(row) = geom_rows.next().map_err(|e| e.to_string())? {
                let rk = (
                    row.get::<_, i32>(0).map_err(|e| e.to_string())?,
                    row.get::<_, i32>(1).map_err(|e| e.to_string())?,
                    row.get::<_, i32>(2).map_err(|e| e.to_string())?,
                );
                match rk.cmp(&target_key) {
                    std::cmp::Ordering::Less => continue,
                    std::cmp::Ordering::Equal => {
                        let elevation: f32 = row.get(3).map_err(|e| e.to_string())?;
                        let blob = row
                            .get_ref(4)
                            .map_err(|e| e.to_string())?
                            .as_blob()
                            .map_err(|e| e.to_string())?;
                        raw_size = raw_size.saturating_add(8).saturating_add(blob.len());
                        if raw_size > tile_archive::MAX_PAYLOAD {
                            return Err("Contour tile exceeds 256 MiB".into());
                        }
                        contours::append_gpkg_row(elevation, blob, &mut tile_bytes)?;
                        seen += 1;
                    }
                    std::cmp::Ordering::Greater => {
                        let elevation: f32 = row.get(3).map_err(|e| e.to_string())?;
                        let blob = row
                            .get_ref(4)
                            .map_err(|e| e.to_string())?
                            .as_blob()
                            .map_err(|e| e.to_string())?
                            .to_vec();
                        carry = Some((rk, elevation, blob));
                        break;
                    }
                }
            }
        }

        if seen != expected {
            return Err(format!(
                "Contour manifest mismatch at {body}/{level}/{y}/{x}"
            ));
        }

        pending_bytes += tile_bytes.len();
        pending_batch.push((
            Key {
                body,
                layer: *b"CNTR",
                grid: 2,
                level,
                y,
                x,
            },
            tile_bytes,
        ));
        if pending_batch.len() >= 256 || pending_bytes >= 32 * 1024 * 1024 {
            writer.put_batch(&pending_batch, None)?;
            pending_batch.clear();
            pending_bytes = 0;
        }
        count += 1;
        if count % 100 == 0 {
            progress(format!("Packed {count} contour tiles for body {body}"));
        }
    }
    if !pending_batch.is_empty() {
        writer.put_batch(&pending_batch, None)?;
    }
    if before != contours::fingerprint(path)? {
        return Err(format!(
            "Contour source changed during packing: {}",
            path.display()
        ));
    }
    writer.metadata(&format!("contours:{body}"), &before)?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn converter_publishes_valid_snapshot_and_cleans_failed_staging() {
        let root = std::env::temp_dir().join(format!(
            "1kee-pack-command-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let osm = root.join("osm");
        fs::create_dir_all(osm.join("road_cells")).unwrap();
        let source = osm.join("road_cells/road_cell_+000_+0000.1kc");
        fs::write(
            &source,
            cell_format::write::write_cell(0, 0, &[(*b"ROAD", &[])]),
        )
        .unwrap();
        let boundary = osm.join("road_cells/road_cell_-017_+0180.1kc");
        let boundary_features = [cell_format::CellFeature {
            way_id: 1,
            class: 2,
            is_polygon: false,
            name: None,
            points: vec![
                cell_format::CellPoint {
                    lat: -16.8,
                    lon: 179.9998,
                },
                cell_format::CellPoint {
                    lat: -16.79,
                    lon: 180.0,
                },
            ],
            elevations: Some(vec![4.0, 5.0]),
        }];
        fs::write(
            &boundary,
            cell_format::write::write_cell(-17, 180, &[(*b"ROAD", &boundary_features)]),
        )
        .unwrap();
        let out = root.join("world.1ka");
        let command = || Command {
            out: out.clone(),
            osm: Some(osm.clone()),
            pipelines: None,
            platforms: None,
            terrain: Vec::new(),
        };
        run_with_progress(command(), &mut |_| {}).unwrap();
        assert!(
            tile_archive::Reader::open(&out)
                .unwrap()
                .has_cell(*b"ROAD", 0, 0)
                .unwrap()
        );
        let reader = tile_archive::Reader::open(&out).unwrap();
        let packed = vector::read_cell(&reader, *b"ROAD", -17, 180, [-17.0, -16.0, 179.9, 180.0])
            .unwrap()
            .unwrap();
        assert_eq!(
            cell_format::write::write_cell(-17, 180, &[(*b"ROAD", &packed)]),
            fs::read(&boundary).unwrap(),
        );
        drop(reader);
        let original = fs::read(&out).unwrap();
        assert!(run_with_progress(command(), &mut |_| {}).is_err());
        assert_eq!(fs::read(&out).unwrap(), original);
        fs::write(&source, b"broken").unwrap();
        let failed = root.join("failed.1ka");
        assert!(
            run_with_progress(
                Command {
                    out: failed.clone(),
                    osm: Some(osm.clone()),
                    pipelines: None,
                    platforms: None,
                    terrain: Vec::new()
                },
                &mut |_| {}
            )
            .is_err()
        );
        assert!(!failed.exists());
        fs::remove_file(&source).unwrap();
        let invalid = osm.join("road_cells/road_cell_+000_+0181.1kc");
        fs::write(
            &invalid,
            cell_format::write::write_cell(0, 181, &[(*b"ROAD", &[])]),
        )
        .unwrap();
        let error = run_with_progress(
            Command {
                out: failed.clone(),
                osm: Some(osm),
                pipelines: None,
                platforms: None,
                terrain: Vec::new(),
            },
            &mut |_| {},
        )
        .unwrap_err();
        assert!(error.contains(&invalid.display().to_string()), "{error}");
        assert!(
            error.contains("Invalid vector cell coordinates (0,181)"),
            "{error}"
        );
        assert!(!failed.exists());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 2); // osm directory + published file only
        fs::remove_dir_all(root).unwrap();
    }
}
