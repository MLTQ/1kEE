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
  requested sources are released; retained sources and display tiles are culled
  by cores. A covered fallback source keeps its shared Arc to avoid re-adding
  the same base-loader tile and recomposing every frame; it has no display piece.

## Contracts
- Only decoded/published tiles enter composition; no-source progress is not data.
- `LocalContourLoad.detail` identifies the requested tier when its main loading
  grid is SRTM fallback. Fine arrivals receive the current generation priority;
  base fallback receives priority zero, below all retained requested sources.
  Panning into a missing fine edge never changes the tier/generation or demotes
  visible detail. Actual zoom changes still assign the new tier higher priority.
- Prepared frames stay stable during upload, even if more reads arrive.
- Frame IDs prevent a worker finishing between prepare/select from being lost.
- Root, body, tier changes and manual reset invalidate late workers. The occupied
  worker slot survives invalidation until return, bounding concurrent work.
- Leaving local view clears display/source geometry and cached compositions.
- Disk grids and cache files are unchanged. Both CPU and GPU consume the same
  clipped picture; legacy overlapping footprints are handled by `composition`.
