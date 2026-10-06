# globe_contour_merge.rs

## Purpose
Compose Earth globe terrain as persistent disjoint tile batches on one worker.
An arriving tile no longer copies every existing contour into a giant vector.

## Components
- `ownership` partitions modern cores and legacy halos, including empty cores.
- `compose_resident` caches source bounds and clipped pieces by source Arc and
  owned regions. Unchanged pieces keep their identity through tile arrivals.
- `Frame` contains immutable tile handles and precomputed instance byte cost.
- Previous-tier fallback stays partitioned; replacement clips only overlapping
  pieces. Offscreen fallback tiles are retired under byte pressure.
- `render` coalesces arrivals. `finish` rejects reset/root/tier/view revisions,
  accepts coherent intermediate arrivals, and releases the single worker gate.

## Contracts
No coordinate simplification or joins across gaps. Camera movement alone does
not rebuild geometry. Empty decoded tiles own their cores. Root changes discard
fallback; normal zoom changes retain uncovered coverage. The GPU tile renderer
uses stable contour Arcs to reuse uploads and culls retained offscreen pieces.
