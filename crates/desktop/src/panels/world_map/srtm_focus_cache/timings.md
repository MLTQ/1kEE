# timings.rs

## Purpose
Measures terrain pipeline stages on real data without assigning artificial
time estimates to the loading grid. Enable with `ONEKEE_TERRAIN_TIMINGS=1`
before launching the app or an ignored benchmark.

## Components
- `StageTimer`: opt-in wall-clock timer; `finish` reports success and counts,
  while early returns/unwinding report an aborted stage.
- `record`: emits one `[terrain]` log record with tile identity, stage,
  milliseconds, rows, bytes, and outcome. Used for accumulated decode time.
- `clock`: returns a timestamp only when profiling is enabled, avoiding clock
  calls inside the normal geometry read loop.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| Builders/importers/readers | No filesystem access or logging when disabled | Enabling by default or synchronous log files |
| Benchmarks | Stage names distinguish queue waits from useful work | Combining processing wait with contour generation |

## Notes
Overlapping stages must not be summed as total throughput. `build_3dep` and
`build_srtm` measure whole jobs; nested timers explain their costs. Read timing
separates WKB parsing/simplification from SQLite/iteration overhead; it cannot
distinguish physical drive reads from OS filesystem-cache hits.
