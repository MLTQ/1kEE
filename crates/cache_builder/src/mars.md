# mars.rs

## Purpose

Builds reusable Mars contour tiles from CTX DTMs, falling back to global MOLA
MEGDR data where CTX does not cover the selected region. The resulting SQLite
cache uses the same tile schema and zoom buckets as the desktop Mars renderer.

## Components

### `MarsBuildCommand`

- **Does**: Carries source paths, cache destination, geographic bounds, zoom
  selection, and optional GDAL location for one background build job.
- **Interacts with**: `app.rs` form validation and `job.rs` dispatch.

### `build_mars_contour_tiles`

- **Does**: Validates GDAL, indexes CTX data, prepares the MOLA VRT, plans
  missing tiles, and imports parallel GDAL contour results into SQLite.
- **Interacts with**: `contours.rs`, `lunar.rs` zoom specifications, and the
  desktop `srtm_focus_cache` contract.
- **Rationale**: CTX is preferred for detailed terrain; MOLA preserves global
  coverage where stereo DEMs are absent.

### CTX/MOLA helpers

- **Does**: Locate source rasters, create the MOLA mosaic, run bounded GDAL
  commands, and clean up per-tile temporary artifacts.
- **Interacts with**: the operator's Mars data root and GDAL executables.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| `app.rs` | A validated `MarsBuildCommand` produces progress events and a result summary | Changing command fields or progress semantics |
| Desktop Mars renderer | Cache path and SQLite tile schema match `mars_ctx_cache.sqlite` | Changing database name, tile keys, or geometry schema |
| Operators | Existing tiles with real contours are skipped; MOLA can upgrade empty tiles | Removing skip/upgrade behavior |

## Notes

- `mars_ctx_cache.sqlite` is the canonical shared cache name. Legacy
  `mars_focus_cache.sqlite` files are not migrated or rewritten automatically.
- The builder checkpoints WAL after a run, but it requires free space to create
  new terrain data.
