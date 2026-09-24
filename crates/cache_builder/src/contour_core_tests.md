# contour_core_tests.rs

## Purpose
Test production native and GDAL builders on adjacent synthetic terrain cores.

## Contracts
- Native test verifies clipped stored coordinates and identical seam endpoints.
- Explicit GDAL test uses the configured tools, temporary local data and the
  real raster/contour/import path; it needs no user data or network.
