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

- Contour/coastline imports prepare the destination insert once, borrow source
  geometry blobs, and stream in fid order. No geometry sort is needed while
  importing; display order is restored by the reader. Rows and manifests still
  commit together, and malformed source schemas/rows roll back the transaction
  instead of marking a broken tile as successfully empty.

- Regression tests verify exact geometry, atomic replacement and rollback for
  malformed rows/schemas, coastline parity, and valid empty-source imports.

- Import sources open read-only; a missing temporary GeoPackage is an error,
  so it cannot create an accidental empty manifest entry.

- Active build handles receive imported-row counts every 128 contours, using
  the source table count as denominator. The commit milestone advances only
  after the SQLite transaction succeeds; import errors cannot report completion.

- Import progress is passed as the exact attempt handle rather than looked up
  by tile key, so late imports cannot advance a newer attempt after a reset.
  A regression checks this with two handles for the same tile.

- Import profiling separates connection/schema setup, immediate writer-lock
  acquisition, and row copying/commit. `BEGIN IMMEDIATE` obtains the writer
  before mutations, keeping lock wait separate while preserving atomic import
  and the existing busy timeout. Successful writes report rows and blob bytes.

- Tile temporary names include process/attempt IDs, isolating terrain bodies
  and multiple app instances. `TempTileCleanup` removes each attempt’s TIFF,
  GeoPackage, and SQLite sidecars on success, early return, or unwinding.

- A regression unwinds one attempt with raster/GeoPackage/journal/WAL/SHM
  files and verifies cleanup leaves a simultaneous same-tile attempt intact.

- Explicit clipped import variants store only an Earth tile core. Unsupported
  geometry rolls back the transaction; discarded halo rows do not inflate the
  manifest. Progress counts processed source rows, including discarded ones.
  Original import wrappers retain unmodified lunar/Mars and legacy semantics.
