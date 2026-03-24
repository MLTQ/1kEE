# roads_osmium.rs

## Purpose
Focused-road importer that uses `osmium extract` to avoid reparsing the full planet file for every nearby revisit. It now turns focus cells into durable on-disk extracts so the expensive planet cut only happens once per 1° cell.

## Components

### `import_focus_roads_via_osmium`
- **Does**: Resolves which focus cells already have direct GeoJSON road vectors, which still need only a vector build from an existing extract, and which still need an on-disk extract, then coordinates the batch extract + per-cell vector-cache flow
- **Interacts with**: `job_dispatch.rs` focus-cell helpers, `roads_vector_cache.rs`
- **Rationale**: Pays the planet-extract cost once per 1° cell and makes revisits hit directly streamable on-disk vectors instead of replaying geometry through SQLite

### `extract_focus_cells_from_batch`
- **Does**: Splits one batched osmium extract into persistent per-cell `.osm.pbf` files under the runtime extract directory
- **Interacts with**: `run_osmium_extract` in `job_dispatch.rs`

### `ensure_vector_cells`
- **Does**: Builds direct GeoJSON road cells from the durable per-cell extracts and bumps the road generation counter so the renderer can repaint immediately
- **Interacts with**: `roads_vector_cache.rs`, `job_dispatch.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `job_dispatch.rs` | Focused road jobs return a summary string and keep `road_data_generation` in sync | Changing the function signature or not bumping the road generation counter |
| `roads_vector_cache.rs` | Cell extracts are valid `.osm.pbf` files it can convert into direct GeoJSON road cells | Changing extract format without updating the vector cache builder |
| `road_layer.rs` | Focused imports eventually populate direct road-cell GeoJSON files for rendering | Removing the vector-cell cache without adding another direct path |

## Notes
- The focused osmium path no longer depends on `osm_focus_cell_cache` metadata rows to decide whether a cell is reusable. Real extract and GeoJSON files on disk are the source of truth.
- Focused road imports no longer fall back to the catastrophic full-planet stream scan; that fallback path is reserved for other flows and focused jobs now drop to Overpass if the osmium/vector-cache path fails.
