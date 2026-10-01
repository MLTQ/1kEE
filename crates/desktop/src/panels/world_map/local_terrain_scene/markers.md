# markers.rs

## Purpose

Draws local-terrain event and camera markers at terrain-relative elevations.
It anchors them to the displayed fill mesh when available, while keeping paint
responsive when an Earth-only raw SRTM fallback is needed.

## Components

### `marker_surface_elevation_m`

- **Does**: Prefers a supplied elevation sampled from the currently displayed
  fill mesh, applies the named glyph clearance, and uses nonblocking SRTM only
  as an Earth fallback.
- **Interacts with**: `local_terrain_scene/mod.rs` and `srtm_stream.rs`.
- **Rationale**: The fill mesh is an interpolated contour/GEBCO surface, so it
  is the only source that can guarantee the glyph lies on the pixels drawn.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| Local scene | Markers use its displayed fill-surface elevation when supplied | Sampling an unrelated terrain source while a fill mesh is visible |
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
