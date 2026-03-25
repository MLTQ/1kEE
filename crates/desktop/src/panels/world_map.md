# world_map.rs

## Purpose
Implements the main geographic canvas for the demo. The current version wraps an interactive 3D globe scene with drag orbit, wheel zoom, contour-heavy overlays, and preserved event/camera click flow at the panel boundary.

## Components

### `render_world_map`
 - **Does**: Draws the globe panel, renders the top layer bar, applies pointer interaction to the persistent camera state, delegates rendering to `globe_scene.rs`, handles click-based selection, and shows event hover tooltips
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
- The old descriptive text band at the top of the map has been replaced by a layer bar that owns coastline and major/minor road toggles and uses those road toggles to kick off focused OSM ingest.
- When the selected event carries a Factal payload, the layer bar now exposes a `Brief` button that opens the dedicated Factal detail window.
- If the low-zoom globe coastline cache is missing, enabling coastlines now kicks off a background GEBCO-derived bootstrap instead of leaving the globe permanently blank on a fresh machine.
- The terrain path now prefers streamed SRTM land tiles, and contour overlays are generated into a shared SQLite-backed GDAL focus cache off the UI thread instead of one file per streamed bucket.
- The globe now has a zoom overlap band where local terrain fades/scales in before the full local scene takes over, which makes the globe-to-local handoff much less abrupt.
- Manual globe mode no longer forces a steady repaint loop. The panel only free-runs when auto-spin is on or when local terrain cache work is still pending.
- At high zoom the panel now hands off from the globe renderer to a dedicated selected-event local terrain scene with a fixed oblique camera.
- At high zoom the panel can also follow a manual city focus from the terrain library, which is useful for terrain inspection and precompute validation away from the seeded event list.
- In local terrain mode, plain drag now pans the streamed viewport and `Ctrl`/`Shift` drag rotates the terrain camera independently from the globe orbit state.
- Local terrain mode now uses a footer control strip below the map for layer spread, terrain zoom readout, and a contour color legend instead of placing those controls over the rendered scene.
- That footer now exposes a much wider `LAYER SPREAD` range (`0.15..=100.0`), so operators can intentionally reintroduce extremely strong vertical exaggeration after the projection was made zoom-stable.
- The same footer now also explains the road overlay palette so enabling road layers does not introduce unexplained linework, and it uses semantic labels instead of hard-coded color names so alternate themes read correctly.
- Major/minor road toggles now act as draw filters over one shared local road cache. Toggling a class off should no longer force a full road reload or temporarily blank the other class.
- `world_map.rs` now pins `globe_scene` and `srtm_focus_cache` to their split `mod.rs` entrypoints explicitly so the modularized implementations build cleanly without colliding with the legacy single-file versions.
- While a road layer is enabled, the panel now keeps repainting during active OSM jobs and re-queues focused road imports around the current terrain focus / local viewport instead of leaving the renderer pointed at a stale older region.
- Mock geography has been removed; if the terrain pipeline has no real data to show for a view, the globe stays minimal.
- Preserve the same event-selection and nearby-camera interaction semantics if the renderer is upgraded again.
