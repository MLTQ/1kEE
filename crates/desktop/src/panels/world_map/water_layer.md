# water_layer.rs

## Purpose
Builds and renders the local-terrain water overlay. It caches waterway and water-body geometry by tile coverage, enriches it with sampled elevation, and draws a lightweight themed line representation over the local terrain scene.

## Components

### `invalidate_water_cache`
- **Does**: Clears the cached water geometry so the next draw reloads it for the current viewport
- **Interacts with**: `local_terrain_scene.rs`, water-layer toggles

### `water_cache_building`
- **Does**: Reports whether a background water-cache build is still running
- **Interacts with**: `world_map.rs`

### `draw_water`
- **Does**: Detects stale tile coverage, launches background geometry plus elevation-enrichment work, projects features into screen space, and draws the visible overlay
- **Interacts with**: `osm_ingest.rs`, `srtm_stream.rs`, `theme.rs`, `local_terrain_scene.rs`
- **Rationale**: Keeps large polygon/river decoding and terrain sampling off the render thread

### `ElevatedWater`
- **Does**: Stores one water feature with pre-sampled elevation data for later projection
- **Interacts with**: `draw_water`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `local_terrain_scene.rs` | Water drawing can be requested per frame without synchronous SQLite or elevation work on the UI thread | Reintroducing blocking cache enrichment |
| `world_map.rs` | `water_cache_building` remains a cheap readiness signal | Removing or changing build-state semantics |

## Notes
- Like the road layer, water enrichment now happens in the background cache build instead of on the first render frame.
- Waterways and shorelines now keep per-vertex elevation sampling and full projected linework, so the overlay retains its terrain-wrapped shape while the expensive prep still happens off the render thread.
