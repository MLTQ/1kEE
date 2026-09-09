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
| Background map layers | `sample_elevation_m` returns an exact value when the tile is readable, and **the same value for the same point regardless of cache state** | Making this API nonblocking, or letting it pick its source by what happens to be resident |
| Local markers | Cache misses return promptly and schedule at most one load per path | Performing raster/GDAL work in `peek_elevation_m` |

## Notes

- Both entry points prefer USGS 3DEP 1 m, but through **different** calls, and
  the difference matters.
  - `sample_elevation_m` uses `threedep::blocking_elevation_m`, which downloads
    the covering chunk if necessary. Its callers bake one elevation per vertex
    into cached road, water, building and tree geometry, so a sampler that
    answered from 3DEP or SRTM depending on what was resident would freeze the
    difference between two terrain models into those outlines. It briefly did:
    roads came out scattered across tens of metres of elevation, adjacent
    vertices disagreeing, because each one resolved against whichever source
    the 6-entry chunk cache happened to hold at that instant.
  - `peek_elevation_m` uses `threedep::peek_elevation_m` and never blocks. A
    marker is a single point that re-resolves every frame, so it converges on
    the finer source without ever recording a mixed result.
- `load_tile_via_gdal` passes an explicit `.bil` output path. The EHdr driver
  writes its raw band to exactly the path given and derives the header by
  swapping the extension, so an extensionless stem produced a binary the reader
  never found and silently failed the fallback.
- A marker uses the existing zero-elevation fallback only until its preload
  completes; the next repaint restores its exact terrain-relative position.
- The loading set is intentionally shared with all marker requests and allows
  one raster preload at a time, so dense camera/event groups cannot duplicate
  or saturate the machine with TIFF/GDAL work.
