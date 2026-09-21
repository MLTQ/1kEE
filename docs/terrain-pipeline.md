# Terrain Pipeline

## Purpose
This document defines the first practical preprocessing path from the raw terrain datasets already checked into `Data/` to runtime-friendly assets for the 1kEE globe.

## Current Source Inventory

- `Data/GEBCO/gebco_2025_sub_ice_topo_geotiff/`: eight 90x90 degree GeoTIFF tiles at 15 arc-second resolution
- `Data/GEBCO/gebco_2025_tid_geotiff/`: matching TID provenance tiles
- `Data/natural_earth/GRAY_HR_SR_OB_DR/GRAY_HR_SR_OB_DR.tif`: global grayscale shaded relief
- `Data/srtm_gl1/`: partial SRTM 1 arc-second mirror with a VRT and individual tiles

## Recommended Runtime Strategy

1. Use GEBCO as the global base terrain.
2. Use Natural Earth as the fallback or artistic shaded-relief layer.
3. Use SRTM later as a regional refinement source for selected hotspots once the full mirror is available.

Do not read the raw global rasters directly from the UI thread at runtime. Preprocess them into derived assets first.

## Derived Assets To Produce

- `Derived/terrain/gebco_2025_global.vrt`
- `Derived/terrain/gebco_2025_preview_4096.tif`
- `Derived/terrain/gebco_2025_contours_200m.gpkg`
- `Derived/terrain/gebco_2025_contours_500m.gpkg`
- `Derived/terrain/natural_earth_relief_4096.tif`

These are enough for:
- low-cost globe shading
- real contour extraction
- future regional mesh or overlay generation

## Current Local Outputs

The following derived assets have already been generated in this repository:

- `Derived/terrain/gebco_2025_global.vrt`
- `Derived/terrain/gebco_2025_preview_4096.tif`
- `Derived/terrain/gebco_2025_preview_4096.png`
- `Derived/terrain/gebco_2025_contours_500m.gpkg`
- `Derived/terrain/gebco_2025_contours_200m.gpkg`
- `Derived/terrain/natural_earth_relief_4096.tif`
- `Derived/terrain/natural_earth_relief_4096.png`

The contour GeoPackages currently use a single layer named `contour`.

## GDAL Commands

Run from repo root.

### 1. Build a virtual global GEBCO mosaic

```bash
mkdir -p Derived/terrain
gdalbuildvrt Derived/terrain/gebco_2025_global.vrt Data/GEBCO/gebco_2025_sub_ice_topo_geotiff/*.tif
```

### 2. Produce a smaller preview raster for fast iteration

```bash
gdal_translate \
  -outsize 4096 2048 \
  -ot Int16 \
  -of GTiff \
  Derived/terrain/gebco_2025_global.vrt \
  Derived/terrain/gebco_2025_preview_4096.tif
```

### 3. Extract global contours from the downsampled preview first

```bash
gdal_contour \
  -i 200 \
  -a elevation_m \
  Derived/terrain/gebco_2025_preview_4096.tif \
  Derived/terrain/gebco_2025_contours_200m.gpkg
```

```bash
gdal_contour \
  -i 500 \
  -a elevation_m \
  Derived/terrain/gebco_2025_preview_4096.tif \
  Derived/terrain/gebco_2025_contours_500m.gpkg
```

### 4. Produce the depth-fill BIL grid for the globe texture layer

Used by `gebco_depth_fill.rs` to render ocean depth as a coloured texture on
the globe.  The 1440×720 size matches 0.25°/pixel — fast to load, sufficient
for a full-globe visualisation.

```bash
gdal_translate \
  -outsize 1440 720 \
  -ot Int16 \
  -of EHdr \
  Derived/terrain/gebco_2025_preview_4096.tif \
  Derived/terrain/gebco_depth_1440x720.bil
```

