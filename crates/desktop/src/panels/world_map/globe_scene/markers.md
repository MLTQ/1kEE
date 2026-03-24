# markers.rs

## Purpose
Renders interactive globe markers and their visual effects. This includes vessels, flights, events, cameras, and the connector/selection styling used for globe hit testing.

## Components

### `draw_ships`
- **Does**: Paints AIS vessel markers with heading-aware glyphs and selection rings
- **Interacts with**: `MovingTrack` in `model.rs`, `projection.rs`

### `draw_flights`
- **Does**: Paints ADS-B flight markers with category-based theme colors and heading arrows
- **Interacts with**: `FlightTrack` in `model.rs`, theme flight color helpers

### `draw_event_marker`
- **Does**: Draws the animated event beam and ground-strike marker
- **Interacts with**: `EventRecord` in `model.rs`, `ProjectedPoint` from `mod.rs`

### `draw_camera_marker` / `draw_camera_links`
- **Does**: Paint nearby-camera markers and their relationship lines to the selected event
- **Interacts with**: `theme.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `globe_scene/mod.rs` | Marker painters do not mutate shared state and only emit shapes | Adding stateful side effects or changing selection semantics |
| `world_map.rs` | Event/camera marker positions still match what the renderer draws | Decoupling hit-test positions from painted positions |
