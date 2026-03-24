# road_layer.rs

## Purpose
Builds and renders the local-terrain road overlays. The file caches road geometry by tile coverage, enriches it with sampled elevation, and projects the result into the local terrain view without re-querying SQLite every frame.

## Components

### `invalidate_road_cache`
- **Does**: Clears the cached road geometry so the next render rebuilds it for the current viewport and visibility flags
- **Interacts with**: `layer_import.rs`, local terrain UI toggles

### `road_cache_building`
- **Does**: Reports whether a background road-cache build is still running
- **Interacts with**: `world_map.rs`

### `draw_roads`
- **Does**: Detects stale tile coverage, launches background road-geometry loads, lazily samples elevations, and draws major/minor road layers with theme-aware colors
- **Interacts with**: `osm_ingest.rs`, `srtm_stream.rs`, `theme.rs`, `local_terrain_scene.rs`
- **Rationale**: Keeps heavy I/O and elevation prep out of the per-frame hot path while still matching the active palette

### `draw_road_layer`
- **Does**: Projects elevated road polylines into screen space and submits the line shapes to egui
- **Interacts with**: `project_local` in `local_terrain_scene.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `local_terrain_scene.rs` | Road drawing can be requested per frame without blocking on SQLite when cache coverage is already valid | Reintroducing synchronous geometry loads into the draw path |
| `world_map.rs` | `road_cache_building` is cheap and `draw_roads` respects the major/minor visibility flags | Changing repaint-signalling behavior or ignoring the toggles |
| `theme.rs` | Major/minor road colors are provided as semantic tokens rather than hard-coded legend colors | Removing the theme helper integration |
