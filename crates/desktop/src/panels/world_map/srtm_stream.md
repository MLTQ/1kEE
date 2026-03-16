# srtm_stream.rs

## Purpose
Streams SRTM GL1 land elevation tiles directly from disk for local globe relief sampling. This avoids preloading a massive global raster and establishes the runtime path for higher-resolution land terrain.

## Components

### `sample_normalized`
- **Does**: Resolves the SRTM mirror root, lazily loads the needed one-degree tile, caches a small working set, and returns normalized land elevation for a geographic point
- **Interacts with**: `terrain_assets.rs`, `terrain_raster.rs`, local SRTM GeoTIFF tiles
- **Rationale**: Keeps the renderer responsive while still using higher-resolution source data where land tiles exist

### `sample_elevation_m`
- **Does**: Returns sampled elevation in meters from the streamed SRTM tile cache
- **Interacts with**: `sample_normalized`
- **Rationale**: Local contour generation needs real meter values rather than normalized shading signals

### `SrtmTile`
- **Does**: Holds one decoded SRTM tile and provides bilinear elevation sampling
- **Interacts with**: `sample_normalized`

### `TileCache`
- **Does**: Remembers a small set of loaded tiles plus missing-tile lookups
- **Interacts with**: `OnceLock`, `Mutex`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `terrain_raster.rs` | SRTM sampling is lazy and cheap after first tile load | Making each sample hit the filesystem directly |
| Runtime terrain path | Tile names remain NASA-style one-degree `N48E002.tif` style | Changing tile naming assumptions without updating the resolver |

## Notes
- This loader treats SRTM as a land-first refinement layer; ocean and missing tiles still fall back to lower-resolution global assets.
- The cache is intentionally small because close-focus viewing should only touch a handful of neighboring tiles.
- Contour generation no longer depends on this loader; it now uses `srtm_focus_cache.rs` and external GDAL tools because the current in-process GeoTIFF decode path is not reliable enough for contour extraction.
