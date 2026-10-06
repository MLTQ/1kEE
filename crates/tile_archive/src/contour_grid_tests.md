# contour_grid_tests.rs

## Purpose
Regression coverage for shared edges, aligned source halos, storage reduction,
line continuity, half-open ownership and both existing terrain decoders.
Native SRTM tests cover every source tier, both hemispheres, unchanged ownership,
one-arc-second sampling and a common global pixel phase across zoom changes.

## Contracts
Synthetic tests need no data or network. The ignored regional measurement opens
an explicitly selected cache read-only and clips only a few tiles in memory.
It does not represent a national, area-weighted storage benchmark.
