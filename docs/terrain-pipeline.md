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

## USGS 3DEP 1 m (on demand, not mirrored)

The 3DEP 1 m bare-earth holding is hundreds of terabytes — a single state runs
500 GB to 1 TB — so it is deliberately **not** part of the `Data/` mirror. It is
consumed live from the dynamic image service:

`https://elevation.nationalmap.gov/arcgis/rest/services/3DEPElevation/ImageServer`

### Why this rather than the S3 COGs

`s3://prd-tnm/StagedProducts/Elevation/1m/Projects/` holds the raw source COGs
and is readable through `/vsicurl/` range requests, but the tiles are organised
per lidar project in mixed UTM zones. Using them means solving project discovery
and cross-projection mosaicking locally. The image service already does both,
server-side, for an arbitrary bounding box.

### What is fetched, and what is kept

| Path | Request | Persisted product |
|---|---|---|
| Deep-zoom contours | One `exportImage` clip per contour tile at the tile's raster size | Contour geometry in `srtm_focus_cache.sqlite`; the GeoTIFF is deleted |
| Elevation sampling | 0.02° chunks, cached as DEFLATE Float32 COGs | `Derived/terrain/3dep_1m/`, LRU-evicted against a configurable budget |
| Hillshade | `exportImage` with the `Hillshade Multidirectional` rendering rule | Nothing on disk; an in-memory texture |

The contour path is the important one: contours are perhaps two orders of
magnitude smaller than the raster they came from, so the durable cost of visiting
an area is contour geometry, not elevation data.

### Measured service behaviour

- `exportImage` returns HTTP 500 once the encoded response passes roughly 32 MB.
  2400×2400 Float32 (~22 MB) is comfortably inside that; 3000×3000 is not.
- Failures come back as an HTML page, not an error status, so responses are
  validated by magic number.
- The catalog `query` endpoint reports the finest available pixel size under a
  point, which is a free availability probe. `LowPS: 1` means 1 m source exists.
- Coverage is wider than expected — rural Nevada is 1 m — but Alaska is mostly
  3 m or coarser and nothing outside the US is served.

### Zoom tiers

Buckets 0–6 keep their SRTM sources unchanged. Buckets 7–10 are new and source
3DEP, extending the ladder from 0.16° half-extent down to 0.006° (~1.3 km
across) at a 0.5 m interval. Before this, `local_render_zoom` clamped the tile
spec at zoom 20 while the visual scale kept going to 60, so everything below
~19 km across was interpolated from a 40 m/px raster.

