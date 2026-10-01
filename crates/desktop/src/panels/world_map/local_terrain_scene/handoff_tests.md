# handoff_tests.rs

## Purpose
Network-free regression fixtures for terrain coverage across non-nested grids.

## Contracts
Tests require the last covering tile, published merge, and last GPU upload
before replacing known terrain. Exercise zoom in/out, rapid reversal, partial
views, decoded empty versus unavailable data, rotated culling, panning, roots,
and body switches. Fixtures own their state; no global render caches are reset.

Coverage checks invert the actual oblique projection at both elevation extremes;
visible terrain beyond the nominal ground envelope cannot be retired early.
