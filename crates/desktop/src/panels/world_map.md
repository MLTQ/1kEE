# world_map.rs

## Purpose
Implements the main geographic canvas for the demo. The current version wraps an interactive 3D globe scene with drag orbit, wheel zoom, contour-heavy overlays, and preserved event/camera click flow at the panel boundary.

## Components

### `render_world_map`
- **Does**: Draws the globe panel, applies pointer interaction to the persistent camera state, delegates rendering to `globe_scene.rs`, and handles click-based selection
- **Interacts with**: `AppModel` in `model.rs`, `apply_interaction` in `camera.rs`, `paint` in `globe_scene.rs`, theme helpers in `theme.rs`
- **Rationale**: Centralizes repaint policy so the globe does not redraw at full speed when the scene is idle

### `draw_focus_card`
- **Does**: Renders the selected-event HUD card over the globe
- **Interacts with**: `EventRecord` from `model.rs`
- **Rationale**: Keeps scene rendering separate from UI overlay copy and layout

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `app.rs` | This module owns the central map canvas render path | Moving the central view elsewhere |
| Future map work | Globe rendering can evolve behind `globe_scene.rs` without changing the rest of the app shell | Hard-coding projection logic into unrelated modules |

## Notes
- The panel now uses a persistent 3D camera and zoom-dependent LOD.
- The terrain path now prefers streamed SRTM land tiles, and contour overlays are generated into a shared SQLite-backed GDAL focus cache off the UI thread instead of one file per streamed bucket.
- At high zoom the panel now hands off from the globe renderer to a dedicated selected-event local terrain scene with a fixed oblique camera.
- At high zoom the panel can also follow a manual city focus from the terrain library, which is useful for terrain inspection and precompute validation away from the seeded event list.
- In local terrain mode, plain drag now pans the streamed viewport and `Ctrl`/`Shift` drag rotates the terrain camera independently from the globe orbit state.
- Local terrain mode now uses a footer control strip below the map for layer spread, terrain zoom readout, and a contour color legend instead of placing those controls over the rendered scene.
- Mock geography has been removed; if the terrain pipeline has no real data to show for a view, the globe stays minimal.
- Preserve the same event-selection and nearby-camera interaction semantics if the renderer is upgraded again.
