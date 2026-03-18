# camera.rs

## Purpose
Handles direct globe interaction and chooses level-of-detail settings from the current zoom. This keeps camera input and rendering-density policy out of the scene renderer itself.

## Components

### `GlobeLod`
- **Does**: Describes how dense the wireframe, contour layering, and sampling passes should be for the current zoom
- **Interacts with**: `globe_scene.rs`

### `apply_interaction`
- **Does**: Applies drag orbit in globe mode, drag pan in local terrain mode, `Ctrl`/`Shift` drag rotation in local terrain mode, wheel zoom, and idle auto-spin behavior to `GlobeViewState`
- **Interacts with**: `GlobeViewState` in `model.rs`, `egui::Response`

### `lod`
- **Does**: Maps zoom ranges to rendering density and altitude exaggeration
- **Interacts with**: `globe_scene.rs`
- **Rationale**: Lets the scene renderer become more detailed as the analyst zooms in without hard-coding thresholds there, while also thinning the global wireframe at close zoom so local terrain contours remain readable

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `world_map.rs` | `apply_interaction` mutates the persistent globe state in place | Changing the entrypoint signature |
| `globe_scene.rs` | `lod` returns stable semantic density settings | Renaming/removing `GlobeLod` fields |

## Notes
- Zoom now spans globe navigation, a globe-to-local overlap band, and a broader local-terrain range up to `20x`, so wheel input continues to tighten or widen terrain detail instead of freezing once local mode is active.
- In local terrain mode, wheel zoom now changes the visible terrain span continuously even when the underlying contour cache stays on the same LOD bucket for a while.
- High zoom intentionally reduces global latitude/longitude line density so real terrain contours can dominate the scene.
- Local terrain mode uses its own yaw/pitch pair so analysts can rotate the contour stack without disturbing the globe camera.
- Local terrain mode also maintains its own viewport center, which is updated by plain drag using the current terrain half-extent so contour cache requests can stream across the wider region at any local zoom level.
- Globe mode now allows near-polar orbiting up to about `±87.7°` latitude instead of stopping around `±63°`, so analysts can inspect Greenland, Iceland, Antarctica, and the high Arctic directly.
