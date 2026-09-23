# Road continuity and toggle stalls

## Cause

The former local road renderer prepared up to 192 vertices per OSM way, sorted
ways longest-first, and spent 400,000 major / 800,000 minor points each frame.
Prefetched offscreen geometry also consumed those budgets. Once exhausted, the
renderer dropped remaining ways, including the short pieces connecting longer
roads; it could also stop partway through the last admitted way.

Each displayed point was projected on the CPU and passed to egui for line
tessellation again every frame. Turning both road classes off discarded the
cache, forcing another load/preparation on reenable. Automatic import scheduling
and ingest dequeue/schema checks also performed synchronous SQLite/disk work
from UI calls, creating another possible pause when roads were enabled.

## Change

- A background worker builds retained geographic GPU segments for every loaded
  road way. The shader uses the same local projection as terrain and markers.
- No per-layer point cutoff or longest-first road selection. Existing 192-point
  per-way simplification and baked heights remain compatible.
- Each upload batch is at most 2 MiB; at most three road batches upload per
  callback/frame. Pending batches request another frame until all are resident.
- Road/contour cache identities and uniform slots are independent; source
  version mismatches cannot paint stale buffers. Chunk boundaries preserve joins.
- Both road toggles retain prepared geometry. Root/data/theme changes and an
  escaped viewport still trigger background refresh. Cache publication locks
  are short; old geometry is released outside them.
- Road queue checks, inventory discovery and ingest dequeue/schema work run
  off the UI thread. Cached roads no longer depend on a contour tile being ready.
- Roads share the existing local GPU cache budget, now labelled **Local Map
  GPU Budget**. As before, actively drawn geometry is protected from eviction;
  the setting is not a hard limit on the current visible scene.

## Validation, 2026-09-23

Synthetic regressions retain a short connector after exceeding the old major
budget, preserve every edge across upload boundaries, and avoid joining unrelated
ways or bridging invalid points. A real GPU offscreen test draws contours and
four connected road batches (one more than the upload cap), checks the second
frame completes, and reads pixels back to verify continuity and separate widths.

A read-only release benchmark used Hilbert's Paris road cell at 48/2:

| Measurement | Result |
|---|---:|
| Major roads / prepared points | 55,908 / 357,722 |
| Minor roads / prepared points | 311,090 / 1,922,160 |
| Minor ways omitted or truncated by old budget | 264,941 |
| Retained GPU segments | 1,912,884 |
| GPU geometry | 61,212,288 bytes, 30 batches |
| Background instance preparation | 72.94 ms |
| Old CPU projection + egui tessellation, 3 samples | 58.39–64.38 ms |
| Retained GPU callback submission, 3 samples | 0.030–0.048 ms |

These timings isolate CPU frame preparation with elevations zeroed in memory;
they exclude loading, real terrain sampling, GPU upload/rasterization and other
layers. They are not full-app frame-time claims. No source cache was changed.

Reproduce with `ONEKEE_ROAD_BENCH_CELL=/path/to/road_cell.1kc cargo test --release
-p one-thousand-electric-eye-desktop benchmark_real_road_frame_preparation --
--ignored --nocapture` (one line). The hardware test is
`roads_and_contours_render_together_and_deferred_uploads_complete`.

This addresses renderer-imposed omissions. A screenshot alone cannot establish
whether additional gaps already exist in the source cache. Missing/incomplete
source roads require separate inspection; repacking an archive cannot restore
geometry absent from its inputs. No `.1ka` rebuild is required for this change.
