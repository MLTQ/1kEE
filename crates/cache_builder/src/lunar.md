# lunar.rs

## Purpose
Offline builder for SLDEM2015 lunar contour tiles. It fills `lunar_focus_cache.sqlite` ahead of time so Moon Mode can load contours from SQLite instead of repeatedly extracting windows from the raw JP2 at runtime.

## Components

### `LunarSpec` / `all_lunar_specs`
- **Does**: Defines the lunar zoom buckets, raster sizes, and contour intervals.
- **Interacts with**: Desktop `srtm_focus_cache::zoom::lunar_spec_for_zoom`.
- **Rationale**: Builder and runtime must agree exactly on tile geometry or the desktop will miss prebuilt tiles.

### `SourceChunk` helpers
- **Does**: Maps many overlapping contour tiles onto a persistent `lunar_source_chunks/` raster cache and lazily builds each chunk once from the JP2.
- **Interacts with**: GDAL `gdal_translate`, `build_lunar_contour_tiles`.
- **Rationale**: The same 22 GB JP2 was previously re-read for every tile. Chunk reuse moves the repeated work onto much smaller tiled GeoTIFFs.

### `LunarChunkSampler`
- **Does**: Loads cached lunar source chunks as GeoTIFF rasters and bilinearly samples them in-process.
- **Interacts with**: `marching_squares::build_tile_contours_with_sampler`.
- **Rationale**: Once a chunk is decoded from JP2, contour extraction should stay inside Rust instead of spawning `gdal_contour` per tile.

### `build_lunar_contour_tiles`
- **Does**: Plans missing tiles for a bbox, ensures the needed source chunks exist, then runs native marching-squares workers over those chunks and writes results into SQLite through one writer thread.
- **Interacts with**: `marching_squares.rs`, shared contour cache schema.

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| Desktop lunar contour loader | `lunar_focus_cache.sqlite` uses the shared contour manifest/tile schema | Renaming tables or changing tile bucket math |
| Desktop runtime GDAL path | `lunar_source_chunks/` is a persistent sibling of the terrain cache DB and uses the same chunk layout | Changing chunk naming/layout without updating runtime |
| CLI / job runner | `build_lunar_contour_tiles` returns readable progress/error strings and reports chunk-prep/native-build stages | Changing return shape or progress semantics |

## Notes
- Chunk caches are keyed by zoom bucket because each zoom tier wants a different pixels-per-degree density.
- Chunk extents intentionally overlap the snapped center grid so every lunar tile fits inside one cached source chunk.
- The builder now only uses GDAL for the cold-path JP2 decode. Contour extraction itself is parallel native code.
