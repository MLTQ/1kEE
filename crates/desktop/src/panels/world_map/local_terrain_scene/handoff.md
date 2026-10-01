# handoff.rs

## Purpose
Coordinate progressive geographic LOD composition and CPU/GPU publication.
A missing edge tile cannot trap the whole view on an older coarse generation.

## Components
- `prepare` retains visible source tiles, assigns the requested tier priority,
  and runs one coalesced composition worker. Geometry work stays off paint.
- `select` stages the complete composition, retaining the prior picture until
  its changed GPU buffers finish uploading. CPU-only bodies publish directly.
- `Piece` caches reuse stable clipped geometry across arrivals. Fully replaced
  sources are released; retained sources and display tiles are culled by cores.

## Contracts
- Only decoded/published tiles enter composition; no-source progress is not data.
- Prepared frames stay stable during upload, even if more reads arrive.
- Frame IDs prevent a worker finishing between prepare/select from being lost.
- Root, body, tier changes and manual reset invalidate late workers. The occupied
  worker slot survives invalidation until return, bounding concurrent work.
- Leaving local view clears display/source geometry and cached compositions.
- Disk grids and cache files are unchanged. Both CPU and GPU consume the same
  clipped picture; legacy overlapping footprints are handled by `composition`.
