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
