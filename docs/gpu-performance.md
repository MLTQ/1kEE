# GPU selection and terrain throughput

For the Arch workstation with an RTX 4090 (24 GiB) and 2070 SUPER (8 GiB), launch
from the repository with:

```sh
./scripts/run-4090.sh
```

The script builds/runs the release desktop, selects an adapter whose name contains
`RTX 4090`, enables terrain timings, and sets the experimental budgets below.
It does not restart an existing desktop process or modify data/configuration.
Startup prints the actual adapter, backend, driver and effective budgets to stderr.
No matching surface-compatible adapter is an error, rather than a silent fallback.
When multiple backends expose the matching GPU, native Vulkan/Metal/DX12 is preferred.
Without `ONEKEE_GPU`, the existing eframe adapter selection remains unchanged.

| Launch variable | Portable default | 4090 script | Accepted range |
|---|---:|---:|---:|
| `ONEKEE_GPU_CACHE_MIB` | 3072 | 8192 | 256–32768 |
| `ONEKEE_CPU_CACHE_MIB` | 4096 | 12288 | 256–65536 |
| `ONEKEE_ATLAS_RAM_MIB` | 1024 | 8192 | 256–32768 |
| `ONEKEE_ATLAS_DISK_MIB` | 8192 | 32768 | 512–262144 |
| `ONEKEE_ATLAS_KEEP_GLOBE` | 0 | 1 | 0–1 |
| `ONEKEE_UPLOAD_MIB` | 4 | 16 | 1–64 |
| `ONEKEE_UPLOAD_US` | 2000 | 2000 | 100–8000 |

Values are positive integers read once per process; invalid values log a warning
and use the portable default. Script defaults respect explicit environment values.
For example, `ONEKEE_UPLOAD_MIB=8 ./scripts/run-4090.sh` tries a smaller upload batch.
`ONEKEE_GPU='2070 SUPER'` selects the other GPU, but use appropriate smaller memory
budgets too: the script's memory profile is intended for the 24 GiB card.

