# roads_vector_cache.rs

## Purpose
Loads and maintains focused road caches. Prefers validated packed archive tiles,
then binary cells, then legacy GeoJSON. The import path still creates `.1kc` cells.

## Components
- `load_roads_for_bounds_from_vector_cache` / `read_cached_roads`: return ready
  roads and separate missing-cell bounds. Preserve baked heights and deduplicate IDs.
- `write_roads_to_vector_cells`: merges normalized roads and preserves existing
  elevation arrays when encoding the merged binary output.
- `ensure_cell_geojson_from_extract`: historical name; now writes binary cells.
- `load_all_roads_from_vector_cell`: binary-first reader with GeoJSON fallback.

## Contracts
- Partial coverage returns available data. Callers fill only missing regions;
  partial presence is not proof of complete coverage.
- Each load opens its own archive reader; no UI-thread disk access is introduced.
- Missing/changed/corrupt archive cells use legacy readers independently.
- IDs, geometry, names, classes and elevations stay aligned.
- Tests cover a ready cell beside a missing cell and preserved baked heights.
