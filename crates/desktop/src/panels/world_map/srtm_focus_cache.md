# srtm_focus_cache.rs

## Purpose
Builds and caches local SRTM contour assets for the currently focused event using external GDAL tools. This file exists to bypass the broken in-process GeoTIFF decode path while keeping contour generation bounded to the active view and storing the derived result in one shared SQLite tile cache instead of a directory full of tiny GeoPackages.

## Components

### `ensure_focus_contours`
- **Does**: Resolves the SRTM mirror and shared contour-cache database, buckets the current focus/zoom state, and either returns an existing local contour tile or queues background generation for it
- **Interacts with**: `terrain_assets.rs`, `contour_asset.rs`, the local GDAL CLI tools
- **Rationale**: Keeps expensive contour generation off the UI thread and reuses work across nearby zoom/view states

### `ensure_focus_contour_region`
- **Does**: Resolves a neighborhood of bucketed contour assets around the current viewport center and queues any missing buckets in the background
- **Interacts with**: `contour_asset.rs`, GDAL cache generation helpers
- **Rationale**: Supports streamed regional panning without forcing the renderer to wait on one monolithic terrain export

### `feature_budget_for_zoom`
- **Does**: Exposes the target contour feature budget for the current zoom bucket
- **Interacts with**: `contour_asset.rs`

### `half_extent_for_zoom` / `contour_interval_for_zoom`
- **Does**: Exposes the current cache bucket's spatial half-extent and contour interval
- **Interacts with**: `camera.rs`, `local_terrain_scene.rs`
- **Rationale**: Keeps local-terrain navigation and legend text aligned with the same zoom-to-LOD policy used for cache generation

### `focus_contour_region_status`
- **Does**: Reports how many contour buckets for the current streamed neighborhood are ready, pending, or still missing
- **Interacts with**: `local_terrain_scene.rs`
- **Rationale**: Lets the UI show real cache-generation progress instead of leaving the operator guessing whether terrain is still being built

### GDAL command helpers
- **Does**: Crop the needed SRTM tile subset into a local raster, run `gdal_contour` against it, then import the resulting contour rows into the shared SQLite tile cache
- **Interacts with**: local SRTM GeoTIFF tiles, `gdalwarp`, `gdal_contour`, `rusqlite`, app teardown in `app.rs`
- **Rationale**: Uses the same validated source tooling that successfully reads the external SRTM mirror on this machine while avoiding the runtime cost of managing one SQLite file per contour tile

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `contour_asset.rs` | Returned asset identifiers point at the shared SQLite cache DB and include stable zoom/lat/lon tile coordinates | Changing cache-file location, tile-key semantics, or making generation synchronous |
| `contour_asset.rs` | Region queries return the currently available neighborhood assets immediately and leave missing buckets pending in the background | Making region queries blocking or changing the bucket naming scheme |
| Runtime environment | GDAL CLIs are available either in Postgres.app, Homebrew, or `PATH` | Removing tool discovery without replacing it |

## Notes
- The derived cache now lives primarily in `Derived/terrain/srtm_focus_cache.sqlite` when available, or a temp fallback if no derived root can be resolved.
- Generation is intentionally local and LOD-bucketed; this is a streamed focus-window cache, not a globe-wide contour dataset.
- Each tile import writes contour rows into `contour_tiles` plus a manifest row in `contour_tile_manifest`, so the filesystem no longer has to act as the tile index.
- The zoom ladder is intentionally denser in local terrain mode than it was initially, so analysts can continue zooming through multiple contour extents instead of landing on one fixed terrain scene.
- Temporary GDAL outputs are now scratch files under `Derived/terrain/srtm_focus_tmp`; they are cleaned after import instead of becoming the persistent cache format.
- SQLite is opened in WAL mode with an extended busy timeout, which is a much better fit for overlapping background tile writes than a directory of thousands of small standalone databases.
- Background GDAL generation is now slot-limited instead of unbounded, so panning no longer tries to spawn a full neighborhood of contour workers at once.
- GDAL subprocesses now have a bounded timeout so a wedged export does not leave a bucket pending forever.
- App shutdown now flips the cache module into a shutdown state, stops new bucket builds from spawning, and terminates any tracked GDAL child processes so closing the app does not leave orphaned contour jobs behind.
- Feature budgets are intentionally capped per zoom bucket because the close-focus renderer now magnifies these contours substantially; oversupplying vectors just adds lag.
- Region generation uses overlapping bucket centers so local panning can stitch neighboring contour windows together without visible hard resets.
