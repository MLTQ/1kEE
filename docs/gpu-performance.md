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

Raw PBF and terrain builds remain CPU/storage pipelines. GPU raster sampling and
contour extraction require a separate implementation and output-parity validation.
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
