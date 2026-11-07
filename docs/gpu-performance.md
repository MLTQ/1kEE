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
| `ONEKEE_UPLOAD_MIB` | 4 | 16 | 1–64 |
| `ONEKEE_UPLOAD_US` | 2000 | 2000 | 100–8000 |

Values are positive integers read once per process; invalid values log a warning
and use the portable default. Script defaults respect explicit environment values.
For example, `ONEKEE_UPLOAD_MIB=8 ./scripts/run-4090.sh` tries a smaller upload batch.
`ONEKEE_GPU='2070 SUPER'` selects the other GPU, but use appropriate smaller memory
budgets too: the script's memory profile is intended for the 24 GiB card.

Memory settings are Earth globe terrain retention targets, not reservations or
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
source rasters stay in VRAM across builds. The compute worker follows
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

Final RTX 4090/Vulkan measurements on the Ryzen 5 5600X, with compilation stopped:

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
