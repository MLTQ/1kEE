//! Disjoint ownership for mixed legacy/full-footprint and modern globe tiles.
use super::*;
use crate::model::ActiveBody;
use crate::panels::world_map::{local_contour_pass::LocalTileId, local_terrain_scene::composition};
use tile_archive::contour_grid::Bounds as Rect;

type Cell = (i32, i32);
type Tiles = HashMap<Cell, Arc<Vec<ContourPath>>>;

/// Keep the last published picture while one worker rebuilds changed tiles.
pub(super) fn render(
    cache: &'static Mutex<GlobeRegionCache>,
    ctx: &egui::Context,
) -> Option<Arc<Vec<ContourPath>>> {
    let mut guard = cache.lock().ok()?;
    if guard.merge_in_flight.is_none() && guard.merged_revision != Some(guard.tiles_revision) {
        let (epoch, revision, bucket) = (guard.load_epoch, guard.tiles_revision, guard.zoom_bucket);
        let tiles = guard.tiles.clone();
        let fallback = guard.zoom_fallback.clone();
        guard.merge_in_flight = Some((epoch, revision));
        let wake = ctx.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("globe-contour-ownership".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    compose(bucket, &tiles, fallback)
                }));
                if let Ok(mut guard) = cache.lock() {
                    finish(&mut guard, epoch, revision, result.ok());
                }
                wake.request_repaint();
            })
        {
            guard.merge_in_flight = None;
            eprintln!("[1kEE] globe contour ownership worker: {error}");
            ctx.request_repaint_after(CONTOUR_READ_RETRY_DELAY);
        }
    }
    guard.merged.clone().or_else(|| guard.zoom_fallback.clone())
}

fn finish(
    cache: &mut GlobeRegionCache,
    epoch: u64,
    revision: u64,
    result: Option<Arc<Vec<ContourPath>>>,
) {
    if cache.merge_in_flight != Some((epoch, revision)) {
        return;
    }
    cache.merge_in_flight = None;
    if cache.load_epoch == epoch
        && let Some(result) = result
    {
        // Accept a coherent intermediate snapshot even when more tiles arrived:
        // it makes progress now, and the next repaint coalesces the newer data.
        cache.merged = Some(result);
        cache.merged_revision = Some(revision);
    }
}

fn tile_id(bucket: i32, (lat, lon): Cell) -> LocalTileId {
    LocalTileId {
        zoom_bucket: bucket,
        lat_bucket: lat,
        lon_bucket: lon,
    }
}

fn intersection(a: Rect, b: Rect) -> Option<Rect> {
    let r = Rect {
        min_lon: a.min_lon.max(b.min_lon),
        min_lat: a.min_lat.max(b.min_lat),
        max_lon: a.max_lon.min(b.max_lon),
        max_lat: a.max_lat.min(b.max_lat),
    };
    (r.min_lon < r.max_lon && r.min_lat < r.max_lat).then_some(r)
}

fn footprint(bounds: residency::Bounds) -> Rect {
    // A contour set can be a single horizontal/vertical line. Give such a
    // footprint area for rectangle subtraction without moving the line itself.
    Rect {
        min_lon: f64::from(bounds.min[0]) - 1e-6,
        min_lat: f64::from(bounds.min[1]) - 1e-6,
        max_lon: f64::from(bounds.max[0]) + 1e-6,
        max_lat: f64::from(bounds.max[1]) + 1e-6,
    }
}

/// Native cores win. Missing cores can still use a legacy tile's outer geometry;
/// overlapping outer pieces choose the nearest available source deterministically.
fn ownership(bucket: i32, tiles: &Tiles) -> HashMap<Cell, Vec<Rect>> {
    let mut regions: HashMap<Cell, Vec<Rect>> = HashMap::new();
    let mut halos: HashMap<Cell, Vec<(Cell, Rect)>> = HashMap::new();
    for (&cell, contours) in tiles {
        let own = composition::core(ActiveBody::Earth, tile_id(bucket, cell));
        regions.entry(cell).or_default().push(own); // decoded empty cores count too
        let Some(bounds) = residency::Bounds::from_contours(contours) else {
            continue;
        };
        let bounds = footprint(bounds);
        // Historical footprints have half-extent step/0.45, spanning at most
        // three neighboring cores each way. Bound traversal even for bad input.
        for lat in cell.0 - 3..=cell.0 + 3 {
            for lon in cell.1 - 3..=cell.1 + 3 {
                let neighbor = (lat, lon);
                if tiles.contains_key(&neighbor) {
                    continue;
                }
                let core = composition::core(ActiveBody::Earth, tile_id(bucket, neighbor));
                if let Some(part) = intersection(core, bounds) {
                    halos.entry(neighbor).or_default().push((cell, part));
                }
            }
        }
    }
    let mut cells: Vec<_> = halos.into_iter().collect();
    cells.sort_by_key(|(cell, _)| *cell);
    for (cell, mut candidates) in cells {
        candidates.sort_by_key(|(id, _)| ((id.0 - cell.0).pow(2) + (id.1 - cell.1).pow(2), *id));
        let mut available = vec![composition::core(ActiveBody::Earth, tile_id(bucket, cell))];
        for (source, bounds) in candidates {
            for &space in &available {
                if let Some(part) = intersection(space, bounds) {
                    regions.entry(source).or_default().push(part);
                }
            }
            available = available
                .into_iter()
                .flat_map(|r| composition::subtract(r, bounds))
                .collect();
            if available.is_empty() {
                break;
            }
        }
    }
    regions
}

fn compose(
    bucket: i32,
    tiles: &Tiles,
    fallback: Option<Arc<Vec<ContourPath>>>,
) -> Arc<Vec<ContourPath>> {
    let regions = ownership(bucket, tiles);
    let mut keys: Vec<_> = tiles.keys().copied().collect();
    keys.sort();
    let mut merged = Vec::new();
    for cell in keys {
        let contours = tiles[&cell].clone();
        let tile = LocalTileGeometry {
            id: tile_id(bucket, cell),
            bounds: residency::Bounds::from_contours(&contours),
            contours,
        };
        merged.extend(
            composition::clip_contours(&tile, &regions[&cell])
                .iter()
                .cloned(),
        );
    }
    // Keep the outgoing tier only in areas that the incoming data cannot cover.
    if let Some(contours) = fallback
        && let Some(bounds) = residency::Bounds::from_contours(&contours)
    {
        let mut remaining = vec![footprint(bounds)];
        let mut ordered: Vec<_> = regions.iter().collect();
        ordered.sort_by_key(|(key, _)| **key);
        for (_, owned) in ordered {
            for &r in owned {
                remaining = remaining
                    .into_iter()
                    .flat_map(|space| composition::subtract(space, r))
                    .collect();
            }
        }
        let tile = LocalTileGeometry {
            id: tile_id(bucket, (0, 0)),
            bounds: Some(bounds),
            contours,
        };
        merged.extend(
            composition::clip_contours(&tile, &remaining)
                .iter()
                .cloned(),
        );
    }
    merged.sort_by(|a, b| a.elevation_m.abs().total_cmp(&b.elevation_m.abs()));
    Arc::new(merged)
}

#[cfg(test)]
#[path = "globe_contour_merge_tests.rs"]
mod tests;
