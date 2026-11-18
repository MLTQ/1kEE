# Terrain loading measurements, 17 November 2026

## Cold source reads and decoding

On the RTX 4090, the new reader keeps native signed 16-bit SRTM posts through
GPU upload. On the tested 512×512 Deflate TIFFs it reads requested blocks in
file-offset order, overlaps reading with decompression and shares up to 32 MiB
of decoded boundary blocks across requests. The two GPU generations below both
build native-resolution contour geometry.

| Nine-tile region | Reader and source drive | First draw | Five tiles | All nine |
| --- | --- | ---: | ---: | ---: |
| Wide Yemen | Previous, external HDD | 2.34 s | 6.90 s | 10.55 s |
| Wide Yemen | New, external HDD | 2.11 s | 5.79 s | 9.10 s |
| Wide Yemen | New, internal SSD | 1.30 s | 2.68 s | 4.12 s |
| Dead Sea | Previous, external HDD | 0.54 s | 1.20 s | 1.46 s |
| Dead Sea | New, external HDD | 0.49 s | 0.91 s | 1.34 s |

Each cell is the median of three fresh processes running
`cold_earth_local_nine_tiles`. On the HDD the sequence was new, previous,
previous, new, new, previous, measuring both regions on each pass. Every run
requested Linux file-page eviction for its source files and started with an empty
SQLite cache on the internal SSD. The SSD comparison used temporary copies of
the same 36 Yemen GeoTIFFs, flushed to storage before source-page eviction.
Copy time is outside the draw timings. First-draw time varies with worker
scheduling. The wide HDD ranges were 1.77–2.72 s previously and 1.62–2.15 s
with the new reader.

| Median for nine tiles on the HDD | Wide, previous | Wide, new | Dead Sea, previous | Dead Sea, new |
| --- | ---: | ---: | ---: | ---: |
| Source bytes read by the TIFF reader | 336.4 MB | 302.2 MB | 15.3 MB | 9.0 MB |
| OS block reads charged to process and children | 299.7 MB | 291.9 MB | 13.3 MB | 10.9 MB |
| Blocks decoded | 1,764 | 1,736 | 72 | 42 |
| Time preparing source windows and submitting resampling | 9.16 s | 7.61 s | 0.50 s | 0.37 s |
| Time inside source reads and seeks | 6.17 s | 7.11 s | 0.27 s | 0.30 s |
| Reported decode time | 2.58 s | 1.21 s | 0.16 s | 0.07 s |
| Process peak resident memory | 1,805 MiB | 1,788 MiB | 1,101 MiB | 1,139 MiB |

OS block reads use `wait4.ru_inblock × 512` and include activity outside the
input reader. The new reader includes TIFF validation in its byte count.
Decode timings are approximate. The old timer measured window work
outside file I/O. The new timer measures inflation, validation and copying, which
can overlap the next disk read. On the SSD, preparing the wide group's source
windows took 1.78 s. The 16-bit source format also halves the GPU source
window capacity requirement, from 594 to 297 MiB for twelve full windows.

The old and new GPU builds produced identical ordered contour geometry for
Yemen tile 0/9/27, covering 58,996 paths and 1,531,831 segments. Both 63,833,458-byte
geometry exports had SHA-256 `673a1e6da4138f0bfa94ffbf425c84e1d38a26a3d34120965f3da3103e41ffe8`.

## Original GDAL and GPU comparison from an empty cache

On the RTX 4090, the desktop now uses Vulkan compute for native SRTM resampling
and contour extraction. One observed 5.61 s GDAL build of Yemen tile 0/9/27
spent 3.96 s in `gdalwarp`. The following test measures the additional wait
for neighboring tiles, including source reads, cache writes and drawing.

| Nine-tile group and engine | First tile | Five tiles | All nine |
| --- | ---: | ---: | ---: |
| Wide Yemen, GDAL | 9.54–9.66 s | 10.82–11.02 s | 14.37–14.54 s |
| Wide Yemen, RTX 4090 | 1.89–2.14 s | 6.85–6.91 s | 10.67–10.69 s |
| Dead Sea, GDAL | 0.87–0.92 s | 1.15–1.34 s | 1.63–1.94 s |
| Dead Sea, RTX 4090 | 0.47–0.59 s | 1.10–1.35 s | 1.56–1.68 s |

Each range covers two fresh processes. Both engines ran on the same final code
in GDAL/GPU/GPU/GDAL order per region. Every process started with a new
temporary SQLite destination on the internal SSD. Before each run Linux was
asked to evict the input TIFF pages for that group from the external drive's
OS file cache. Hardware caches and desktop activity were uncontrolled.
The nine tiles are centered on native addresses 0/10/28 and 4/126/143. In the
wide run, the GPU worker read 336.4 MB of compressed source windows. Across
nine tiles, source I/O took 6.19–6.20 s, TIFF decoding took 2.53–2.58 s and
GPU contour extraction plus readback took 0.41 s. Other work overlaps these
times. With OS source pages warm and an empty RAM-backed destination, the wide
GPU group finished in 4.71–4.87 s. Uncached hosted detail has a separate
network path.

