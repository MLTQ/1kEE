# handoff.rs

## Purpose
Retain one displayed terrain generation across asynchronous LOD changes.

## Components
- `select` waits for decoded coverage, merge publication and GPU uploads before
  replacing visible terrain. Cold views can display CPU arrivals progressively.
- `preserves_coverage` compares geographic overlap across non-nested grids in
  either zoom direction. Counts and source availability alone are insufficient.
- `Display::prune` releases offscreen tile and GPU references. Only one prior
  CPU merge is retained until replacement, or until its last tile leaves view.

## Contracts
The scene stages candidate GPU batches without painting them. Display and
candidate tiles both retain instance cache entries during the transition.
Source-root/body changes, leaving local view, and manual reset clear retention.
Empty decoded cells count as coverage; failed/missing cells do not. The progress
grid continues describing the requested load while the old terrain is drawn.

Coverage checks invert the actual oblique projection at both elevation extremes;
visible terrain beyond the nominal ground envelope cannot be retired early.