This produces both `gebco_depth_1440x720.bil` and its companion
`gebco_depth_1440x720.hdr` (written automatically by GDAL's EHdr driver).
The app expects little-endian Int16 values; `BYTEORDER I` in the .hdr confirms
this.  Positive values and the NODATA sentinel (−32767) are rendered
transparent so land shows the globe background.

### 6. Produce a Natural Earth fallback raster

```bash
gdal_translate \
  -outsize 4096 2048 \
  -of GTiff \
  Data/natural_earth/GRAY_HR_SR_OB_DR/GRAY_HR_SR_OB_DR.tif \
  Derived/terrain/natural_earth_relief_4096.tif
```

### 7. Produce runtime-friendly PNG assets for in-app sampling

```bash
gdal_translate \
  -scale -11000 9000 0 65535 \
  -ot UInt16 \
  -of PNG \
  Derived/terrain/gebco_2025_preview_4096.tif \
  Derived/terrain/gebco_2025_preview_4096.png
```

```bash
gdal_translate \
  -ot Byte \
  -of PNG \
  Derived/terrain/natural_earth_relief_4096.tif \
  Derived/terrain/natural_earth_relief_4096.png
```

## SRTM Plan

When the full SRTM mirror is available:

1. Build a world or regional VRT.
2. Limit SRTM use to land hotspots where higher resolution matters.
3. Keep GEBCO as the global default, because it covers oceans and poles and stays coherent at planetary scale.

## Notes

- GEBCO is the right first runtime source because it is already global and tiled cleanly.
- SRTM is higher resolution over land but should be treated as a second-stage enhancement, not the primary global source.
- The TID grid should be preserved for later provenance overlays or confidence masking.
- `gdal_contour` emitted repeated GeoPackage RTree warnings during generation, but the resulting files are valid and queryable with `ogrinfo`.

## USGS 3DEP 1 m (on demand, not mirrored)

The 3DEP 1 m bare-earth holding is hundreds of terabytes — a single state runs
500 GB to 1 TB — so it is deliberately **not** part of the `Data/` mirror. It is
consumed live from the dynamic image service:

`https://elevation.nationalmap.gov/arcgis/rest/services/3DEPElevation/ImageServer`

### Why this rather than the S3 COGs

`s3://prd-tnm/StagedProducts/Elevation/1m/Projects/` holds the raw source COGs
and is readable through `/vsicurl/` range requests, but the tiles are organised
per lidar project in mixed UTM zones. Using them means solving project discovery
and cross-projection mosaicking locally. The image service already does both,
server-side, for an arbitrary bounding box.

### What is fetched, and what is kept

| Path | Request | Persisted product |
|---|---|---|
| Deep-zoom contours | One `exportImage` clip per contour tile at the tile's raster size | Contour geometry in `srtm_focus_cache.sqlite`; the GeoTIFF is deleted |
| Elevation sampling | 0.02° chunks, cached as DEFLATE Float32 COGs | `Derived/terrain/3dep_1m/`, LRU-evicted against a configurable budget |
| Hillshade | `exportImage` with the `Hillshade Multidirectional` rendering rule | Nothing on disk; an in-memory texture |

The contour path persists geometry rather than elevation rasters. Dense,
sub-metre contour geometry can be much larger than its source raster: measured
tiles range from 69 MB to 244 MB of geometry for a roughly 24 MB raster. This
expansion is an important part of the storage and loading cost.

### Measured service behaviour

- `exportImage` returns HTTP 500 once the encoded response passes roughly 32 MB.
  2400×2400 Float32 (~22 MB) is comfortably inside that; 3000×3000 is not.
- Failures come back as an HTML page, not an error status, so responses are
  validated by magic number.
- The catalog `query` endpoint reports the finest available pixel size under a
  point, which is a free availability probe. `LowPS: 1` means 1 m source exists.
- Coverage is wider than expected — rural Nevada is 1 m — but Alaska is mostly
  3 m or coarser and nothing outside the US is served.

### Zoom tiers

Buckets 0–6 keep their SRTM sources unchanged. Buckets 7–10 are new and source
3DEP, extending the ladder from 0.16° half-extent down to 0.0148° at a 0.5 m
interval. Before this, `local_render_zoom` clamped the tile spec at zoom 20
while the visual scale kept going to 60, so everything below ~19 km across was
interpolated from a 40 m/px raster.

| Bucket | Opens at zoom | Tile box | Raster | Resolution | Interval |
|---|---|---|---|---|---|
| 7 | 13 | 46.7 km | 2048 | 22.8 m/px | 5 m |
| 8 | 21 | 14.9 km | 2400 | 6.2 m/px | 2 m |
| 9 | 31 | 6.8 km | 2400 | 2.8 m/px | 1 m |
| 10 | 44 | 3.3 km | 2400 | 1.4 m/px | 0.5 m |

**Sizing rule.** These tiers take a 5×5 envelope rather than the SRTM tiers'
13×13, because each tile is a network request. The lost coverage is made back
by enlarging the tiles: every tier must satisfy

    half_extent * (1 + 0.45 * radius)  >=  2.5 * visual_half_extent_for_zoom

at its opening zoom. The 2.5 factor is how far the oblique camera actually sees,
matching the local marker cull distance. Fewer wide requests beat more narrow
ones — radius 3 would allow only slightly smaller tiles for twice the downloads.

**Cost.** A fully filled deepest-tier view is 25 source rasters of ~22 MB.
Four downloads run independently of a CPU-scaled GDAL/import budget (half the
available CPUs, capped at four). Eight staging slots bound downloading plus
queued rasters; each download releases its network permit on completion and
its staging permit when processing takes ownership. Thus at most twelve
hosted jobs exist with the default maximum processing budget. Actual first-visit
time depends on USGS response latency. Contours are cached permanently and the
rasters are discarded.

### Drawing this much geometry

3DEP tiles carry far more geometry than SRTM ones. A measured bucket-10 tile
holds **9 826 contours and 4.3 million points**, so a 25-tile envelope is on the
order of 100 M points — orders of magnitude past what a frame can draw. Three
limits govern what reaches the screen, and all three had to move together:

1. **Cull margin** — `2.5 x visual_half_extent_for_zoom`, matching how far the
   oblique camera sees. It was `1.5x`, which culled terrain that was still on
   screen.
2. **Reader budget** — `feature_budget / assets`, floored at 120. Controls how
   much is resident in memory.
3. **Frame budget** — `MAX_CONTOUR_RENDER_POINTS`, 1 000 000 points. Controls
   how much is projected and tessellated per frame.

**Selection is length-ordered at both budgets.** The point distribution is very
skewed: the longest 10% of contours hold ~73% of all points, and the shortest
75% hold under 3%. Stride decimation — every Nth contour — drops a 10 000-point
shoreline trace and a 5-point speck at the same rate, so it spends the budget on
noise and produces a scattered sample rather than a picture. Keeping the longest
contours instead makes each budget buy far more visible structure, and it is
**stable under camera motion**: a long contour stays long, so it keeps making the
cut rather than flickering at the budget boundary.

### Moving it to the GPU

Those three limits were all consequences of drawing contours on the CPU. Earth
now renders them through `local_contour_pass`, an instanced segment pass in the
mould of the globe's `contour_pass`, and the per-frame budget stops applying:

- Instances are keyed **per source tile** and uploaded once. A tile's contour
  `Arc` is stable after loading, so panning, rotating and zooming never rebuild
  or re-upload anything — the vertex shader applies the whole local transform to
  raw `(lon, lat, elevation_m)` endpoints.
- Per frame the cost is one uniform write and one instanced draw per resident
  tile, regardless of how many points those tiles hold.
- The AABB cull disappears; off-screen segments are discarded by the rasterizer.
- The reader stops decimating: `feature_budget` for these tiers is now larger
  than a dense tile's contour count.

The remaining ceiling is a **resident-memory budget**, not a point count —
`local_contour_vram_budget_gb`, default 10 GB, evicting least-recently-drawn
tiles. At 32 bytes per segment a full 25-tile bucket-10 envelope is roughly
3.4 GB, so in practice the budget rarely evicts anything.

Above that the real limits are legibility rather than throughput: ~100 M
segments drawn into ~2 M screen pixels is heavy overdraw, and the thing to tune
there is the contour interval, not the geometry budget.

`MAX_CONTOUR_RENDER_POINTS` survives for the CPU fallback and the Moon and Mars
scenes. Its original 300 000 was sized against a WGPU index-buffer limit hit
when 1 600 globe tiles accumulated, which is a different path from the 25-tile
local envelope.


## Terrain throughput improvements (September 2026)

The runtime cache schema and tile grid are unchanged. Existing caches benefit
immediately; no purge or rebuild is required.

- Local SQLite reads stream in indexed fid order, decode borrowed geometry
  blobs, then stably sort the much smaller contour records by absolute elevation.
  Fid/part ordering, simplification, and longest-contour selection are preserved.
- Imports stream source rows in fid order with one prepared destination insert,
  borrowing blob storage rather than allocating another copy for every feature.
  Geometry and manifest still commit atomically; malformed input rolls back.
- Disposable GDAL contour/coastline GeoPackages skip unused spatial indexes and
  synchronous flushes. Persistent cache writes still use WAL/NORMAL. These are
  documented [GeoPackage driver options](https://gdal.org/en/stable/drivers/vector/gpkg.html),
  applied only to staging files which can be regenerated after interruption.
- Hosted jobs reserve download capacity separately from processing, allowing
  local NVMe-backed work to proceed while USGS responses are in flight.

### Measurements

On the attached Hilbert volume, a real bucket-10 tile contained 6,451 contours
and 244 MB of geometry blobs. In an optimized build, three alternating warm
read/decode comparisons produced:

| Operation | Before | After |
|---|---:|---:|
| Complete tile read + decode, run 1 | 143.9 ms | 96.5 ms |
| Complete tile read + decode, run 2 | 146.0 ms | 90.9 ms |
| Complete tile read + decode, run 3 | 125.9 ms | 85.8 ms |

That is 32–38% less time for this stage. Every comparison checked exact contour
geometry and order. These are warm-cache measurements, not a whole-app loading
speed claim; first reads also depend on disk state and OS caching.

A 2400×2400 raster cropped from local SRTM, contoured at 0.5 m solely for a dense
benchmark, produced the same 4,244 contours with identical geometry hashes in
both configurations. GDAL staging time decreased from 2.13–2.14 s to 2.05–2.07 s.
This measures staging overhead, not SRTM accuracy at sub-metre intervals. The
hosted pipeline's concurrency improvement is not a measured network speedup.

Reproduce the read/decode benchmark against an existing cache:

```bash
ONEKEE_CONTOUR_BENCH_DB=/path/to/Derived/terrain/srtm_focus_cache.sqlite \
  cargo test --release -p one-thousand-electric-eye-desktop \
  benchmark_dense_cached_tile -- --ignored --nocapture
```

The benchmark opens the supplied database read-only and selects one dense
bucket-10 tile. It is ignored in normal tests. At measurement time Hilbert had
about 7.8 GiB free and the Earth cache was 257 GiB; these changes do not reclaim
existing cache storage.


## Bounded pipeline and stage measurements (September 2026)

The scheduler now separates four active downloads from eight downloading/queued
rasters and up to four processing jobs. Download slots are released when a
validated raster has been saved, so a slow GDAL job does not stop the next
request. Admission reserves staging space before a request starts. Permits
release on errors, cancellation, and unwinding; no cache migration is needed.
Capacity releases invalidate manifest scheduling immediately, and snapshots
keep their selection-start revision so a wakeup during a query is not lost.
`ONEKEE_TERRAIN_WORKERS=1..8` overrides the processing limit for experiments,
while retaining one spare logical CPU on multicore machines. The default uses
half the available CPUs, capped at four. Moon/Mars retain their additional
per-body source limits.

Local cached geometry uses two readers per body cache on machines with at least
four available CPUs, otherwise one. Requests remain bounded to eight center-first
tiles per batch. Each completed tile is published immediately for background
merging. Reset epochs reject stale reads, and decoded nonempty tiles stay at
99% in the real loading grid until an accepted merge contains them.

### Results and limits

Measured in an optimized build on the 10-core, 32 GiB machine. Fresh benchmark
cache writes were compared on both system temporary storage and the attached
Hilbert NVMe (USB, encrypted APFS); existing cached geometry was read from Hilbert. These are headless pipeline
measurements, not whole-app frame-rate measurements.

| Workload | Previous limit | New limit | Measured wall time |
|---|---|---|---|
| Eight identical 2400-pixel USGS rasters, 0.5 m contours and shared SQLite import | 2 processing workers | 4 workers | 9.77–9.86 s → 5.84–5.93 s (about 40% less) |
| Same processing batch, internal temporary storage | 4 workers | 6 workers | 5.84–5.93 s → 5.38 s; smaller gain and more writer contention |
| Eight-raster build/import on the attached NVMe | 2 processing workers | 4 workers | 10.19–10.35 s → 6.44–6.85 s (about 33–38% less) |
| Twelve adjacent hosted tiles, repeated after responses warmed | 4 whole jobs / 2 processing workers | 4 downloads / 4 processing workers | 11.11 s → 8.73 s (about 21% less) |
| Eight dense existing tiles, warm repeated reads | 1 reader | 2 readers | 0.81–0.83 s → 0.51–0.56 s (about 31–38% less) |

The first live comparison was 31.51 s → 8.93 s, but mean download times changed
from 8.50 s to 2.35 s per tile. That comparison is **not** an isolated software
speedup. Initial reads of the eight NVMe tiles took 6.94–13.17 s before later
warm reads fell below one second; OS cache warmth was not controlled or flushed.

For the identical-raster workload, two workers spent about 1.9 s per tile in
GDAL, 0.4 s copying/committing geometry, and 0.03 s waiting for a writer. Six
workers increased mean writer wait to 0.56 s on internal storage. On the NVMe,
six workers took 6.17 s, a small gain over four. Four was selected to improve
throughput while leaving capacity for rendering/readers. Every processing round
committed eight manifests and identical row/byte totals; live runs each produced
115,536 rows / 420,472,032 geometry bytes. Cached-reader rounds require matching
hashes of every selected coordinate, elevation, and order.

### Diagnostics and reproduction

Set `ONEKEE_TERRAIN_TIMINGS=1` before launching the app to log `[terrain]` records
with tile identity, stage, milliseconds, row/byte counts, and success/abort.
Stages include source/download, processing wait, contouring, cache setup,
writer-lock wait, row copying/commit, SQLite read overhead, WKB decoding, and
geometry selection. Profiling is off by default and does no per-row clock reads
when disabled. GDAL stdout progress is suppressed only while profiling; errors
on stderr remain visible. Nested stage times must not be summed across workers.

```bash
# One real raster download (or supply ONEKEE_TERRAIN_BENCH_RASTER).
# Optional ONEKEE_TERRAIN_BENCH_OUTPUT=/volume/path selects the output drive;
# only a unique disposable child directory is created and removed.
ONEKEE_TERRAIN_TIMINGS=1 cargo test --release \
  -p one-thousand-electric-eye-desktop benchmark_contour_processing_concurrency \
  -- --ignored --nocapture --test-threads=1

# Legacy admission and processing baseline; repeat with both extra variables
# omitted for the new default pipeline. Outputs are disposable temporary files.
ONEKEE_TERRAIN_TIMINGS=1 ONEKEE_TERRAIN_WORKERS=2 ONEKEE_TERRAIN_BENCH_LEGACY=1 \
  cargo test --release -p one-thousand-electric-eye-desktop \
  benchmark_live_threedep_pipeline -- --ignored --nocapture --test-threads=1

# Read-only access to an existing cache; compares 1/2/4 readers.
ONEKEE_TERRAIN_TIMINGS=1 ONEKEE_CONTOUR_BENCH_DB=/path/to/srtm_focus_cache.sqlite \
  cargo test --release -p one-thousand-electric-eye-desktop \
  benchmark_cached_reader_concurrency -- --ignored --nocapture --test-threads=1
```
