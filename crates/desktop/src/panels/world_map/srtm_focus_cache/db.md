# db.rs

## Purpose
Provides the SQLite storage layer for focus-cache contour and coastline tiles. It creates the cache schema, imports GDAL outputs, and manages cache/temp file locations.

## Components

### `open_cache_db` / `ensure_cache_schema_with_connection`
- **Does**: Open the shared cache DB with WAL-friendly settings and ensure the required tables exist
- **Interacts with**: `mod.rs`, `builders.rs`

### `tile_exists`
- **Does**: Checks whether a contour tile manifest row already exists for a tile key
- **Interacts with**: `builders.rs`, `mod.rs`

### `import_tile_into_cache` / `import_coastline_into_cache`
- **Does**: Copy contour or coastline rows from a temporary GeoPackage into the shared cache database
- **Interacts with**: `gdal.rs`

### Cache-path helpers
- **Does**: Resolve the terrain cache root, DB path, journal/WAL sidecars, and temp artifact paths
- **Interacts with**: `mod.rs`, `gdal.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `gdal.rs` | Import helpers can be called repeatedly for the same tile and will replace stale rows atomically | Removing transactional replace semantics |
| `mod.rs` | Cache root resolution stays under the derived terrain root when available | Relocating the cache without updating callers |
