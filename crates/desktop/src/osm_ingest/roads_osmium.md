# roads_osmium.rs

## Purpose
Focused-road importer that uses `osmium extract` to avoid reparsing the full planet file for every nearby revisit. It now turns focus cells into durable on-disk extracts so the expensive planet cut only happens once per 1° cell.

## Components

### `import_focus_roads_via_osmium`
- **Does**: Resolves which focus cells already have imported road tiles, which still need SQLite import, and which still need an on-disk extract, then coordinates the batch extract + per-cell reuse flow
- **Interacts with**: `job_dispatch.rs` focus-cell helpers, `roads_stream.rs`, `db.rs`
- **Rationale**: Keeps the current SQLite render store, but makes the osmium path pay the planet-extract cost once and reuse durable cell extracts afterward

### `extract_focus_cells_from_batch`
- **Does**: Splits one batched osmium extract into persistent per-cell `.osm.pbf` files under the runtime extract directory
- **Interacts with**: `run_osmium_extract` in `job_dispatch.rs`

### `import_cells_from_cache`
- **Does**: Replays cached per-cell extracts through the existing stream importer and marks those cells as imported in SQLite metadata
- **Interacts with**: `import_focus_roads_via_stream_scan` in `roads_stream.rs`, `mark_focus_cell_cached` in `job_dispatch.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `job_dispatch.rs` | Focused road jobs return a summary string and keep `road_data_generation` in sync | Changing the function signature or not bumping the road generation counter |
| `roads_stream.rs` | Cell extracts are valid `.osm.pbf` files it can scan like any other OSM source | Changing extract format without updating the stream importer |
| `road_layer.rs` | Focused imports eventually populate the shared SQLite road tile store for rendering | Removing the SQLite import step |

## Notes
- The durable cache is still `.osm.pbf` per focus cell, not GeoJSON/FlatGeobuf yet. That keeps the first change small and makes revisit speed much better without rewriting the renderer.
- Metadata-only cache entries are no longer sufficient; the importer now treats the on-disk cell extract as the durable artifact and recreates missing files when needed.
