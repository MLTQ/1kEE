# marching_squares.rs

## Purpose
Pure-Rust contour extraction for raster terrain tiles. It exists to avoid shelling out to GDAL for every contour tile when the raster samples are already available in-process.

## Components

### `NativeSrtmSampler`
- **Does**: Loads SRTM GeoTIFF tiles on demand and bilinearly samples them.
- **Interacts with**: `build_tile_contours`, SRTM tile files on disk.

### `build_tile_contours_with_sampler`
- **Does**: Runs the generic contour pipeline against any `(lat, lon) -> elevation` sampler closure.
- **Interacts with**: `extract_segments`, `chain_segments`.
- **Rationale**: Lets Earth and lunar pipelines share the same marching-squares engine while sourcing samples from different raster caches.

### `build_tile_contours`
- **Does**: SRTM-specific wrapper around the generic sampler-based contour builder.
- **Interacts with**: `NativeSrtmSampler`.

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `contours.rs` native engine | Returned contour/coastline geometry matches the shared SQLite schema | Changing contour ordering or coordinate encoding assumptions |
| Lunar builder | Generic sampler hook can be fed from cached lunar raster chunks | Removing `build_tile_contours_with_sampler` or changing sampling semantics |

## Notes
- Sampling stays bilinear so the native path tracks the smoothness of the GDAL-derived raster path reasonably closely.
