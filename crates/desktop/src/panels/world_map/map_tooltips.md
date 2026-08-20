# map_tooltips.rs

## Purpose

Draws non-interactive hover cards for projected world-map markers. Tooltip
lookup uses the exact marker ids and screen positions returned by the active
scene, keeping labels aligned with painted and clickable glyphs.

## Components

### `draw_event_hover_tooltip`

- **Does**: Shows severity, title, and location for a hovered event.
- **Interacts with**: `GlobeScene::event_markers` and `AppModel::events`.

### `draw_camera_hover_tooltip`

- **Does**: Shows reachability, label, provider, feed type, and approximate
  coordinates for a hovered camera dot, including the click-to-open-feed cue.
- **Interacts with**: `GlobeScene::camera_markers` and `AppModel::cameras`.

### `draw_ship_hover_tooltip` / `draw_flight_hover_tooltip`

- **Does**: Shows compact live-track metadata when no corresponding detail
  panel is already selected.
- **Interacts with**: live track snapshots and theme category colors.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| `world_map.rs` | Hover helpers are cheap, non-mutating, and safe to call every frame | Performing network or model mutations |
| Scene renderers | Tooltip hit targets use the same marker positions returned for click handling | Reprojecting markers independently |