The Atlas settings govern prepared height/color RAM and compressed disk caches
separately from contour memory. With `ONEKEE_ATLAS_KEEP_GLOBE=1`, completed Globe
windows bypass RAM eviction and their GPU textures stay resident until Cache
Blast or exit; Local remains bounded. Use `0` for a bounded-residency comparison.
See [Atlas reuse](atlas-cache.md#interactive-map-reuse).

Memory settings are terrain retention targets, not reservations or
whole-application memory limits. Visible geometry takes priority over eviction;
staging, retained images, other layers and other applications consume extra memory.
The CPU target also matters: evicting decoded sources can retire their GPU geometry.
Defaults on other machines are unchanged, and no VRAM capacity is guessed from
wgpu's maximum single-buffer size.

The upload budget is shared across line layers within each globe/local frame.
Both byte and elapsed submission limits still apply. The time limit cannot
preempt a driver allocation already in progress and does not measure GPU execution.
Complete snapshots replace older geometry atomically. Increasing bytes can reduce
frames needed to publish a tile only when bytes, rather than time, limit progress.

## Measurement

Compare the same release executable with 4 and 16 MiB uploads, the same adapter,
window size and navigation. Keep the memory targets equal when isolating uploads.
Use `ONEKEE_TERRAIN_TIMINGS=1` for read/decode/build stage logs. Cache warmth matters;
do not interpret warm second visits as a software speedup.

The offscreen hardware regression selects `ONEKEE_GPU` too:

```sh
ONEKEE_GPU='RTX 4090' ONEKEE_UPLOAD_MIB=16 cargo test --release \
  -p one-thousand-electric-eye-desktop \
  bounded_uploads_publish_atomically_and_strip_matches_previous_pixels \
  -- --ignored --nocapture --test-threads=1
```

The native-cache benchmark is read-only and requires existing Earth native tiles
in its Yemen/Red Sea fixture window. Run with `ONEKEE_NATIVE_BENCH_DB` pointing at
`srtm_native_v1.sqlite`, and the test filter `benchmark_native_globe_tiles`.
It reports preparation time, upload frames and offscreen render/refinement timings;
these are not interactive pan-to-visible latency measurements.

Raw PBF builds remain CPU/storage pipelines. SRTM raster sampling and contour
extraction now have an opt-in compute backend (see below); other terrain sources
continue through GDAL.
Existing terrain preparation already uses Rayon across tiles. See
[terrain-pipeline.md](terrain-pipeline.md), [cache-builder.md](cache-builder.md)
and [runtime-archive.md](runtime-archive.md) for build and storage measurements.

## Arch RTX 4090 measurements (2026-11-06)

Release offscreen tests selected NVIDIA RTX 4090 / Vulkan, driver 615.78.08,
on the Ryzen 5 5600X workstation. The real-cache fixture read 46 tiles and
39,491,374 unchanged segments from Minkowski's `srtm_native_v1.sqlite`.
After initial runs and compilation finished, order was 4/16/16/4 MiB,
keeping the 2 ms submission budget unchanged:

| Upload cap | Frames to publish all geometry | Whole benchmark time |
|---|---|---|
| 4 MiB | 291, 291 | 6.28 s, 6.19 s |
| 16 MiB | 184, 178 | 6.02 s, 6.28 s |

The larger cap needed about 37–39% fewer upload frames. The benchmark advances
frames immediately, so this is not a measured wall-clock loading speedup at a
fixed interactive frame rate; overall benchmark time was effectively similar.
Maximum staging calls were 154–193 ms, despite the submission budget: allocation,
cleanup and driver calls cannot be preempted by admission checks. Other desktop
activity was not isolated. This leaves an important frame-latency follow-up.
No CPU preparation or rendering improvement is attributed to this budget change.

The separate synthetic atomic-publication/pixel comparison passed at both caps,
but required seven frames in each case: that fixture hit the time budget instead.
The larger memory targets have not been benchmarked for interactive revisit
latency or peak whole-app memory. The profile remains opt-in.


## Opt-in GPU terrain generation (2026-11-07)

Start the desktop with both the RTX 4090 rendering profile and GPU SRTM builds:

```sh
./scripts/run-4090-terrain.sh
```

Or set `ONEKEE_TERRAIN_GPU=1 ONEKEE_GPU='RTX 4090'` on the desktop executable.
This affects uncached SRTM tiles, including isolated repair tiles. Existing
caches are reused. The current internal repair-root setting remains in effect;
source elevation files are read-only. No cache deletion or drive repair is needed.
Unset `ONEKEE_TERRAIN_GPU` (or set it to `0`) to return to GDAL generation.

GPU resampling and marching squares share a resident raster; CPU work still
includes TIFF decoding, line stitching, clipping, and SQLite writes. Up to twelve
source windows stay in VRAM across builds. The compute worker follows
`ONEKEE_GPU`, uses one adapter, and consumes up to roughly 1 GiB beyond renderer
budgets. CPU fallback handles unsupported source formats/projections, unavailable
GPUs, invalid source reads, and output-capacity failures. GDAL remains the default.

For explicit offline builds, use `contours-bbox --engine gpu` with the usual
SRTM root, bounding box, zoom buckets, and cache destination. This mode reports
GPU failures instead of hiding them behind a fallback; successful tiles commit
atomically and can be resumed using the same destination. For example:

```sh
ONEKEE_GPU='RTX 4090' target/release/one-thousand-electric-eye-cache-builder contours-bbox \
  --engine gpu \
  --srtm-root /run/media/m/Minkowski/Data/srtm_gl1/SRTM_GL1_srtm \
  --cache-db /tmp/1kee-gpu-example/srtm_native_v1.sqlite \
  --min-lat 31.18 --max-lat 31.18 --min-lon 35.39 --max-lon 35.39 \
  --zoom-buckets 4
```

Validation uses synthetic GDAL contour fixtures (including ties, saddles and
partial nodata), two seam-crossing source patches, and complete rasters for three
real native tiles. Elevations agree within 0.01 m across those raster comparisons.
Total clipped contour lengths agree within 0.002% on the three real tiles.
This does not establish bitwise or global topology equivalence: the native
writer uses f32 coordinates and path/vertex counts can differ from GDAL's f64
geometry. Wider validation remains required before default enablement.

Hardware checks, with `ONEKEE_SRTM_ROOT` and `ONEKEE_GPU` set:

```sh
cargo test --release -p one-thousand-electric-eye-cache-builder gpu_terrain \
  -- --include-ignored --nocapture --test-threads=1
# Leave ONEKEE_TERRAIN_GPU unset for the GDAL baseline in this benchmark:
cargo test --release -p one-thousand-electric-eye-desktop gpu_srtm_end_to_end_benchmark \
  -- --ignored --nocapture --test-threads=1
```

The benchmark writes temporary caches, alternates GDAL/GPU then GPU/GDAL order,
and includes source loading, GPU transfers, CPU stitching, clipping and SQLite
commits. Source OS caches may already be warm; first-use GPU timings include
initialization and uploads, while repeated calls reuse the GPU source cache.
These are tile-build measurements, not interactive frame-rate or disk-cold tests.

Initial-backend (`51bc9c3`) RTX 4090/Vulkan measurements on the Ryzen 5 5600X,
with compilation stopped; these precede the cold-source optimization below:

| Native tile | GDAL repeat | GPU first pass | GPU repeat | Repeat speedup |
|---|---:|---:|---:|---:|
| 4/126/143 | 334 ms | 422 ms | 106 ms | 3.2× |
| 0/9/27 (Yemen, 5837² raster) | 3389 ms | 1536 ms | 821 ms | 4.1× |
| 4/-98/-235 | 270 ms | 154 ms | 68 ms | 4.0× |

Only the first row's GPU first pass includes initial device/pipeline setup.
The first small tile was slower than GDAL; resident source reuse is central to
these gains. Timing logs separate source submission, compute/readback and CPU
stitching. The three-region benchmark is intentionally limited; follow-up
`1kee-988` covers wider parity and interactive-load validation. Current regression
checks passed: 394 desktop tests, 36 cache-builder unit/integration tests, six
GPU tests, the end-to-end benchmark, and forced unavailable-adapter GDAL fallback.


## Cold-source builds: windowed TIFF reads

The SRTM compute backend already replaces both `gdalwarp` and `gdal_contour`
for uncached tiles. Its first implementation read and uploaded complete
3601×3601 source rasters on a residency miss. A small output tile could therefore
pay for most of a one-degree file it did not need.

It now decodes only TIFF tiles/strips intersecting the requested sample window,
including interpolation neighbors. Uploads contain only those windows; the
bounded GPU cache reuses a window only when it fully covers the request. The
output raster is initialized to nodata by a compute pass, removing the CPU
allocation/fill/upload of up to 144 MB per output grid. Normal OS readahead is
retained because forcing random scheduling did not improve elapsed time on this
connection. Startup/lock-wait, file I/O, decoding, upload submission, and
compute/readback are logged separately.

The cold benchmark uses a fresh process and fresh temporary destination for
every run, requests `POSIX_FADV_DONTNEED` on the relevant source files before
each run, and alternates GDAL/GPU ordering. This is best-effort OS data-cache
eviction, not a controller-cache flush. It includes GPU initialization,
source reading, processing, SQLite writes and final geometry validation. Source
files/schema are opened read-only; generated databases/logs are under `/tmp`.
The reported input-byte count is `ru_inblock × 512` for the process and its
children, which can include non-source reads. The original binary was saved
before editing and tested using the same harness.

```sh
python3 scripts/benchmark-cold-terrain.py \
  --binary target/release/one-thousand-electric-eye-desktop \
  --label windowed \
  --srtm-root /run/media/m/Minkowski/Data/srtm_gl1/SRTM_GL1_srtm \
  --schema-db /path/to/a/valid/srtm_native_v1.sqlite
```

`--schema-db` supplies only table/index definitions and the native-quality tag;
no existing geometry is copied or replaced. The fixture source directory must
contain the three tested regions. The harness fails on unexpected GPU fallback.


Measured on the **current USB 2.0 connection**, two fresh-process runs per case:

| Native tile | GDAL | Windowed GPU | Previous GPU input reads | Windowed GPU input reads |
|---|---:|---:|---:|---:|
| 4/126/143 | 0.44–0.50 s | 0.59–0.67 s | 10.71 MB | 4.78 MB |
| 0/9/27, 5837² | 5.39–5.60 s | 3.27–3.53 s | 78.36 MB | 46.08 MB |
| 4/-98/-235 | 0.326–0.328 s | 0.40–0.43 s | 5.28 MB | 1.45 MB |

MB here is decimal; input reads include process-tree I/O, not only source bytes.
The large tile benefits; first-use GPU setup still makes the small cases slower
than GDAL. Each process pays roughly 0.2 s for GPU initialization. A running
application normally pays that setup cost once, but source reads remain cold
when visiting a new region. These numbers must not be conflated with the earlier
warm-source 3–4× result.

An independent 1kEE **global-overview** `gdalwarp -r average -tr 0.2` job was
active during the final runs, reading the same drive and consuming CPU. Wall
times varied across runs, so before/after time ratios are not presented as
controlled speedups. The distinct overview path still uses GDAL; detailed
SRTM tile acceleration does not replace it. Raw observations and protocol are
saved in [terrain-cold-2026-11-07.json](benchmarks/terrain-cold-2026-11-07.json).

Validation passed: 394 desktop tests, 38 cache-builder unit/integration tests,
eight focused GPU/source-window checks, and the three-region full-cache parity
benchmark. Clippy completed with existing repository warnings. The cache-builder
suite was rerun serially after an unrelated compact-PBF lock failure; follow-up
`1kee-990` records that failure. Wider terrain/topology and interactive validation
remain tracked by `1kee-988`.
