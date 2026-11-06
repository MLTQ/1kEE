# Gravity anomaly maps

In **Layers → Gravity anomaly**, choose **Free-air**, **Bouguer**, or
**Isostatic residual**, then a region. The layer starts hidden and works offline
in Globe and Local Earth views. Restart the rebuilt app to see the new controls.
The selected map paints above SWOT VGG and below coastlines, contours and markers.

These are physically different anomaly corrections, measured in **mGal**
(1 mGal = 10⁻⁵ m/s²). They are not alternate projections, elevations, inferred
rock identities or the SWOT vertical gravity gradient measured in Eötvös.

| Quantity | Available regions | Original grid spacing |
|---|---|---|
| Terrain-corrected free-air | Contiguous U.S., Alaska, Hawaii, Mexico, Puerto Rico / Virgin Islands | 2 arcmin; Alaska 2 × 4 arcmin |
| Bouguer | Contiguous U.S.; Idaho batholith, Idaho, Maine, Nevada, Ohio, Utah, Minnesota | U.S. 4 km; regional 1.5–2.5 km |
| Isostatic residual | Contiguous U.S., Basin and Range, Idaho batholith, Nevada | U.S. 4 km; regional 2 km |

These **17 regional maps** retain their own coverage and missing cells. No
cross-product mosaic or synthetic global Bouguer/isostatic map is generated.
A region name describes the study grid, which may extend across its named border.
The broad SEG U.S. map uses **Bouguer on land and free-air offshore**; the selector
states this. Grid spacing is not guaranteed independent feature resolution.

Blue is negative, dark gray zero and orange positive. Color saturation is
±150 mGal for free-air, ±350 for Bouguer and ±100 for isostatic residual, with
the active range shown in the menu. The larger Bouguer range keeps the western
U.S. from becoming one saturated color. Values are not clipped in the data.
Missing/outside-grid areas stay transparent. No land mask is applied.

## Data drive and reproduction

The user's prepared installation lives on `/Volumes/Hilbert`:

- Raw binaries, headers, format declarations and notes:
  `Data/Gravity/NCEI/maps/<product>/`.
- Runtime grids and processing/source/checksum manifests:
  `Derived/gravity/ncei/<product>/`.

The 17 selected binary downloads total 24.0 MB; prepared assets occupy
42.1 MB including metadata. Data stays outside git. Mount the drive and use the
existing Data/Derived settings. Preparation requires Python 3.11+, curl and GDAL:

```bash
python3 scripts/fetch_ncei_gravity.py --asset-root /Volumes/Hilbert
```

`--only grav96us seggrav isores` prepares just the three broad U.S. products.
Re-running verifies finished products and resumes missing ones. Source byte
counts and SHA256 checksums pin the supported layouts. Differing existing
outputs are never overwritten. No routine reload is necessary; restart after
an intentional data replacement.

## Scientific conversion and source qualifications

The [NCEI gravity catalog](https://www.ncei.noaa.gov/products/gravity-data)
links these legacy NGS/USGS grids. Original source notes, URLs and checksums
remain with every product; see the bundled catalog in
`crates/desktop/src/model/gravity_maps.json` for exact source files.
The original compilations vary in age, correction assumptions and survey density.

Packed source values are converted to physical Float32 mGal. Southern-first
rows become north-up; each product's pixel edges, original missing sentinel
and scale are handled separately. Geographic NGS samples are retained exactly
apart from Float32 rounding. Projected sources are converted with nearest-neighbor
sampling finer than their original spacing. Reprojection does not create new
measured detail. Each selected image fits within a 4096-pixel texture limit; runtime preserves
every prepared pixel when supported by the GPU texture limit.

Two legacy metadata ambiguities require explicit treatment:

- ISORES's narrative says upper-left while its header says lower-left. Its
  binary missing mask agrees with SEG in 1,003,595 of 1,003,600 cells using the
  same lower-left order. Its actual physical missing value is 999999.99 mGal,
  rather than the header's rounded 999999. SEG and ISORES extents also span
  whole pixels despite a center-cell label; other selected grids span centers.
- Projected legacy headers omit the horizontal datum/ellipsoid. Preparation
  explicitly assumes Clarke 1866 without claiming a precise datum shift;
  each affected manifest records this assumption. NGS geographic NAD83
  coordinates are retained, an approximation to WGS84 far below their grid spacing.

The renderer reads/converts one selected map on a background worker and retains
no whole catalog in memory. Product/drive changes invalidate incompatible images.
Shared globe meshes honor the perspective horizon. Local rendering inverts the
actual affine viewport and clips the regional image before drawing, keeping
large-region imagery visible at close zoom and placing Alaska across the dateline.

## Validation

```bash
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts \
  -p 'test_fetch_ncei_gravity.py'
cargo test -p one-thousand-electric-eye-desktop gravity
cargo test -p one-thousand-electric-eye-desktop raster_mesh
cargo test -p one-thousand-electric-eye-desktop swot_layer
cargo test -p one-thousand-electric-eye-desktop real_hilbert_maps_load_at_native_detail \
  -- --ignored --nocapture
```

Fixtures cover physical scale, missing values, valid zero, row order,
registration, exact sizes, checksum publication, native detail, product identity,
regional bounds, deep Local zoom, pitch/yaw and dateline placement. Preparation
on Hilbert was checked against source coordinates/values across all 17 products.
