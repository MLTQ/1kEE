# contour_asset.rs

## Purpose
Loads contour geometry from disk into in-memory render caches for both local terrain and globe views. It merges per-tile SQLite assets into draw-ready polyline sets while avoiding UI-thread stalls.

## Components

### `LocalRegionCache`
- **Does**: Tracks currently visible local-terrain tiles, a generation-safe
  single in-flight SQLite/WKB read, a wide decoded return-pan envelope,
  background manifest/merge workers, and zoom fallback geometry. It merges
  the active 13×13 source envelope off-thread so overlapping Moon/Mars
  contours owned by an outer tile cannot vanish at the visible edge or stall
  paint; the local draw pass performs its existing geographic AABB cull before
  projection.
- **Interacts with**: `load_srtm_region_for_view`, `load_lunar_region_for_view`.

### `GlobeRegionCache`
- **Does**: Accumulates globe-mode tiles across orbit movement, tracks in-flight
  background loads, and memoizes the merged contour `Arc` behind a monotonic
  tile-set revision while its tile set is unchanged.
- **Interacts with**: `load_srtm_for_globe`, `load_lunar_for_globe`.

### `render_globe_tiles` / `merged_partitioned_local_contours`

- **Does**: Return stable merged `Arc`s until the underlying tile set changes;
  the latter also preserves the exclusive-region filter needed for overlapping
  lunar and Mars tiles. Empty newly-ready globe tiles continue to display the
  prior zoom fallback rather than flashing blank.
- **Interacts with**: `contour_pass.rs`, local-terrain scene renderers.

### `begin_*_read` / `spawn_*_read`

- **Does**: Coalesce camera-driven tile requests into at most one named reader
  per cache. Reset epochs make late readers discard stale results rather than
  repopulating a cleared or newly selected scene; a short retry backoff avoids
  reader churn when a cache database is temporarily unavailable. Local reads
  prioritize the viewport center and consume small bounded batches, so a wide
  prefetch ring does not delay visible contours behind outer tiles or publish
  a large one-frame geometry spike.
- **Interacts with**: Both local and globe contour loaders,
  `query_local_contours_batch`, and repaint scheduling.

### WKB decoding

- **Does**: Accepts direct LineString children of MultiLineString geometries
  without recursively descending nested collections, and validates geometry
  counts before reserving memory.
- **Interacts with**: SQLite contour cache rows and `parse_gpkg_lines`.

### `load_lunar_region_for_view` / `load_lunar_for_globe`
- **Does**: Request lunar cache assets, batch missing-tile SQLite reads onto one background thread, and merge the ready contours for rendering.
- **Interacts with**: `srtm_focus_cache`, `query_local_contours_batch`.
- **Rationale**: Lunar mode often needs many overlapping tiles from the same SQLite file; batching reduces connection churn and thread storms.

### `query_local_contours_batch`
- **Does**: Decode tile contour blobs from SQLite into `ContourPath` polylines.
- **Interacts with**: `srtm_focus_cache::db::open_cache_db_read_only`,
  `parse_gpkg_lines`, render caches.

### Local manifest and merge workers

- **Does**: Runs SQLite/root/build selection and full local tile flattening on
  named single-flight workers. Results are revision/window checked before
  publication; the last complete contour `Arc` stays drawable during a refresh.
- **Interacts with**: `LocalRegionCache`, `srtm_focus_cache`, and egui repaint
  requests.
- **Rationale**: A cache miss, external-drive contention, or dense Mars batch
  must never make the UI thread wait for SQLite or clone every contour path.

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| World-map renderers | Returned contours are simplified but geographically correct polylines | Changing coordinate decoding or simplification semantics |
| `srtm_focus_cache` | Tile cache keys remain `(path, zoom_bucket, lat_bucket, lon_bucket)` compatible with SQLite manifests | Changing keying or bucket math |
| UI responsiveness | Local manifest selection, disk reads, and full contour merges stay off the render thread; each cache has one coalesced reader/merge and local reads stay batch-bounded during camera motion | Reintroducing synchronous selection/merges or an unbounded reader fan-out |
| GPU contour pass | Unchanged globe tiles return the same merged Arc so their instance version stays stable across repaints | Allocating a fresh merged Arc every frame |

## Notes
- Lunar local rendering still performs the midpoint-based exclusive-region filter so overlapping tiles do not double-draw the same contour.
- Batched reads still execute one SQL query per tile, but they reuse a single SQLite connection and one worker thread per repaint batch.
- All renderer geometry readers, including global contour GeoPackages, use the
  cache module's read-only/immutable fallback. A full derived-data volume can
  therefore stop new cache builds without blanking already checkpointed lines.
- `blast_tile_caches()` clears tile entries and memoized merges together, so a
  manual reset cannot retain obsolete contour geometry in memory; its epoch
  also prevents old readers from writing back after the reset and releases
  any failed-build cooldowns after a user fixes storage/source availability.
- Ready-but-empty tiles are cached as empty results, avoiding repeated SQLite
  reads for nodata terrain while preserving an existing zoom fallback.
- Local terrain retains a bounded 12-tile return-pan envelope. Its full local
  prefetch envelope is both the merge source and build window, ensuring a cold
  outer Moon/Mars ownership tile cannot leave a gap at the visible edge. The
  globe keeps its own smaller fetch behavior.
- Local manifest selections are reused while the viewport remains in the same
  bucket. Their refresh and on-demand build selection happen in a single
  worker; in-process builder completion invalidates them immediately, while a
  short timeout admits writes from the companion cache builder without
  continuous SQLite polling.
- A local read batch is intentionally eight tiles. The full 13×13 envelope is
  still retained and eventually decoded, but small center-first arrivals avoid
  a burst of thousands of new paths in one frame.
- Full tile flattening and lunar/Mars ownership partitioning use an immutable
  `Arc` snapshot on a background worker. A result only replaces the displayed
  merge if its cache revision and requested camera window still match.
- `LocalContourLoad` carries ready/progress data from that same manifest
  selection, so the pulse grid and progress cards do not rescan SQLite.
- A mixed batch commits every tile it decoded successfully. A fully failed
  batch retries after a short delay, so one locked or malformed tile cannot
  repeatedly throw away healthy neighbouring contours.
- WKB MultiLineString children are required to be direct LineStrings; rejecting
  invalid nesting and impossible count fields prevents a corrupt cache blob
  from overflowing a loader thread's stack or allocating unreasonable memory.
