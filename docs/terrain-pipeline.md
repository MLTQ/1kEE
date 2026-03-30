# Terrain Pipeline

## Purpose
This document defines the first practical preprocessing path from the raw terrain datasets already checked into `Data/` to runtime-friendly assets for the 1kEE globe.

## Current Source Inventory

- `Data/GEBCO/gebco_2025_sub_ice_topo_geotiff/`: eight 90x90 degree GeoTIFF tiles at 15 arc-second resolution
- `Data/GEBCO/gebco_2025_tid_geotiff/`: matching TID provenance tiles
- `Data/natural_earth/GRAY_HR_SR_OB_DR/GRAY_HR_SR_OB_DR.tif`: global grayscale shaded relief
- `Data/srtm_gl1/`: partial SRTM 1 arc-second mirror with a VRT and individual tiles

## Recommended Runtime Strategy

1. Use GEBCO as the global base terrain.
2. Use Natural Earth as the fallback or artistic shaded-relief layer.
3. Use SRTM later as a regional refinement source for selected hotspots once the full mirror is available.

Do not read the raw global rasters directly from the UI thread at runtime. Preprocess them into derived assets first.

## Derived Assets To Produce

- `Derived/terrain/gebco_2025_global.vrt`
- `Derived/terrain/gebco_2025_preview_4096.tif`
- `Derived/terrain/gebco_2025_contours_200m.gpkg`
- `Derived/terrain/gebco_2025_contours_500m.gpkg`
- `Derived/terrain/natural_earth_relief_4096.tif`

These are enough for:
- low-cost globe shading
- real contour extraction
- future regional mesh or overlay generation

## Current Local Outputs

The following derived assets have already been generated in this repository:

- `Derived/terrain/gebco_2025_global.vrt`
- `Derived/terrain/gebco_2025_preview_4096.tif`
- `Derived/terrain/gebco_2025_preview_4096.png`
- `Derived/terrain/gebco_2025_contours_500m.gpkg`
- `Derived/terrain/gebco_2025_contours_200m.gpkg`
- `Derived/terrain/natural_earth_relief_4096.tif`
- `Derived/terrain/natural_earth_relief_4096.png`

The contour GeoPackages currently use a single layer named `contour`.

## GDAL Commands

Run from repo root.

### 1. Build a virtual global GEBCO mosaic

```bash
mkdir -p Derived/terrain
gdalbuildvrt Derived/terrain/gebco_2025_global.vrt Data/GEBCO/gebco_2025_sub_ice_topo_geotiff/*.tif
```

### 2. Produce a smaller preview raster for fast iteration

```bash
gdal_translate \
  -outsize 4096 2048 \
  -ot Int16 \
  -of GTiff \
  Derived/terrain/gebco_2025_global.vrt \
  Derived/terrain/gebco_2025_preview_4096.tif
```

### 3. Extract global contours from the downsampled preview first

```bash
gdal_contour \
  -i 200 \
  -a elevation_m \
  Derived/terrain/gebco_2025_preview_4096.tif \
  Derived/terrain/gebco_2025_contours_200m.gpkg
```

```bash
gdal_contour \
  -i 500 \
  -a elevation_m \
  Derived/terrain/gebco_2025_preview_4096.tif \
  Derived/terrain/gebco_2025_contours_500m.gpkg
```

### 4. Produce the depth-fill BIL grid for the globe texture layer

Used by `gebco_depth_fill.rs` to render ocean depth as a coloured texture on
the globe.  The 1440×720 size matches 0.25°/pixel — fast to load, sufficient
for a full-globe visualisation.

```bash
gdal_translate \
  -outsize 1440 720 \
  -ot Int16 \
  -of EHdr \
  Derived/terrain/gebco_2025_preview_4096.tif \
  Derived/terrain/gebco_depth_1440x720.bil
```

This produces both `gebco_depth_1440x720.bil` and its companion
`gebco_depth_1440x720.hdr` (written automatically by GDAL's EHdr driver).
The app expects little-endian Int16 values; `BYTEORDER I` in the .hdr confirms
this.  Positive values and the NODATA sentinel (−32767) are rendered
transparent so land shows the globe background.

### 6. Produce a Natural Earth fallback raster

```bash
gdal_translate \
  -outsize 4096 2048 \
  -of GTiff \
  Data/natural_earth/GRAY_HR_SR_OB_DR/GRAY_HR_SR_OB_DR.tif \
  Derived/terrain/natural_earth_relief_4096.tif
```

### 7. Produce runtime-friendly PNG assets for in-app sampling

```bash
gdal_translate \
  -scale -11000 9000 0 65535 \
  -ot UInt16 \
  -of PNG \
  Derived/terrain/gebco_2025_preview_4096.tif \
  Derived/terrain/gebco_2025_preview_4096.png
```

```bash
gdal_translate \
  -ot Byte \
  -of PNG \
  Derived/terrain/natural_earth_relief_4096.tif \
  Derived/terrain/natural_earth_relief_4096.png
```

## SRTM Plan

When the full SRTM mirror is available:

1. Build a world or regional VRT.
2. Limit SRTM use to land hotspots where higher resolution matters.
3. Keep GEBCO as the global default, because it covers oceans and poles and stays coherent at planetary scale.

## Notes

- GEBCO is the right first runtime source because it is already global and tiled cleanly.
- SRTM is higher resolution over land but should be treated as a second-stage enhancement, not the primary global source.
- The TID grid should be preserved for later provenance overlays or confidence masking.
- `gdal_contour` emitted repeated GeoPackage RTree warnings during generation, but the resulting files are valid and queryable with `ogrinfo`.
