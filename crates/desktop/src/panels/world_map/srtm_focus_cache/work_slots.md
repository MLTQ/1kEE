# work_slots.rs

## Purpose
Bounds terrain processing separately from hosted raster downloads, so network
latency does not occupy the CPU/disk slots needed by local terrain generation.

## Components
- `Slots::try_acquire`: atomically claims capacity without blocking the caller.
- `Slots::acquire_until`: waits off-thread for processing capacity, checking
  cancellation every 25 ms.
- `Permit`: returns capacity on drop, including early returns and unwinding.
- `processing`: shares at most two GDAL/import jobs across terrain bodies,
  reduced to one on machines with fewer than three available CPUs.
- `remote_jobs`: permits four complete 3DEP jobs, including downloads, waiting
  rasters, and processing. This also bounds staging storage and waiting threads.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| `builders.rs` | Local jobs acquire processing; hosted jobs acquire remote capacity | Holding processing capacity during download |
| `gdal.rs` | 3DEP acquires processing only after download, releases on every exit | Nested processing permits or UI-thread waits |

## Validation
Tests exercise stage overlap, saturation, cancellation, concurrent claims,
and permit release during unwinding without contacting a service.
