# roads_vector_cache.rs

## Purpose
Provides a directly streamable focused-road cache format on disk. Focused OSM road jobs can write per-cell GeoJSON files here so the renderer can load road geometry without waiting for the SQLite tile replay path.

## Components

### `vector_cache_dir` / `vector_cell_path`
- **Does**: Resolves the per-cell GeoJSON cache directory and stable filenames
- **Interacts with**: `roads_osmium.rs`, `mod.rs`

### `ensure_cell_geojson_from_extract`
- **Does**: Parses one focused `.osm.pbf` cell extract and writes a compact road `FeatureCollection` GeoJSON for that cell
- **Interacts with**: `roads_osmium.rs`, `util.rs`

### `write_roads_to_vector_cells`
- **Does**: Merges already-normalized road polylines into the direct per-cell GeoJSON cache, preserving existing cached roads in the same 1° cells
- **Interacts with**: `roads_overpass.rs`

### `load_roads_for_bounds_from_vector_cache`
- **Does**: Loads and filters cached road-cell GeoJSON files covering the current bounds, returning normalized `RoadPolyline` records
- **Interacts with**: `mod.rs`, `road_layer.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `roads_osmium.rs` | Cell extracts can be converted into durable GeoJSON road cells | Renaming the cache path format |
| `roads_overpass.rs` | Focused Overpass road results can be merged into the same on-disk vector-cell cache without losing prior roads in the cell | Making writes destructive instead of merge-based |
| `mod.rs` | Vector-cache loads can return `Some(Vec<_>)` even when the list is empty if matching cell files exist | Changing the return contract |
| `road_layer.rs` | Focused road loads become available as soon as matching GeoJSON cells exist on disk | Removing the direct vector-cache load path |

## Notes
- This is intentionally focused-road only. Global road bootstraps still use the SQLite tile store.
- GeoJSON is the first directly streamable format because it is easy to inspect and matches the user’s current fast local assets. A future follow-up can replace it with FlatGeobuf or another denser vector-cell format.
