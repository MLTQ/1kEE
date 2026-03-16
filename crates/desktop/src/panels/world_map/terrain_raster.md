# terrain_raster.rs

## Purpose
Provides layered terrain sampling for the globe renderer. It now prefers streamed SRTM land tiles when available and falls back to the derived GEBCO runtime raster for global coverage.

## Components

### `TerrainRaster`
- **Does**: Holds the normalized GEBCO preview raster in memory
- **Interacts with**: `globe_scene.rs`

### Cached raster loading
- **Does**: Lazily loads and caches the derived runtime raster for the currently selected root
- **Interacts with**: local filesystem, `OnceLock`, `Mutex`
- **Rationale**: Prevents repeated image decode work in the render loop

### `sample_normalized`
- **Does**: Samples normalized terrain height by latitude/longitude with bilinear interpolation for the selected root
- **Interacts with**: `globe_scene.rs`, `srtm_stream.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `globe_scene.rs` | Sampling is cheap and deterministic after first load | Making sampling allocate or perform blocking IO repeatedly |
| Terrain pipeline | Runtime asset stays at `Derived/terrain/gebco_2025_preview_4096.png` or an equivalent known path | Renaming/moving the runtime raster without updating loader logic |

## Notes
- This module now acts as a source multiplexer: SRTM first for land detail, GEBCO preview second for global fallback.
- The GEBCO path still intentionally consumes the derived PNG, not the raw GeoTIFF.
