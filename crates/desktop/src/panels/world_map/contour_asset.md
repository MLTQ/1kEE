# contour_asset.rs

## Purpose
Loads contour geometry from disk into in-memory render caches for both local terrain and globe views. It merges per-tile SQLite assets into draw-ready polyline sets while avoiding UI-thread stalls.

## Components

### `LocalRegionCache`
- **Does**: Tracks currently visible local-terrain tiles, in-flight reads, and zoom fallback geometry.
- **Interacts with**: `load_srtm_region_for_view`, `load_lunar_region_for_view`.

### `GlobeRegionCache`
- **Does**: Accumulates globe-mode tiles across orbit movement and now tracks in-flight background loads so repeated repaints do not enqueue the same tile reads.
- **Interacts with**: `load_srtm_for_globe`, `load_lunar_for_globe`.

### `load_lunar_region_for_view` / `load_lunar_for_globe`
- **Does**: Request lunar cache assets, batch missing-tile SQLite reads onto one background thread, and merge the ready contours for rendering.
- **Interacts with**: `srtm_focus_cache`, `query_local_contours_batch`.
- **Rationale**: Lunar mode often needs many overlapping tiles from the same SQLite file; batching reduces connection churn and thread storms.

### `query_local_contours` / `query_local_contours_batch`
- **Does**: Decode tile contour blobs from SQLite into `ContourPath` polylines.
- **Interacts with**: `parse_gpkg_lines`, render caches.

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| World-map renderers | Returned contours are simplified but geographically correct polylines | Changing coordinate decoding or simplification semantics |
| `srtm_focus_cache` | Tile cache keys remain `(path, zoom_bucket, lat_bucket, lon_bucket)` compatible with SQLite manifests | Changing keying or bucket math |
| UI responsiveness | All disk reads stay off the render thread and avoid duplicate in-flight work | Reintroducing synchronous reads or duplicate thread spawns |

## Notes
- Lunar local rendering still performs the midpoint-based exclusive-region filter so overlapping tiles do not double-draw the same contour.
- Batched reads still execute one SQL query per tile, but they reuse a single SQLite connection and one worker thread per repaint batch.
