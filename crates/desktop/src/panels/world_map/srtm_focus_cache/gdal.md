# gdal.rs

## Purpose
Owns the desktop-side GDAL pipelines for terrain assets: SRTM focus contours, GEBCO derived assets, and SLDEM2015 lunar tiles. It isolates external process orchestration and temp-file handling from the render code.

## Components

### International hosted contour builds
- The historical `build_threedep_contours` entrypoint also dispatches eligible
  windows through `elevation_sources`. Exact core/halo bounds, download/staging
  admission, processing permits, progress and clipped SQLite import stay shared.
- Confirmed no-coverage keeps SRTM fallback active instead of importing an empty
  fine tile. Service and decoder errors are logged and retryable. Intermediate
  provider rasters are cleaned even on failed builds.

### `run_command_with_timeout`
- **Does**: Runs GDAL tools with timeout and shutdown-aware cancellation.
- **Interacts with**: `active_children`, app shutdown flow.

### `build_focus_contours`
- **Does**: Builds one SRTM focus tile from one or more source GeoTIFF tiles and imports it into SQLite.
- **Interacts with**: `db.rs` import helpers.

### `bounds_have_srtm_source`
- **Does**: Reports whether any SRTM source file overlaps a bucket's bounds,
  short-circuiting on the first hit.
- **Interacts with**: `builders.rs` sourceless-tile memo.
- **Rationale**: `build_focus_contours` returns `None` both when no source
  covers the bounds and when GDAL genuinely fails. Callers that must not retry
  ocean forever need to distinguish the two before scheduling any work.

### Lunar source chunk helpers
- **Does**: Build and reuse persistent `lunar_source_chunks/` GeoTIFFs under the terrain cache root.
- **Interacts with**: `build_lunar_contour_tile`, `gdal_translate`.
- **Rationale**: Moon Mode tiles overlap heavily; chunk reuse prevents repeated reads from the 22 GB SLDEM JP2.
- **Notes**: Chunk creation is deduplicated across background tile threads, and each build uses a unique temp filename to avoid same-chunk races.

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

- Temporary contour/coastline GeoPackages disable their unused spatial index
  and synchronous flushes. These options apply only to disposable GDAL output;
  the persistent cache keeps WAL/NORMAL transactions. Failed output is never
  imported after a failed GDAL exit. No newer GDAL transaction flag is required.
- 3DEP takes an explicit download/staging permit from `builders.rs`. A valid
  saved raster releases its download slot immediately, then waits for shared
  processing capacity. Starting processing releases the staging slot. Both
  transitions invalidate manifest scheduling and request repaint so available
  capacity refills without waiting for the manifest refresh timeout.
  Waiting is shutdown-aware, and every return releases owned permits.

- Tile builds publish completed source and contour-generation stages through
  `progress.rs`. Hosted rasters also report actual response bytes when the
  server provides a length; source completion requires a validated saved TIFF.
  GDAL stages without a progress count hold their last completed milestone.

- Opt-in stage timers measure SRTM source preparation, hosted downloads,
  processing waits, contour generation, coastlines, and total build time.
  `pipeline_bench.rs` contains ignored real-data concurrency comparisons.
  Profiling suppresses child stdout progress dots to keep timing records intact;
  child stderr remains visible for GDAL errors.
- The source milestone completes before waiting for a processing permit.
  Imports receive that attempt's handle explicitly across resets and retries.

- Every tile build owns a `TempTileCleanup` guard; failed source, GDAL, or
  import stages no longer leave temporary files filling the volume. Coastline
  staging shares the unique attempt prefix and is scoped separately.
- `storage.rs` requires 1 GiB headroom before source work and contouring. It runs
  on workers only; failures follow existing backoff and cleanup.

- `gdal_process.rs` drains diagnostics, stops/reaps fatal storage or contour
  writers promptly, and rejects fatal write errors even with a zero exit.
  Prints the first eight stderr lines, the first fatal write error even after
  that limit, and a suppression count per command.

- Earth SRTM/3DEP builds derive a CoreTile from the original spec/key: a disjoint
  core plus two raster pixels on each side. Sampling density and elevation
  intervals stay approximately unchanged; the halo is clipped before import.
  gdalwarp and hosted requests use f64 bounds. No existing tile is rewritten.

- `international_build_tests.rs` exercises a complete Tokyo source-to-contour
  cache build and verifies source rasters are removed after publication.
