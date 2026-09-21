# bench_resources.rs

## Purpose
Measures approximate peak resident memory across the benchmark test process
and its GDAL descendants. It is compiled only into opt-in benchmark tests.

## Components
- `Sampler`: samples `ps` every 100 ms on a joined background thread; stops on
  normal completion or unwinding. Reports zero if process sampling is denied.
- `sample`: follows parent PID relationships, sums only the requested tree,
  excludes the sampler's `ps`, and counts concurrent GDAL processes.
- `Peak`: peak summed RSS in KiB and peak concurrent GDAL process count.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| `pipeline_bench.rs` | No native app instrumentation or production overhead | Compiling into normal runtime |
| Measurements | Approximate 100 ms process-tree snapshots | Presenting RSS as exact physical memory or UI frame rate |

## Notes
Summed RSS may count shared pages more than once. Samples exclude the user's
running app and other tasks, so they describe additional benchmark pressure.
