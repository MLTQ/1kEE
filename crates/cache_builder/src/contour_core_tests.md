# contour_core_tests.rs

## Purpose
Test production native and GDAL builders on adjacent synthetic terrain cores.

## Contracts
- Native test verifies clipped stored coordinates and identical seam endpoints.
- Both engines exercise the globally aligned native SRTM grid, rather than the
  historical downsampled raster for their tile address.
- Explicit GDAL test uses the configured tools, temporary local data and the
  real raster/contour/import path; it needs no user data or network.
