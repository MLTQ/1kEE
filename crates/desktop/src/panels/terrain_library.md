# terrain_library.rs

## Purpose
Embed the city search, terrain focus, precompute queue and OSM job controls in
Settings → Terrain Library. Keep background job advancement independent of UI.

## Components
- `tick`: advances terrain/OSM workers and requests progress repaints. Called by
  the app every frame, including cinematic and Focus capture with settings closed.
- `render`: city selection/focus, explicit queue actions, download progress and
  OSM inventory inspection, only while the Terrain Library section is visible.
- Job row helpers retain progress and completion information.

## Contracts
- Existing queue actions and selected-city state are preserved.
- OSM worker gating still requires an enabled road/water layer.
- Library visibility cannot stall jobs; rendering cannot open a second window.
- SQLite inventory refresh stays confined to visible library content.
