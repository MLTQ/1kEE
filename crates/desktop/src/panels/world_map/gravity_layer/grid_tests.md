# grid_tests.rs

## Purpose
Verify the runtime reader against physical raster contracts and image limits.

## Contracts
Asymmetric fixtures pin north-up row order, valid zero, transparent missing and
nonfinite values, mGal palette clipping, unwrapped Alaska bounds, native detail
above 2048 pixels, GPU limits, incorrect product/units/bounds and short files.
An explicit ignored smoke test loads every real prepared map on Hilbert.
