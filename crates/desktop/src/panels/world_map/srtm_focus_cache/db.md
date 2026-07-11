# db.rs

## Purpose

Owns the SQLite contour-cache schema, cache-path helpers, and the connection
contracts shared by on-demand terrain builders and render-time tile lookups.

## Components

### `open_cache_db`

- **Does**: Opens a writable cache database, configures WAL/temporary storage,
  and creates or migrates the schema for builder/import work.
- **Interacts with**: `builders.rs`, `gdal.rs`, and cache import helpers.

### `open_cache_db_read_only`

- **Does**: Opens an existing cache for renderer/status queries without schema
  setup, WAL configuration, or filesystem writes. It validates a normal
  read-only connection first, then falls back to SQLite immutable mode if WAL
  shared-memory access is impossible.
- **Interacts with**: `mod.rs` region lookups and `contour_asset.rs` readers.

### Tile manifest helpers

- **Does**: Read and update contour/coastline tile manifest rows.
- **Interacts with**: tile builders and focus-region asset selection.

### `contour_manifest_window`

- **Does**: Reads one indexed rectangular manifest snapshot for a zoom bucket.
- **Interacts with**: `mod.rs` region selection, which uses it to make wide
  local prefetch decisions without a SQLite query per tile.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| Builders/importers | `open_cache_db` may initialize and mutate cache files | Changing write pragmas or schema setup |
| Render-time lookup | Read-only queries work even when the cache volume has no free space | Opening renderer paths in write/WAL mode |
| Contour cache schema | Tile identity remains `(zoom_bucket, lat_bucket, lon_bucket)` | Changing manifest or tile key columns |

## Notes

- Read-only connections set `temp_store=MEMORY` so sort/query scratch space
  does not require room on the cache volume.
- The normal connection reads `schema_version` before it is returned. This
  makes a deferred WAL/`-shm` failure trigger immutable fallback instead of
  surfacing later in a contour query.
- Manifest-window snapshots are read-only and contain only tile identifiers
  and contour counts; geometry remains in the background WKB reader.
