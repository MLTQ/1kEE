# gdal.rs

## Purpose
Owns the desktop-side GDAL pipelines for terrain assets: SRTM focus contours, GEBCO derived assets, and SLDEM2015 lunar tiles. It isolates external process orchestration and temp-file handling from the render code.

## Components

### `run_command_with_timeout`
- **Does**: Runs GDAL tools with timeout and shutdown-aware cancellation.
- **Interacts with**: `active_children`, app shutdown flow.

### `build_focus_contours`
- **Does**: Builds one SRTM focus tile from one or more source GeoTIFF tiles and imports it into SQLite.
- **Interacts with**: `db.rs` import helpers.

### Lunar source chunk helpers
- **Does**: Build and reuse persistent `lunar_source_chunks/` GeoTIFFs under the terrain cache root.
- **Interacts with**: `build_lunar_contour_tile`, `gdal_translate`.
- **Rationale**: Moon Mode tiles overlap heavily; chunk reuse prevents repeated reads from the 22 GB SLDEM JP2.

### `build_lunar_contour_tile`
- **Does**: Crops the requested contour tile from a cached lunar source chunk, runs `gdal_contour`, and imports the result into the lunar SQLite cache.
- **Interacts with**: `builders.rs`, `db.rs`.

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `builders.rs` | Lunar and SRTM builds return `Option<()>` and are safe to run off-thread | Changing return contract or making calls blocking on UI thread |
| `db.rs` | Imported tile geometry matches contour/coastline table schema | Changing import format or layer names |
| Offline cache builder | Desktop runtime uses the same lunar chunk layout and scaling assumptions | Diverging chunk naming/bounds/scaling |

## Notes
- Lunar chunk files are persistent rather than temp files so the offline builder and desktop runtime can share them.
- Chunk rasters are written as tiled, compressed GeoTIFFs because they are read many times after the initial JP2 decode.
