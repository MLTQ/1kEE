# markers.rs

## Purpose

Draws local-terrain event and camera markers at terrain-relative elevations.
It samples the Earth elevation cache without blocking paint, then adds the
existing small clearance for the ground glyph.

## Components

### `marker_surface_elevation_m`

- **Does**: Samples cached Earth elevations and applies glyph clearance. A cache
  miss schedules background preload and temporarily uses sea level.
- **Interacts with**: `local_terrain_scene/mod.rs` and `srtm_stream.rs`.
- **Rationale**: Marker rendering must not decode rasters or fetch source data
  on the UI thread. No interpolated fill surface is accepted or retained.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| Local scene | Anchors use nonblocking Earth elevation samples plus clearance | Introducing blocking I/O or changing clearance |
| SRTM stream | Marker cache misses request only nonblocking preload work | Replacing background layer exact-sample behavior |

## Notes

- The transient Earth fallback is visually consistent with the pre-existing
  missing-terrain behavior; worker completion asks egui for a repaint
  automatically. Moon and Mars never initiate an Earth SRTM preload.

### Scaled event and webcam spires
Events retain their severity color and 110-point default height. Webcams use a
green 76-point light spire, small ground strike and selected ring. Scene-supplied
scale affects widths/rings as well as height. Events use `marker_style::draw_beam`; webcams use its shared `draw_camera_spire`.
The unused legacy `draw_markers` composition path was removed; the local scene
is the single source of marker visibility, ground projection and hit geometry.
