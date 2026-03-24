# mod.rs

## Purpose
Defines the OSM ingest subsystem’s public surface: source discovery, job queue APIs, runtime inventory, and road/water loading helpers. It is the boundary between the app and the underlying planet/overpass/vector-cache implementations.

## Components

### Re-exported queue + inventory API
- **Does**: Exposes runtime store setup, job queue entrypoints, and inventory/status queries
- **Interacts with**: `job_dispatch.rs`, `inventory.rs`, `db.rs`

### `load_roads_for_bounds`
- **Does**: Loads road geometry for the current view, now preferring focused vector-cell GeoJSON cache files before falling back to the SQLite tile store
- **Interacts with**: `roads_vector_cache.rs`, `roads_global.rs`
- **Rationale**: Lets focused road imports become visible as soon as their vector cells are extracted, instead of waiting on the heavier SQLite replay path

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `road_layer.rs` | `load_roads_for_bounds` returns normalized `RoadPolyline` data regardless of the backing cache format | Changing the return type or removing the vector-cache preference |
| `header.rs` / status UI | Queue/inventory re-exports remain stable | Renaming re-exported functions |
| `roads_osmium.rs` | Feature-kind constants and focus-note prefixes stay stable | Renaming the constants without updating focused importers |

## Notes
- Focused roads now have two cache layers: durable `.osm.pbf` cell extracts and directly streamable GeoJSON road cells. The old SQLite tile store still exists for global/legacy callers and as a fallback.
