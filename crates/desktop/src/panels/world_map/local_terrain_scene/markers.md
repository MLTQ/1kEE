# markers.rs

## Purpose

Draws local-terrain event and camera markers at terrain-relative elevations.
It anchors them to the displayed fill mesh when available, while keeping paint
responsive when an Earth-only raw SRTM fallback is needed.

## Components

### `draw_markers`

- **Does**: Projects and draws event beams and nearby camera markers in local
  terrain space.
- **Interacts with**: `projection.rs`, `srtm_stream.rs`, and local scene paint.

### `marker_elevation_m`

- **Does**: Uses a cached SRTM sample when available; otherwise starts a
  deduplicated preload and retains the existing ground-level fallback.
- **Interacts with**: `srtm_stream::peek_elevation_m` and
  `request_elevation_preload`.
- **Rationale**: A marker must never synchronously decompress a GeoTIFF or
  invoke GDAL on the egui paint thread.

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
