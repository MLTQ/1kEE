# markers.rs

## Purpose

Draws local-terrain event and camera markers at terrain-relative elevations.
It keeps marker paint responsive while the raw SRTM cache prepares a newly
crossed raster tile.

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

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| Local scene | Marker positions update to exact terrain elevation after preload | Blocking local paint on raw SRTM I/O |
| SRTM stream | Marker cache misses request only nonblocking preload work | Replacing background layer exact-sample behavior |

## Notes

- The transient fallback is visually consistent with the pre-existing missing
  terrain behavior; worker completion asks egui for a repaint automatically.
