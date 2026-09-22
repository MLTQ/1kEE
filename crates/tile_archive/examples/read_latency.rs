use cell_format::{
    CellFeature,
    read::{read_chunks, read_single_chunk},
    write::write_cell,
};
use std::{fs, path::PathBuf, time::Instant};
use tile_archive::{Reader, Writer, contours, vector};

fn normalized(mut features: Vec<CellFeature>, tag: [u8; 4]) -> Vec<u8> {
    features.sort_by_key(|f| f.way_id);
    write_cell(0, 0, &[(tag, &features)])
}
fn report(name: &str, mut samples: Vec<f64>) {
    samples.sort_by(f64::total_cmp);
    println!(
        "{name}: n={} median_ms={:.3} p95_ms={:.3} max_ms={:.3}",
        samples.len(),
        samples[samples.len() / 2],
        samples[(samples.len() * 95 / 100).min(samples.len() - 1)],
        samples.last().unwrap()
    );
}
fn main() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 2 {
        return Err("Usage: read_latency CELL.1kc NEW_ARCHIVE.1ka".into());
    }
    let source = PathBuf::from(&args[0]);
    let output = PathBuf::from(&args[1]);
    let bytes = fs::read(&source).map_err(|e| e.to_string())?;
    let chunks = read_chunks(&bytes).ok_or("Invalid cell")?;
    if chunks.len() != 1 {
        return Err("Benchmark expects one layer per source cell".into());
    }
    let tag = chunks[0].0;
    let features = &chunks[0].1;
    let lat = i16::from_le_bytes(bytes[5..7].try_into().unwrap()) as i32;
    let lon = i16::from_le_bytes(bytes[7..9].try_into().unwrap()) as i32;
    let mut writer = Writer::create(&output)?;
    let packed_bytes = vector::pack_cell(&mut writer, tag, lat, lon, features)?;
    let meta_key = format!("vector:{}:{lat}:{lon}", String::from_utf8_lossy(&tag));
    writer.metadata(&meta_key, &contours::fingerprint(&source)?)?;
    writer.finish()?;
    println!(
        "source_bytes={} source_features={} packed_payload_bytes={} archive_bytes={}",
        bytes.len(),
        features.len(),
        packed_bytes,
        fs::metadata(&output).map_err(|e| e.to_string())?.len()
    );
    let mut direct_times = Vec::new();
    let mut archive_times = Vec::new();
    let mut selected = 0;
    for i in 0..50 {
        let y = lat as f32 + (i / 7) as f32 / 8.0 + 0.01;
        let x = lon as f32 + (i % 7) as f32 / 8.0 + 0.01;
        let view = if i == 49 {
            [lat as f32, (lat + 1) as f32, lon as f32, (lon + 1) as f32]
        } else {
            [y, y + 0.10, x, x + 0.10]
        };
        let mut expected = None;
        for archived in if i % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let start = Instant::now();
            let result = if archived {
                let reader = Reader::open(&output)?;
                if reader.metadata(&meta_key)? != Some(contours::fingerprint(&source)?) {
                    return Err("Source changed".into());
                }
                vector::read_cell(&reader, tag, lat, lon, view)?.ok_or("Missing cell")?
            } else {
                let bytes = fs::read(&source).map_err(|e| e.to_string())?;
                let mut seen = std::collections::HashSet::new();
                read_single_chunk(&bytes, tag)
                    .ok_or("Invalid cell")?
                    .into_iter()
                    .filter(|f| {
                        vector::bounds(f).is_some_and(|b| vector::intersects(b, view))
                            && seen.insert(f.way_id)
                    })
                    .collect()
            };
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            if i == 49 {
                println!(
                    "whole_cell archived={archived} ms={ms:.3} features={}",
                    result.len()
                );
            } else if archived {
                archive_times.push(ms);
                selected += result.len();
            } else {
                direct_times.push(ms);
            }
            let signature = normalized(result, tag);
            if let Some(expected) = &expected {
                if expected != &signature {
                    return Err(format!("Geometry mismatch in window {i}"));
                }
            } else {
                expected = Some(signature);
            }
        }
    }
    println!(
        "Exact feature bytes matched in 49 small windows and one whole-cell view; selected_features={selected}. Warm-cache read/decode only, not pan-to-paint."
    );
    report("loose_cell", direct_times);
    report("packed_archive", archive_times);
    Ok(())
}
