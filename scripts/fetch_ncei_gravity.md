# fetch_ncei_gravity.py

## Purpose
Download the selected NCEI legacy grids and prepare regional physical-mGal
rasters for the Earth layer. Requires Python 3.11+, curl and GDAL.

## Components
- `fetch` resumes interrupted downloads and publishes only completed responses.
- `physical_grid` decodes catalog-pinned little-endian samples, applies physical
  scales, reverses southern-first rows and maps documented missing sentinels.
- `write_vrt` attaches the curated source projection and exact pixel edges. The raw
  VRT references only its sibling file, matching GDAL restricted-access rules.
- `prepare` preserves geographic samples or reprojects projected rasters with
  nearest-neighbor sampling finer than source spacing. Validates dimensions,
  registration, format and length before atomically publishing each product.
- `main` checks the data drive, source SHA256/byte counts and existing prepared
  checksums. Re-running verifies completed products and resumes remaining work.

## Contracts
One catalog in model/gravity_maps.json supplies import specs and UI labels.
Raw files and notes go to Data/Gravity/NCEI/maps; derived files go to
Derived/gravity/ncei/<id>. No payload goes into git. Runtime format is north-up
little-endian Float32 mGal, sentinel -999999, geographic pixel-edge bounds in
manifest.json. No land mask, cross-product mosaic or inferred Bouguer/isostatic
correction. Source projection assumptions/metadata conflicts remain explicit in
catalog and manifest. Existing differing output is never overwritten.
