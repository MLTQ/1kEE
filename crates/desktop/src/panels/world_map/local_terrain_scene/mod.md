# mod.rs

## Purpose
Owns the high-zoom local terrain scene: camera layout, contour/overlay composition, terrain projection, and the scene-level tests that validate the local map stack against real contour cache data. It is the handoff target when the world map leaves the globe view.

## Components

### `render_local_terrain_scene`
- **Does**: Drives the local terrain renderer, assembles terrain/road/water/uploaded-layer overlays, and coordinates the split helper modules in this folder
- **Interacts with**: `contour_asset.rs`, `terrain_field.rs`, `road_layer.rs`, `water_layer.rs`, `projection.rs`, `ui_overlays.rs`

### Helper submodules (`projection`, `geography`, `markers`, `dissolve`, `ui_overlays`)
- **Does**: Split projection math, geographic drawing, marker rendering, transition effects, and HUD overlays out of the main scene entrypoint
- **Interacts with**: `render_local_terrain_scene` and world-map state in `AppModel`

### Local scene tests
- **Does**: Exercise the contour-loading/projection path against cached focus data so the local terrain stack keeps a working end-to-end sanity check
- **Interacts with**: `contour_asset::load_srtm_region_for_view`, egui layout helpers

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `world_map.rs` | This module remains the local-mode renderer entrypoint and can be switched to by zoom/focus state alone | Moving the local scene entrypoint or changing its model/context contract |
| Overlay renderers | Imported user layers, roads, water, and contours share the same local projection space | Changing coordinate transforms without updating overlay helpers |
| Scene tests | The local contour loader stays reachable through `contour_asset::load_srtm_region_for_view` for end-to-end validation | Renaming/removing that loader without updating the tests |

## Notes
- The scene-level tests are intentionally tolerant of missing local cache data: they return early when the shared focus cache is unavailable instead of making the suite depend on large fixture assets.
