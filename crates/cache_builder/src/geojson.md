# geojson.rs

## Purpose
Owns the offline road vector-cell cache file format. It writes and merges the same per-cell GeoJSON files the desktop app already knows how to stream.

## Components

### `ensure_cache_dir` / `vector_cell_path`
- **Does**: Resolves and creates the output directory plus stable per-cell filenames
- **Interacts with**: `roads.rs`

### `merge_write_cells`
- **Does**: Merges newly built road polylines into existing per-cell GeoJSON cache files
- **Interacts with**: `roads.rs`
- **Rationale**: Re-running the builder for overlapping bounds should extend a cell cache, not replace it destructively

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `roads.rs` | can write `RoadPolyline` groups directly into the desktop-compatible cell cache | Changing the file naming or GeoJSON properties |
| desktop road loader | cache files use `way_id`, `class`, `name`, and LineString coordinates | Renaming fields or changing geometry shape |

## Notes
- This is intentionally GeoJSON-first for inspectability. A future denser format can replace it later if both the builder and desktop loader move together.
