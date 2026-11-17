//! Globe source/read/publication progress; paint performs no filesystem work.
use super::*;
use crate::panels::world_map::{
    global_loading::Snapshot, local_contour_pass::LocalTileId, local_terrain_scene::composition,
};

pub(crate) fn earth() -> Snapshot {
    let Some(cache) = GLOBE_CONTOUR_CACHE.get() else {
        return Snapshot::default();
    };
    let Ok(cache) = cache.try_lock() else {
        return Snapshot {
            selecting: 1,
            ..Default::default()
        };
    };
    snapshot(&cache)
}
pub(super) fn snapshot(cache: &GlobeRegionCache) -> Snapshot {
    let Some(info) = cache.manifest.loading() else {
        return Snapshot {
            selecting: usize::from(cache.manifest.selecting()),
            ..Default::default()
        };
    };
    if !info.available {
        return Snapshot {
            total: 1,
            failed: 1,
            ..Default::default()
        };
    }
    let mut result = Snapshot::default();
    let published = cache.merged_revision == Some(cache.tiles_revision);
    for lat in info.key.center_lat_bucket - info.key.build_radius
        ..=info.key.center_lat_bucket + info.key.build_radius
    {
        for lon in info.key.center_lon_bucket - info.key.build_radius
            ..=info.key.center_lon_bucket + info.key.build_radius
        {
            let cell = (lat, lon);
            if !cache.residency.wanted(cell) {
                continue;
            }
            result.total += 1;
            let fraction = if let Some(tile) = cache.tiles.get(&cell) {
                if tile.is_empty() || published {
                    result.ready += 1;
                    1.
                } else {
                    result.preparing += 1;
                    0.99
                }
            } else if info.assets.contains(&cell) {
                if cache.read_retry_at.is_some() {
                    result.failed += 1;
                } else {
                    result.reading += 1;
                }
                0.75 + 0.24
                    * cache
                        .read_progress
                        .get(&cell)
                        .copied()
                        .unwrap_or(0.)
                        .clamp(0., 1.)
            } else if info.state.ready_buckets.contains(&cell) {
                result.ready += 1;
                1. // Known source-free cell, not invented terrain.
            } else if let Some(progress) = info.builds.get(&cell) {
                result.building += 1;
                progress.fraction()
            } else {
                result.waiting += 1;
                0.
            };
            result.fraction_sum += fraction;
            if fraction < 1. && result.bounds.len() < 32 {
                let b = composition::core(
                    crate::model::ActiveBody::Earth,
                    LocalTileId {
                        zoom_bucket: info.key.zoom_bucket,
                        lat_bucket: lat,
                        lon_bucket: lon,
                    },
                );
                result.bounds.push([
                    b.min_lon as f32,
                    b.min_lat as f32,
                    b.max_lon as f32,
                    b.max_lat as f32,
                ]);
            }
        }
    }
    result
}
pub(crate) fn overview(contours: bool, coast: bool, bathy: bool) -> Snapshot {
    let mut result = Snapshot::default();
    for (enabled, slot) in [
        (contours, &GLOBAL_TOPO_CACHE),
        (coast, &GLOBAL_COASTLINE_CACHE),
        (bathy, &GLOBAL_BATHYMETRY_CACHE),
    ] {
        if !enabled {
            continue;
        }
        let Some(cache) = slot.get() else { continue };
        if let Ok(cache) = cache.try_lock() {
            let (ready, reading, failed) = cache.loading_state();
            result.add(Snapshot {
                total: ready + reading + failed,
                ready,
                fraction_sum: ready as f32,
                reading,
                failed,
                ..Default::default()
            });
        }
    }
    result
}
