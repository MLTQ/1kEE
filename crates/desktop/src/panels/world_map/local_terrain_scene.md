# local_terrain_scene.rs

## Purpose
Renders the high-zoom event terrain mode. This file exists to show one selected event area as a dedicated oblique 3D contour stack instead of trying to coerce the globe renderer into a local topography viewer.

## Components

### `paint`
 - **Does**: Draws the local terrain frame, loads the streamed SRTM contour neighborhood around the current local viewport center, retains previously visited chunk geometry for the current terrain focus, renders height-separated contour slices, optionally drapes focused OSM road polylines over that terrain, and returns marker positions for event/camera selection
- **Interacts with**: `AppModel` in `model.rs`, `contour_asset.rs`, `srtm_focus_cache.rs`, `globe_scene.rs`
- **Rationale**: Keeps local terrain rendering isolated from the globe renderer so both views can evolve independently

### `paint_transition_overlay`
- **Does**: Draws a scaled/faded local-terrain contour overlay during the zoom overlap band before full local mode takes over
- **Interacts with**: `world_map.rs`, `contour_asset.rs`
- **Rationale**: Smooths the handoff from globe-scale rendering to event-local terrain without forcing an abrupt scene swap at one zoom threshold

### `is_active`
- **Does**: Determines when the UI should switch from globe mode to local terrain mode
- **Interacts with**: `world_map.rs`, `terrain_assets.rs`

### `local_render_zoom`
- **Does**: Clamps the shared zoom state into the terrain renderer's usable range
- **Interacts with**: `camera.rs`, `paint`
- **Rationale**: Lets wheel zoom continue working inside local terrain mode without forcing the scene onto one fixed contour cache bucket, while still allowing a much closer local inspection range than the earlier `12x` ceiling

### `visual_half_extent_for_zoom`
- **Does**: Maps the shared zoom value onto a continuously interpolated local-terrain half-span
- **Interacts with**: `camera.rs`, contour projection, legend text
- **Rationale**: Decouples the analyst’s camera motion from the coarser contour-cache LOD bands so each wheel notch moves the camera smoothly instead of only changing the scene at bucket boundaries

### `transition_progress`
- **Does**: Maps the shared zoom state onto the globe-to-local overlap band as a normalized `0..1` blend factor
- **Interacts with**: `world_map.rs`

### `has_pending_cache`
- **Does**: Reports whether the current streamed local terrain neighborhood still has pending contour-cache buckets
- **Interacts with**: `world_map.rs`, `srtm_focus_cache.rs`
- **Rationale**: Lets the map panel request idle repaints only while terrain work is actively progressing instead of redrawing forever in manual mode

### `project_local`
- **Does**: Projects local east/north/elevation coordinates into an analyst-controlled oblique screen view
- **Interacts with**: `GlobeViewState` in `model.rs`, contour rendering, marker placement
- **Rationale**: Uses explicit height-separated contour geometry rather than a flat 2D overlay while still allowing orientation changes

### `marker_elevation_m`
- **Does**: Samples SRTM elevation at an event or camera location and adds a small marker lift above the terrain surface
- **Interacts with**: `srtm_stream.rs`, `draw_markers`
- **Rationale**: Keeps Factal event pins and nearby-camera markers visually above the contour stack instead of leaving them buried at a flat placeholder altitude

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `world_map.rs` | `paint` returns the same `GlobeScene` marker contract used by globe mode | Changing marker semantics or return type |
| Terrain pipeline | `load_srtm_for_focus` returns local contour paths with `elevation_m` metadata | Removing contour elevations or changing point units |

## Notes
- This scene now derives its contour extent and interval from the shared zoom state, so entering local terrain starts wide and progressively tightens to smaller high-detail patches as the analyst keeps zooming in.
- The contour data still comes from discrete cache LOD bands, but the visible terrain span now interpolates continuously across zoom so the view can move smoothly between those bands.
- Before full local mode activates, the same shared cache is used to fade and scale a local terrain overlay in over the globe across a zoom overlap band instead of jumping directly between unrelated views.
- Drag-driven camera rotation comes from `GlobeViewState`.
- Vertical contour separation now comes from the user-controlled `local_layer_spread` value in `GlobeViewState`, and that control only affects elevation offset, not the base terrain-plane tilt.
- The operator-facing spread control is intentionally allowed to reach very large values for dramatic sci-fi exaggeration, even though the default remains at the neutral `1.0` baseline.
- Elevation is now normalized against the current local terrain span before projection, so relief exaggeration stays much more stable across zoom levels instead of mountains changing aspect ratio just because the camera zoom changed.
- Because that normalization made the default view more physically believable, the operator-facing spread control is now intentionally allowed to go much higher so dramatic sci-fi exaggeration remains available when desired.
- Plain drag pans the local viewport center, which causes the terrain cache to stream across the surrounding region while any selected event marker remains at its true map position.
- The streamed neighborhood is intentionally wider again, so the local view keeps more surrounding landform context loaded around the viewport center before relying on panning and retention.
- The local scene keys chunk retention to the current terrain focus location, so already visited terrain buckets stay visible while panning and only reset when the analyst selects a different event or manual city focus.
- Those streamed buckets now come from one shared SQLite tile cache, so panning no longer depends on the filesystem acting like the cache index.
- Major and minor roads now render only in local terrain mode, using the shared `osm_runtime.sqlite` tile store and the same viewport-center projection as the contour stack.
- When the current streamed neighborhood is still being generated, the scene shows a bucket-level cache progress bar based on ready versus pending contour exports.
- Zooming back out below the local-mode threshold hands control back to the globe view, but the overlap band keeps a faint terrain overlay visible for a while so that retreat is visually continuous.
- Event and camera markers now sample the same SRTM terrain source and float slightly above the local surface so live Factal points do not disappear under high-relief contour stacks.
