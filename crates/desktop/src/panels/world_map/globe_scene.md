# globe_scene.rs

## Purpose
Owns the interactive 3D globe rendering for the map panel. It projects geographic data into a perspective view, draws the contour-heavy wireframe language, and returns hit-testable marker positions to the panel wrapper.

## Components

### `GlobeScene`
- **Does**: Returns the visible event and camera marker screen positions after rendering
- **Interacts with**: `world_map.rs`

### `paint`
- **Does**: Draws the globe backdrop, 3D wireframe globe, low-detail global contour context, markers, links, and HUD legend
- **Interacts with**: `AppModel` in `model.rs`, `GlobeLod` from `camera.rs`, `terrain_raster.rs`, `terrain_field.rs`, theme helpers in `theme.rs`
- **Rationale**: Keeps globe navigation separate from the dedicated local terrain renderer

### `GlobeLayout`
- **Does**: Stores projection parameters for the current frame
- **Interacts with**: `project`, drawing helpers
- **Rationale**: Keeps zoom expressed as camera approach and perspective strength instead of scaling the globe mesh itself

### `ProjectedPoint`
- **Does**: Carries screen position, relative depth, and hemisphere visibility for one projected sample
- **Interacts with**: marker rendering and path drawing

### Projection and drawing helpers
- **Does**: Handle perspective globe projection, path splitting by visible hemisphere, backdrop/HUD rendering, and overlay styling
- **Interacts with**: `egui::Painter`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `world_map.rs` | `paint` returns visible marker positions suitable for click hit-testing | Changing return structure or marker semantics |
| `camera.rs` | `GlobeLod` drives scene density without hard-coded zoom thresholds here | Ignoring or removing LOD semantics |
| Future renderer upgrades | Projection and coastline/topography drawing are isolated here | Spreading 3D projection logic back into panel code |

## Notes
- The topographic surface now samples the derived GEBCO runtime raster when available and falls back to the procedural field otherwise.
- The globe renderer is intentionally not responsible for high-zoom terrain analysis anymore; that handoff now belongs to `local_terrain_scene.rs`.
- GEBCO contour fallback remains globe-only context.
- The current zoom path keeps the globe radius mostly stable and changes `camera_distance` instead, with a conservative zoom ceiling until a true local terrain-view handoff exists.
- Decorative backdrop sweep/ring effects were removed so the terrain overlay carries the scene.
- Mock geography has been removed entirely; if terrain data is unavailable, the globe stays visually sparse instead of inventing coastlines.
- If real topography/coastline datasets are added later, keep the same `paint` contract so the rest of the UI stays stable.
