# handoff_tests.rs

## Purpose
Exercise asynchronous composition/publication with isolated coordinator state.

## Coverage
Partial detail replaces ready regions while retaining coarse remainder; new
read arrivals cannot starve GPU upload; workers finishing between prepare and
select are preserved. Rapid zoom reversal, source/body/reset invalidation,
no-source progress, offscreen eviction, and elevated cores are included.
Geometry clipping itself is covered by `composition_tests.rs`; real GPU upload
and old/new buffer coexistence remain covered by `local_line_gpu_tests.rs`.

The pan regression starts with a displayed fine tile, shifts the camera slightly,
then supplies base fallback with a missing fine manifest entry. The fine Arc and
LOD generation must survive, with base coverage only outside its core. Intentional
zoom-out still replaces it. Cold partial detail must also replace only its own
core without waiting for missing fine edge cells.
Fully covered base tiles must not trigger composition on every repeated loader
snapshot, and their shared handles must still be released by offscreen pruning.
