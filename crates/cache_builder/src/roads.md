# roads.rs

## Purpose
Implements the first offline OSM cache-building command: generate direct per-cell road GeoJSON caches from a requested bbox in `planet.osm.pbf`. This is the initial step toward moving heavy OSM parsing out of the desktop app.

## Components

### `build_bbox_cache`
- **Does**: Validates inputs, runs the two-pass planet scan, and writes the resulting road-cell caches
- **Interacts with**: `args.rs`, `geojson.rs`, `util.rs`

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
| desktop road loader | emitted road classes and GeoJSON schema match the direct vector cache it already reads | Renaming road classes or changing the file format |

## Notes
- This first builder command still scans the planet twice for a bbox, which is acceptable for offline work and much better than doing it on the interactive UI thread.
- The current target is roads only. Water, buildings, and boundaries can be layered onto the same offline builder approach next.
