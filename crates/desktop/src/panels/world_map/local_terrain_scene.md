# local_terrain_scene.rs

## Purpose
Renders the high-zoom event terrain mode. This file exists to show one selected event area as a dedicated oblique 3D contour stack instead of trying to coerce the globe renderer into a local topography viewer.

## Components

### `paint`
- **Does**: Draws the local terrain frame, loads the streamed SRTM contour neighborhood around the current local viewport center, retains previously visited chunk geometry for the current terrain focus, renders height-separated contour slices, and returns marker positions for event/camera selection
- **Interacts with**: `AppModel` in `model.rs`, `contour_asset.rs`, `srtm_focus_cache.rs`, `globe_scene.rs`
- **Rationale**: Keeps local terrain rendering isolated from the globe renderer so both views can evolve independently

### `is_active`
- **Does**: Determines when the UI should switch from globe mode to local terrain mode
- **Interacts with**: `world_map.rs`, `terrain_assets.rs`

### `local_render_zoom`
- **Does**: Clamps the shared zoom state into the terrain renderer's usable range
- **Interacts with**: `camera.rs`, `paint`
- **Rationale**: Lets wheel zoom continue working inside local terrain mode without forcing the scene onto one fixed contour cache bucket

### `project_local`
- **Does**: Projects local east/north/elevation coordinates into an analyst-controlled oblique screen view
- **Interacts with**: `GlobeViewState` in `model.rs`, contour rendering, marker placement
- **Rationale**: Uses explicit height-separated contour geometry rather than a flat 2D overlay while still allowing orientation changes

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `world_map.rs` | `paint` returns the same `GlobeScene` marker contract used by globe mode | Changing marker semantics or return type |
| Terrain pipeline | `load_srtm_for_focus` returns local contour paths with `elevation_m` metadata | Removing contour elevations or changing point units |

## Notes
- This scene now derives its contour extent and interval from the shared zoom state, so entering local terrain starts wide and progressively tightens to smaller high-detail patches as the analyst keeps zooming in.
- Drag-driven camera rotation comes from `GlobeViewState`.
- Vertical contour separation now comes from the user-controlled `local_layer_spread` value in `GlobeViewState`, and that control only affects elevation offset, not the base terrain-plane tilt.
- Plain drag pans the local viewport center, which causes the terrain cache to stream across the surrounding region while any selected event marker remains at its true map position.
- The streamed neighborhood is intentionally wider again, so the local view keeps more surrounding landform context loaded around the viewport center before relying on panning and retention.
- The local scene keys chunk retention to the current terrain focus location, so already visited terrain buckets stay visible while panning and only reset when the analyst selects a different event or manual city focus.
- When the current streamed neighborhood is still being generated, the scene shows a bucket-level cache progress bar based on ready versus pending contour exports.
- Zooming back out below the local-mode threshold hands control back to the globe view.
