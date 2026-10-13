# local_occlusion_gpu_tests.rs

## Purpose
Private offscreen GPU tests for real contour depth visibility, camera/source
invalidation and a repeatable synthetic visual/performance comparison.

## Coverage
- Nearby ridges hide lower distant lines without removing foreground, ridge
  crests or higher distant terrain.
- Unchanged frames reuse depth; moving camera, replacement geometry and toggles
  invalidate it. Hidden terrain releases resources. Road uniforms remain separate.
- Offset callback rectangles use correct depth-texture coordinates.
- Visual fixture emits before/after PNGs in /tmp; timings include egui preparation,
  encoding, GPU execution and synchronization, excluding readback and image saving.

## Invocation
`cargo test --release -p one-thousand-electric-eye-desktop contour_occlusion -- --ignored --nocapture`
Requires a real GPU outside the restricted sandbox. Never opens a window or
changes source caches, app settings or the running application.

Camera extremes cover 27 pitch/yaw/spread combinations, including 100× spread,
near-horizontal viewing and geographic coordinates near the date line. The
optional real-data fixture reads nine cached SRTM tiles through SQLite read-only,
clips them with the production core-ownership code, and renders both modes.
Set `ONEKEE_OCCLUSION_DB` to the source `srtm_focus_cache.sqlite`; it neither
builds missing tiles nor writes that database. PNGs use `/tmp/1kee-occlusion-real-*`.

## Measured baseline (2026-10-13)
Apple M1 Pro / Metal, 768×512 offscreen, 40 moving-camera frames, same source
geometry in both modes. Nine production-clipped tiles contain 404,093 line
segments and 24,778 curtain segments. Median/p95: disabled 4.023/5.942 ms;
enabled 3.969/5.313 ms. Differences are within run-to-run noise, not a promised
speedup. These timings include CPU/GPU synchronization, not full-window UI or
OBS capture. Source clipping plus curtain/line construction took 53.3 ms for
all nine tiles; normal operation performs that construction on tile workers.
Single-batch fixtures borrow their batch instead of cloning it per frame.
