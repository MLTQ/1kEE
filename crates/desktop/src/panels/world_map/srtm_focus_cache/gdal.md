# gdal.rs

## Purpose
Owns the external GDAL command workflow for the focus cache. It discovers source rasters, supervises subprocesses, builds temporary rasters/GeoPackages, and imports the results into SQLite.

## Components

### Source discovery helpers
- **Does**: Find prebuilt VRTs, GEBCO tiles, and raw SRTM tiles on disk
- **Interacts with**: `mod.rs`

### Global build helpers
- **Does**: Build the globe-scale land overview and coastline caches in the background
- **Interacts with**: `mod.rs`, `db.rs`

### Process supervision helpers
- **Does**: Track active child processes, resolve GDAL tool paths, and enforce timeout/shutdown behavior
- **Interacts with**: app shutdown, `settings_store.rs`

### `build_focus_contours`
- **Does**: Warp the relevant SRTM tiles, contour them, import the rows into SQLite, and piggyback a 0m coastline extraction
- **Interacts with**: `db.rs`, `builders.rs`, `zoom.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `builders.rs` | Tile builds are idempotent and clean up temp files on success/failure | Leaving stale temp artifacts or dropping shutdown handling |
| `mod.rs` | Global build flags and child tracking remain process-global | Making build state instance-local or removing cancellation |
