# roads.rs

## Purpose
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
- **Does**: First pass over the planet file, retaining only nodes inside the expanded requested bbox
- **Interacts with**: `util.rs`
- **Rationale**: The downloaded planet file does not advertise `LocationsOnWays`, so the builder has to resolve node refs itself

### `collect_roads_by_cell`
- **Does**: Second pass over the planet file, filters `highway=*` ways, reconstructs polylines from retained nodes, and groups them into 1° cache cells
- **Interacts with**: `geojson.rs`, `util.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `main.rs` | `build_bbox_cache` returns `Result<(), String>` with readable failures | Changing the return contract |
| `job.rs` | progress updates use `RoadBuildProgress` with `stage`, `fraction`, and `message` fields | Renaming or removing progress fields |
| desktop road loader | emitted road classes and GeoJSON schema match the direct vector cache it already reads | Renaming road classes or changing the file format |
| future resumed builds | candidate-node checkpoints under `.builder_state/` are keyed by bbox and can be reused safely for the same request | Changing checkpoint naming/format without migration |

## Notes
- This first builder command still scans the planet twice for a bbox, which is acceptable for offline work and much better than doing it on the interactive UI thread.
- The current target is roads only. Water, buildings, and boundaries can be layered onto the same offline builder approach next.
- Completed node scans are now persisted as checkpoint files so a later rerun can skip the expensive first pass.
- The second pass flushes road-cell GeoJSON files incrementally instead of waiting until the very end, so partial output survives app shutdowns or crashes.
