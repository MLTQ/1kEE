use crate::{Key, Reader, Writer};
use cell_format::{CellFeature, read::read_single_chunk, write::write_cell_refs};

pub const DIVISIONS: i32 = 8;
pub type Bounds = [f32; 4]; // min latitude, max latitude, min longitude, max longitude

pub fn bounds(feature: &CellFeature) -> Option<Bounds> {
    let first = feature.points.first()?;
    let mut b = [first.lat, first.lat, first.lon, first.lon];
    for p in &feature.points {
        if !p.lat.is_finite() || !p.lon.is_finite() {
            return None;
        }
        b[0] = b[0].min(p.lat);
        b[1] = b[1].max(p.lat);
        b[2] = b[2].min(p.lon);
        b[3] = b[3].max(p.lon);
    }
    Some(b)
}

pub fn intersects(a: Bounds, b: Bounds) -> bool {
    a[0] <= b[1] && a[1] >= b[0] && a[2] <= b[3] && a[3] >= b[2]
}

/// A broad view can read the original cell more cheaply than assembling many
/// full-detail subtiles. Until baked LODs exist, prefer it above quarter coverage.
pub fn prefer_subtiles(lat: i32, lon: i32, view: Bounds) -> bool {
    let axis = |lo: f32, hi: f32, cell: i32| {
        let start = ((lo * 8.0).floor() as i32).max(cell * 8);
        let end = ((hi * 8.0).floor() as i32).min(cell * 8 + 7);
        (end - start + 1).max(0)
    };
    axis(view[0], view[1], lat) * axis(view[2], view[3], lon) <= 16
}

fn key(layer: [u8; 4], y: i32, x: i32) -> Key {
    Key {
        body: 0,
        layer,
        grid: 1,
        level: 0,
        y,
        x,
    }
}

pub fn encode_cell(
    layer: [u8; 4],
    lat: i32,
    lon: i32,
    features: &[CellFeature],
) -> Result<(Vec<(Key, Vec<u8>)>, usize), String> {
    // Existing builders use floor(min)..=floor(max), so a feature touching
    // +180 longitude or +90 latitude also has a cell at that boundary. Keep
    // those parent keys and their full geometry instead of wrapping or dropping
    // them; readers use the same inclusive cell enumeration.
    if !(-90..=90).contains(&lat) || !(-180..=180).contains(&lon) {
        return Err(format!(
            "Invalid vector cell coordinates ({lat},{lon}): expected latitude -90..=90 and longitude -180..=180"
        ));
    }
    let mut ids = std::collections::HashSet::with_capacity(features.len());
    if features.iter().any(|f| !ids.insert(f.way_id)) {
        return Err(
            "Duplicate feature ID in source cell; cannot safely deduplicate subtiles".into(),
        );
    }
    let boxes = features
        .iter()
        .map(|f| {
            if f.elevations
                .as_ref()
                .is_some_and(|e| e.len() != f.points.len())
            {
                return Err("Invalid baked elevation array".to_owned());
            }
            bounds(f).ok_or_else(|| "Empty/non-finite vector geometry".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    // Bucket feature indices into the 8×8 subtiles in a single O(F) pass
    // instead of scanning every feature 64 times.
    let mut buckets: [Vec<usize>; 64] = std::array::from_fn(|_| Vec::new());
    let base_y = lat * DIVISIONS;
    let base_x = lon * DIVISIONS;
    for (idx, b) in boxes.iter().enumerate() {
        let min_dy = (((b[0] * 8.0).floor() as i32) - base_y - 1).clamp(0, DIVISIONS - 1);
        let max_dy = (((b[1] * 8.0).ceil() as i32) - base_y).clamp(0, DIVISIONS - 1);
        let min_dx = (((b[2] * 8.0).floor() as i32) - base_x - 1).clamp(0, DIVISIONS - 1);
        let max_dx = (((b[3] * 8.0).ceil() as i32) - base_x).clamp(0, DIVISIONS - 1);
        for dy in min_dy..=max_dy {
            let y = base_y + dy;
            let ty0 = y as f32 / 8.0;
            let ty1 = (y + 1) as f32 / 8.0;
            if b[0] > ty1 || b[1] < ty0 {
                continue;
            }
            for dx in min_dx..=max_dx {
                let x = base_x + dx;
                let tx0 = x as f32 / 8.0;
                let tx1 = (x + 1) as f32 / 8.0;
                if b[2] <= tx1 && b[3] >= tx0 {
                    buckets[(dy * DIVISIONS + dx) as usize].push(idx);
                }
            }
        }
    }

    let mut tiles = Vec::with_capacity(64);
    let mut bytes = 0;
    let mut selected: Vec<&CellFeature> = Vec::new();
    for dy in 0..DIVISIONS {
        for dx in 0..DIVISIONS {
            let y = base_y + dy;
            let x = base_x + dx;
            let bucket = &buckets[(dy * DIVISIONS + dx) as usize];
            selected.clear();
            selected.reserve(bucket.len().saturating_sub(selected.capacity()));
            for &idx in bucket {
                selected.push(&features[idx]);
            }
            let data = write_cell_refs(lat as i16, lon as i16, &[(layer, &selected)]);
            bytes += data.len();
            if bytes > crate::MAX_PAYLOAD * 2 {
                return Err(
                    "Packed cell exceeds 512 MiB; reduce feature extents before conversion".into(),
                );
            }
            tiles.push((key(layer, y, x), data));
        }
    }
    Ok((tiles, bytes))
}

pub fn pack_cell(
    writer: &mut Writer,
    layer: [u8; 4],
    lat: i32,
    lon: i32,
    features: &[CellFeature],
) -> Result<usize, String> {
    let (tiles, bytes) = encode_cell(layer, lat, lon, features)?;
    writer.put_batch(&tiles, Some((layer, lat, lon)))?;
    Ok(bytes)
}

pub fn read_cell(
    reader: &Reader,
    layer: [u8; 4],
    lat: i32,
    lon: i32,
    view: Bounds,
) -> Result<Option<Vec<CellFeature>>, String> {
    if !reader.has_cell(layer, lat, lon)? {
        return Ok(None);
    }
    if view.iter().any(|v| !v.is_finite()) || view[0] > view[1] || view[2] > view[3] {
        return Err("Invalid view bounds".into());
    }
    let min_y = ((view[0] * 8.0).floor() as i32).max(lat * 8);
    let max_y = ((view[1] * 8.0).floor() as i32).min(lat * 8 + 7);
    let min_x = ((view[2] * 8.0).floor() as i32).max(lon * 8);
    let max_x = ((view[3] * 8.0).floor() as i32).min(lon * 8 + 7);
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let data = reader
                .get(key(layer, y, x))?
                .ok_or("Incomplete archive cell")?;
            let features =
                read_single_chunk(&data, layer).ok_or("Invalid archive vector payload")?;
            for f in features {
                if bounds(&f).is_some_and(|b| intersects(b, view)) && seen.insert(f.way_id) {
                    out.push(f);
                }
            }
        }
    }
    Ok(Some(out))
}
