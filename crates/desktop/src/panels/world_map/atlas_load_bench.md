# atlas_load_bench.rs

Opt-in benchmark of the production load_detail path on regional GEBCO and local
SRTM windows. Set ONEKEE_ATLAS_ROOT to an existing terrain root and run the
atlas_map_load_latency test with --ignored --nocapture --test-threads=1.
Original data is read through a symlink into a private temporary root; all new
cache writes remain there. Reports first generation, two exact RAM revisits, and
a reload after clearing the in-process view cache. The latter retains the OS page
cache and measures a disk-cache decode, not a physical cold-drive read. Assertions
check raster reuse, persisted height/material row counts, exact elevation bits
and prepared color equality. Timings
cover loading/pigmentation, excluding GPU upload and drawing.

Measured on this Arch machine (2026-11-09):

| View | Previous repeated load | Prepared RAM reuse | Saved-window decode |
| --- | --- | --- | --- |
| GEBCO region, 6240×3600 | 2275–2305 ms | <0.002 ms | 149 ms |
| SRTM local, 3600×3600 | 2544–2735 ms | <0.002 ms | 92 ms |

The saved-window measurements use the private `/tmp` cache and warm memory/file
pages. They are not cold USB-drive measurements. Two saved windows compressed to
68.58 MB; first-generation source reads and GPU transfer/drawing remain separate.

After adding prepared-color persistence and larger retention budgets (2026-11-09):

| View | First generation | RAM revisit | Prepared disk reload |
| --- | --- | --- | --- |
| GEBCO region, 6240×3600 | 3219 ms | <0.002 ms | 179 ms |
| SRTM local, 3600×3600 | 3558 ms | <0.002 ms | 81 ms |

Both colors and heights compressed to 107.66 MB for the two windows. These use
the same private warm `/tmp` methodology above, not cold USB-drive timing or
full GUI latency. The ongoing OSM builder was left running. A separate RTX 4090
GPU retention regression verifies zero re-upload bytes across mode revisits.
