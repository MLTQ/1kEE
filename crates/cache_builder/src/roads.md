# roads.rs

## Purpose
Implements the two-pass offline road cache builder: Pass 1 collects candidate nodes into SQLite, Pass 2 resolves ways into polylines and writes per-cell GeoJSON files. Both passes are fully resumable — a blob-level byte-offset checkpoint is written after every batch so a killed process restarts from exactly where it left off, not from the beginning of the planet file.

## Components

### `PosReader`
- **Does**: Wraps a `File` in a `Read` impl that atomically increments a shared `Arc<AtomicU64>` as bytes are consumed. Because PBF blobs are length-prefixed, the counter equals the file offset of the *next blob* after each `BlobReader::next()` call — a precise resume point.
- **Interacts with**: `open_planet_at`, `BlobReader`

### `open_planet_at(planet_path, start_offset)`
- **Does**: Opens the planet file seeked to `start_offset` (0 for fresh start), wraps it in `PosReader`, and returns a `BlobReader<PosReader>` plus the shared position `Arc`.

### `load_or_collect_candidate_nodes`
- **Does**: Decides whether to (a) skip Pass 1 entirely (node cache complete), (b) resume from a saved offset, or (c) start fresh. Only calls `reset()` for case (c).

### `collect_candidate_nodes`
- **Does**: Iterates blobs, batches in-bounds nodes, and after every 50 k nodes: flushes to SQLite and writes a `"node_scan"` offset checkpoint. `ON CONFLICT DO UPDATE` makes re-inserts on resume idempotent.

### `collect_roads_by_cell`
- **Does**: Pass 2 — resolves way refs via batched `NodeStore::points_for_refs`, buffers by 1° cell, and after every 10 k roads: flushes cells to GeoJSON and saves a `"way_scan"` offset checkpoint. `merge_write_cells` deduplicates by `way_id` so any overlap at the checkpoint boundary is harmless.

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `app.rs` | `build_bbox_cache_with_progress` emits progress with monotonically increasing `fraction` | Regressing fraction values |
| resumed builds | `collect_candidate_nodes` does NOT call `reset()` when `resume_offset.is_some()` | Clearing partial node data on resume |
| way-scan restart | `merge_write_cells` overwrites duplicate `way_id`s; re-processing blobs near checkpoint is safe | Changing merge to append |

## Notes
- Both passes clear their checkpoint key only on *successful* completion, so a crash always leaves a valid resume offset.
- `unsafe impl Send for PosReader` is required because the generic wrapper inhibits the auto-impl; the `BlobReader` owns the reader exclusively so there is no actual data race.

## Original Purpose
Implements the first offline OSM cache-building command: generate direct per-cell road GeoJSON caches from a requested bbox in `planet.osm.pbf`. This is the initial step toward moving heavy OSM parsing out of the desktop app.

## Components

### `build_bbox_cache`
- **Does**: Validates inputs, runs the two-pass planet scan, and writes the resulting road-cell caches
- **Interacts with**: `args.rs`, `geojson.rs`, `util.rs`

### `RoadBuildProgress`
- **Does**: Carries stage/fraction/message updates from the offline road builder to the GUI worker
- **Interacts with**: `job.rs`, `app.rs`

### `build_bbox_cache_with_progress`
- **Does**: Runs the same offline road export while emitting coarse progress updates for UI consumers
- **Interacts with**: `job.rs`, `geojson.rs`
- **Rationale**: This is the resumable/offline-friendly path used by the GUI, including node checkpoints and incremental road-cell flushes

### `collect_candidate_nodes`
- **Does**: First pass over the planet file, retaining only nodes inside the expanded requested bbox and streaming them into the disk-backed node cache
- **Interacts with**: `node_store.rs`, `util.rs`
- **Rationale**: The downloaded planet file does not advertise `LocationsOnWays`, so the builder has to resolve node refs itself without holding the full candidate set in RAM

### `collect_roads_by_cell`
- **Does**: Second pass over the planet file, filters `highway=*` ways, reconstructs polylines from retained nodes, and groups them into 1° cache cells
- **Interacts with**: `geojson.rs`, `util.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `main.rs` | `build_bbox_cache` returns `Result<(), String>` with readable failures | Changing the return contract |
| `job.rs` | progress updates use `RoadBuildProgress` with `stage`, `fraction`, and `message` fields | Renaming or removing progress fields |
| desktop road loader | emitted road classes and GeoJSON schema match the direct vector cache it already reads | Renaming road classes or changing the file format |
| future resumed builds | disk-backed candidate-node caches under `.builder_state/` are keyed by bbox and can be reused safely for the same request | Changing cache naming/format without migration |

## Notes
- This first builder command still scans the planet twice for a bbox, which is acceptable for offline work and much better than doing it on the interactive UI thread.
- The current target is roads only. Water, buildings, and boundaries can be layered onto the same offline builder approach next.
- Completed node scans are now persisted as SQLite node caches so a later rerun can skip the expensive first pass without reloading millions of nodes into memory.
- The second pass flushes road-cell GeoJSON files incrementally instead of waiting until the very end, so partial output survives app shutdowns or crashes.
