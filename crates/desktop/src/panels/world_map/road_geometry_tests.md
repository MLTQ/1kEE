# road_geometry_tests.rs

## Purpose
Guards against the reported missing road connectors and new GPU batch seams.

## Contracts
- A source polyline crossing a 2 MiB upload boundary retains every segment and
  exactly matching shared endpoints; separate roads are never joined.
- A tiny connecting way after more than 400k major-road points remains present,
  with its source coordinates and baked elevation/display offset unchanged.
- Invalid coordinates split a line without bridging across the bad vertex.
- Synthetic tests do not read or modify user data.
- Optional `benchmark_real_road_frame_preparation` reads only the explicit
  `ONEKEE_ROAD_BENCH_CELL` path. Reports old budget omissions and CPU frame
  preparation/tessellation versus retained callback submission, plus one-time
  instance build bytes/time. Heights are zeroed in memory to isolate drawing;
  timings exclude GPU upload/rasterization and are not full frame latency.
