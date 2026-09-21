# work_slots.rs

## Purpose
Bounds terrain processing separately from hosted raster downloads, so network
latency does not occupy the CPU/disk slots needed by local terrain generation.

## Components
- `Slots::try_acquire`: atomically claims capacity without blocking the caller.
- `Slots::acquire_until`: waits off-thread for processing capacity, checking
  cancellation every 25 ms.
- `Permit`: returns capacity on drop, including early returns and unwinding.
- `processing`: shares a CPU-scaled GDAL/import budget across terrain bodies (half
  the available CPUs, capped at four, at least one). A bounded
  `ONEKEE_TERRAIN_WORKERS=1..8` override supports controlled measurements;
  it still reserves one CPU on multicore machines.
- `remote_downloads`: reserves four network slots and eight staging slots.
  Staging covers downloading plus downloaded rasters waiting for processing.
  `DownloadPermit::downloaded` immediately releases the network permit; its
  returned staging permit is released once processing takes ownership.
  Total hosted workers/rasters are bounded by eight plus the processing cap.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| `builders.rs` | Local jobs acquire processing; hosted jobs acquire download and staging capacity | Holding processing capacity during download |
| `gdal.rs` | 3DEP acquires processing only after download, releases on every exit | Nested processing permits or UI-thread waits |

## Validation
Tests exercise stage overlap, saturation, cancellation, refilling downloads behind busy processing, queue backpressure,
failed admission rollback, concurrent claims, and permit release during unwinding without contacting a service.
