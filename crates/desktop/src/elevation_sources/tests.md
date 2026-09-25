# tests.rs

## Purpose
Test source routing, request axis/format contracts, URL and resource limits,
and actual GDAL normalization before allowing a provider raster into the cache.

## Contracts
Offline tests preserve zero/negative elevations, exact raster sample positions,
nodata rejection and scratch cleanup. GSI and Swiss/NZ tests live beside their
adapters. The ignored live test requests only six small windows, validates real
TIFF heights through the production fetch path, and removes its temporary data.
`ONEKEE_ELEVATION_TEST_PROVIDER` selects one provider when debugging; otherwise
all six must pass. No API credentials or production terrain data are required.
