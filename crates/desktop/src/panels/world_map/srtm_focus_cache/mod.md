# mod.rs

## Purpose

Coordinates terrain contour cache discovery, read-time region selection, and
on-demand Earth, lunar, and Mars tile builds.

## Components

### `ensure_*_contour_region`

- **Does**: Returns ready tile assets for the visible region and schedules
  missing tiles through the builder subsystem. Existing databases are queried
  read-only; only a missing database is opened writable for first-run schema
  initialization.
- **Interacts with**: `db.rs`, `builders.rs`, and `contour_asset.rs`.

### Region/status helpers

- **Does**: Report ready tiles and build progress without mutating cache files.
- **Interacts with**: map overlays and local/globe renderers.

### Cache path helpers

- **Does**: Resolve Earth, lunar, and Mars SQLite cache paths beneath the
  selected derived-data root.
- **Interacts with**: `terrain_assets.rs` and `db.rs`.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| `contour_asset.rs` | Existing cached tiles are discoverable without requiring write access | Using writable cache setup for renderer lookups |
| `builders.rs` | Missing tiles can use the writable schema-initializing connection | Removing builder-side write access |
| Map UI | Region queries are cheap enough for repeated frames | Synchronous GDAL work on the UI thread |

## Notes

- Renderer/status lookups intentionally use `db::open_cache_db_read_only`; this
  keeps existing contours available while a full cache volume prevents builds.
- `open_region_cache_db` preserves first-run builder behavior without putting
  established render-time caches back through writable WAL/schema setup.
