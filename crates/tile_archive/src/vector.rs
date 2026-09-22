use crate::{Key, Reader, Writer};
use cell_format::{CellFeature, read::read_single_chunk, write::write_cell};

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

pub fn pack_cell(
    writer: &mut Writer,
    layer: [u8; 4],
    lat: i32,
    lon: i32,
    features: &[CellFeature],
) -> Result<usize, String> {
    if !(-90..90).contains(&lat) || !(-180..180).contains(&lon) {
        return Err("Invalid vector cell coordinates".into());
    }
    let mut ids = std::collections::HashSet::new();
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
    let mut tiles = Vec::with_capacity(64);
    let mut bytes = 0;
    for dy in 0..DIVISIONS {
        for dx in 0..DIVISIONS {
            let y = lat * DIVISIONS + dy;
            let x = lon * DIVISIONS + dx;
            let tile = [
                y as f32 / 8.0,
                (y + 1) as f32 / 8.0,
                x as f32 / 8.0,
                (x + 1) as f32 / 8.0,
            ];
            let selected = features
                .iter()
                .zip(&boxes)
                .filter(|(_, b)| intersects(**b, tile))
                .map(|(f, _)| f.clone())
                .collect::<Vec<_>>();
            let data = write_cell(lat as i16, lon as i16, &[(layer, &selected)]);
            bytes += data.len();
            if bytes > crate::MAX_PAYLOAD * 2 {
                return Err(
                    "Packed cell exceeds 512 MiB; reduce feature extents before conversion".into(),
                );
            }
            tiles.push((key(layer, y, x), data));
        }
    }
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
