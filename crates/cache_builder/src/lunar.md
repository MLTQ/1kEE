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

### `build_lunar_contour_tiles`
- **Does**: Plans missing tiles for a bbox, ensures the needed source chunks exist, crops tile rasters from those chunks, contours them, and imports the output into SQLite.
- **Interacts with**: `contours.rs` cache DB helpers and manifest tables.

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| Desktop lunar contour loader | `lunar_focus_cache.sqlite` uses the shared contour manifest/tile schema | Renaming tables or changing tile bucket math |
| Desktop runtime GDAL path | `lunar_source_chunks/` is a persistent sibling of the terrain cache DB and uses the same chunk layout | Changing chunk naming/layout without updating runtime |
| CLI / job runner | `build_lunar_contour_tiles` returns readable progress/error strings | Changing return shape or progress semantics |

## Notes
- Chunk caches are keyed by zoom bucket because each zoom tier wants a different pixels-per-degree density.
- Chunk extents intentionally overlap the snapped center grid so every lunar tile fits inside one cached source chunk.
