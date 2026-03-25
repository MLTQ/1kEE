# road_layer.rs

## Purpose
Loads OSM road geometry from the runtime store, enriches it with terrain elevation off the render thread, and draws the visible local/regional road overlays. It now also acts as the guardrail against runaway egui geometry when a road import covers a dense urban/coastal area.

## Components

### `draw_roads`
- **Does**: Refreshes the cached elevated road set when tile coverage, root, or road data generation changes, then renders the current major/minor road overlays
- **Interacts with**: `osm_ingest`, `srtm_stream`, `local_terrain_scene`
- **Rationale**: Keeps SQLite/direct-cache reads and terrain sampling off the main render loop while still reacting to new OSM imports

### `draw_road_layer`
- **Does**: Projects cached elevated polylines into the local scene with stable per-layer point budgets
- **Interacts with**: `project_local` in `local_terrain_scene`

### `simplify_source_points`
- **Does**: Reduces per-road source vertices before elevation sampling so the background enrichment step stays bounded
- **Interacts with**: `ElevatedRoad::from_polyline`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `local_terrain_scene/mod.rs` | `draw_roads` renders without blocking the UI thread or exploding egui buffers | Removing the background cache or returning unbounded geometry |
| `world_map.rs` | `invalidate_road_cache` and `road_cache_building` describe the road overlay cache state | Renaming or removing those helpers |
| `osm_ingest` | `load_roads_for_bounds` returns canonicalized road polylines keyed by `way_id` | Changing the loaded shape type or dropping `way_id` dedupe |

## Notes
- The local road overlay now enforces both a per-road source simplification cap and separate per-layer point budgets. This intentionally trades some road detail for stability when the focused import covers a very dense region.
- The road cache always loads both major and minor classes together for the covered viewport. Layer toggles only decide what gets drawn, which keeps checkbox changes from blowing away the loaded road geometry.
- Major roads are rendered before minor roads and use their own reserved point budget so enabling minor roads cannot starve the major-road layer.
- Camera-dependent screen-space thinning was removed because it caused roads to pop in and out as the operator rotated the local scene.
