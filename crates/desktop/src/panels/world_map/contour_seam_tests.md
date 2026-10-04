# contour_seam_tests.rs

## Purpose
Regress terrain grids caused by dropping short contour fragments at tile edges.

## Coverage
- Adjacent synthetic Himalayan-address tiles each have 160 long interior paths,
  24 short shared-edge crossings and a legacy halo-only path. The old 120-path
  policy loses every crossing. Earth-core reads retain all 24 with bit-identical
  neighboring endpoints and discard only the unowned halo.
- SQLite and packed reads produce identical clipped geometry and ordering.
- All eleven Earth tiers retain more than the former 10,000-path deep-tier cap.
- Opt-in `cached_himalayan_cores_recover_boundary_detail` reads four adjacent
  installed 25 m contour tiles, measures retained core paths/edge endpoints and
  bounds, and optionally exports geometry for a visual comparison.

## Contracts
Normal tests use unique temporary databases and need no network/data drive.
Real data requires `ONEKEE_CONTOUR_BENCH_DB`; it is opened read-only. Optional
`ONEKEE_CONTOUR_SEAM_OUTPUT` receives the comparison JSON; source data is never
rebuilt, deleted or modified.