The ignored `cold_earth_local_nine_tiles` test uses the production Local
build scheduler, SQLite reader, contour instance jobs and draw callback with
occlusion off. Each poll submits and waits for an offscreen GPU frame, then
counts the nonempty tile batches drawn. The final frame is checked for contour
pixels. The fixed 3×3 region isolates the contour layer from other app activity
and the desktop compositor. Start it from a test executable with isolated
settings, set `ONEKEE_SRTM_ROOT`, `ONEKEE_GPU='RTX 4090'` and
`ONEKEE_TERRAIN_TIMINGS=1`. Set `TMPDIR` on the destination device.
`ONEKEE_LOCAL_WIDE=1` selects the wide case. `ONEKEE_TERRAIN_GPU=0` selects
GDAL. Leaving the override unset exercises desktop auto-selection.

The separate `gpu_srtm_end_to_end_benchmark` compares completed native cache
tiles against GDAL. For tile 0/9/27, the two writers produced 1,531,831 nonzero
segments each. Summed line length differed by 0.00005%. All three full-cache
fixtures met the 0.1% length tolerance. The raster tests also compared valid
heights and nodata across source seams on both hemispheres. In the wide-tile
benchmark, optimized CPU stitching took 105–127 ms (previously 523–535 ms).
These geometry checks cover the measured fixtures.

## Reading already-built Local contour tiles

The release `benchmark_local_core_load` test opens a private SQLite snapshot,
reads the tile, decodes its geometry, clips to its owned Earth core and returns
the ordered contour paths. The timer stops before checksumming. Four processes per version in alternating order ran on the same CPU core on this machine. The harness
requested Linux file-page eviction for the two fixture files before each first
read. Each process then repeated the read five times. This measures already-built
local tiles. Source download, contour generation and rendering have separate costs.

| Fixture | Ordered parts | Vertices | Before first / warm | After first / warm |
| --- | ---: | ---: | ---: | ---: |
| Native Earth 4/152/-435 | 45,828 | 1,324,808 | 69.52 / 39.89 ms | 26.12 / 12.14 ms |
| Hosted Earth 8/1242/-4060 | 7,712 | 301,562 | 15.22 / 10.72 ms | 10.31 / 6.68 ms |

Medians use four first reads and twenty repeated reads per fixture and version.
The two ordered geometry hashes were `74aa18f05912e6ca` and `f2616bfc05bf824a`
respectively in both versions. The normal renderer can reuse a resident tile
without issuing another SQLite read. To reproduce the read timing, set
`ONEKEE_LOCAL_TILE_DB` to a private snapshot and run the ignored test.

## Projected LiDAR

On the complete cached Sharon sea cliff survey, loading the original LAZ and
transforming its XY coordinates fell from 65.97 s to 10.97 s. Both runs decoded
13,268,620 returns, retained 377,003 inside the 1 km target, and transferred zero
bytes. The sample uses EPSG:2039 and the original vertical values. Run
`target_live_israel_original_points_and_retained_dem` with
`ONEKEE_TARGET_TEST_CATALOG` set to a catalog with the local survey.

## Globe terrain

The native Yemen/Red Sea fixture has 46 tiles and 39,491,374 segments. On the
RTX 4090 (Vulkan 615.78.08) at a 4 MiB byte allowance and 2 ms admission
allowance, the worst map-thread staging call went from 221.11 ms to 0.13 ms.
With the worker, staging p99 was 0.024 ms. The complete first geometry snapshot
took 0.89 s with a 1 ms polling interval. This tight polling run measures CPU
admission and publication. Driver allocation still consumes worker time.

A separate 1920 × 1080 offscreen route uses real source tiles and the production
egui contour callback. It pans, zooms and returns with a 60 Hz target. Frame
durations include egui, rendering, queue submission and a GPU wait. They exclude
the pacing sleep, the desktop compositor and unrelated application layers.

| Leg | Tiles | Frame work p95 | Frame work p99 | Time until that line set was complete |
| --- | ---: | ---: | ---: | ---: |
| Arrive | 18 | 9.86 ms | 11.25 ms | 1.87 s |
| Pan and zoom | 24 | 14.83 ms | 17.44 ms | 2.01 s |
| Return | 18 | 11.25 ms | 16.03 ms | 1.40 s |

That route reused 29 retired buffers. Both Globe measurements use
`benchmark_native_globe_tiles` with `ONEKEE_NATIVE_BENCH_DB` pointing to the
native database and `ONEKEE_GPU='RTX 4090'`. Add `ONEKEE_TERRAIN_ROUTE=1`
for the paced route. These are single-machine measurements.

## Offline verification

A 5° Rocky Mountains pin built from local GEBCO occupied 11,522,048 bytes.
Local and Globe returned identical height arrays from the installed file, with
a sampled range of 2,002 to 4,148 m. Separate native and hosted contour packages
verified, installed twice, and rejected a damaged copy while preserving the
prior destination records.
