# srtm_stream.rs

## Purpose

Provides sampled elevation from raw SRTM tiles. It retains a small LRU of
decoded one-degree rasters and offers a nonblocking preload path for local UI
markers, while background layer builders can continue to request exact samples.

## Components

### `sample_elevation_m`

- **Does**: Returns an exact bilinearly interpolated terrain elevation,
  synchronously decoding a tile when necessary.
- **Interacts with**: Background road, water, building, tree, and raster
  builders.
- **Rationale**: Those workers need exact source elevation before committing
  geometry and are already off the UI thread.

### `peek_elevation_m` / `request_elevation_preload`

- **Does**: Reads only an already decoded tile or schedules one deduplicated
  background TIFF/GDAL load, then requests a repaint.
- **Interacts with**: Local terrain marker rendering.
- **Rationale**: Marker paint must not block on a full GeoTIFF decompression
  or `gdal_translate` subprocess when a viewport crosses a tile boundary.

### `TileCache`

- **Does**: Keeps a bounded LRU, permanent-miss set, and path-keyed loading
  set.
- **Interacts with**: Both sampling APIs and preload workers.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| Background map layers | `sample_elevation_m` returns an exact value when the tile is readable | Making this API nonblocking |
| Local markers | Cache misses return promptly and schedule at most one load per path | Performing raster/GDAL work in `peek_elevation_m` |

## Notes

- A marker uses the existing zero-elevation fallback only until its preload
  completes; the next repaint restores its exact terrain-relative position.
- The loading set is intentionally shared with all marker requests and allows
  one raster preload at a time, so dense camera/event groups cannot duplicate
  or saturate the machine with TIFF/GDAL work.
