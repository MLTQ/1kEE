# mod.rs

## Purpose
Owns the modularized SRTM focus-cache pipeline. It coordinates zoom policy, SQLite cache lookups, background GDAL builds, and progress reporting for local terrain streaming.

## Components

### `FocusContourAsset` / `FocusContourRegionStatus`
- **Does**: Define the contour-tile identity and region-progress data returned to renderers
- **Interacts with**: `contour_asset.rs`, `local_terrain_scene.rs`

### `ensure_focus_contour_region`
- **Does**: Resolves a neighborhood of contour buckets and queues missing buckets in the background
- **Interacts with**: `builders.rs`, `db.rs`, `zoom.rs`

### `ready_tile_buckets` / `focus_contour_region_status`
- **Does**: Expose tile readiness and pending counts for UI overlays and loading animation
- **Interacts with**: `local_terrain_scene.rs`

### Global overview / coastline helpers
- **Does**: Manage one-time background builds for globe-scale derived assets
- **Interacts with**: `gdal.rs`, `contour_asset.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `local_terrain_scene.rs` | Region status and ready-bucket queries remain cheap and non-blocking | Making status checks blocking or removing pending-state reporting |
| `contour_asset.rs` | Returned assets continue to identify rows inside the shared SQLite cache DB | Changing asset identity away from the shared cache database |

## Notes
- This directory-backed module is now the active focus-cache implementation; the older sibling `srtm_focus_cache.rs` remains in the tree but is no longer the path selected by `world_map.rs`.
