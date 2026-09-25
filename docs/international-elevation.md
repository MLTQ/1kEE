# International detailed terrain

In **Settings → Paths → Detailed Terrain**, leave **Stream detailed terrain**
enabled to acquire elevation as you explore. The same control enables USGS and
the international adapters; existing saved settings continue to work.

| Area | Official source | Nominal raster spacing |
|---|---|---|
| England | [Environment Agency DTM](https://www.data.gov.uk/dataset/01b3ee39-da3f-47b6-83da-dc98e73a461f/lidar-composite-digital-terrain-model-dtm-1m) | 1 m |
| Metropolitan France | [IGN LiDAR HD MNT](https://www.data.gouv.fr/datasets/mnt-lidar-hd) | 0.5 m |
| Netherlands | [AHN / PDOK](https://www.ahn.nl/dataroom) | 0.5 m |
| Switzerland | [swissALTI3D](https://www.swisstopo.admin.ch/en/height-model-swissalti3d) | 0.5 or 2 m |
| New Zealand main islands | [LINZ national merged DEM](https://github.com/linz/elevation) | 1 m |
| Japan | [GSI elevation tiles](https://maps.gsi.go.jp/development/ichiran.html#dem) | 1 m where available, then 5 m and 10 m |

These are bare-earth heights, not satellite imagery or building-surface models.
Raster spacing differs from contour interval and measurement accuracy. Coverage
and capture dates vary; Switzerland's finest grid can include oversampling,
and Japan does not have nationwide 1 m coverage. Zooming further cannot create
new survey detail.

New contours enter the existing Earth SQLite cache and are included by the
existing `.1ka` terrain packing workflow. This is **on-demand acquisition**,
not an automatic national/planetary mirror. Contour input rasters are removed
after processing. Elevation chunks needed for markers and other map layers use
the existing adjustable cache budget. The historical directory name `3dep_1m`
also holds these international chunks for compatibility.

Where finer tiles are absent, the SRTM fallback remains available. A timeout,
server error, or decoder failure is retried with backoff rather than written as
permanent empty terrain. Completed offline contour tiles are checked first.
The optional server-rendered hillshade drape remains USGS-only; the ordinary
contour-based elevation fill works with the international terrain.

The installed GDAL tools perform small-area reprojection. New Zealand's LERC
files use **LIBERTIFF, available in GDAL 3.11+**; the tested machine has 3.12.2.
Downloads and processing remain bounded, with no API keys required for these
public endpoints. Settings includes source credits and links to terms:
© Environment Agency 2022 (Open Government Licence), AHN/PDOK, © IGN,
© swisstopo, Toitū Te Whenua LINZ (CC BY 4.0), and the Geospatial Information
Authority of Japan. Contours are derived by 1kEE.

## Verification

Official service metadata and six small live elevation windows were checked on
2026-09-25. Offline tests cover signed GSI decoding, nodata, interpolation across
PNG edges, native grid selection, latest Swiss survey selection, axis order,
numeric source layers, resource bounds and normalization. An opt-in integration
test builds a Tokyo contour tile through clipping and SQLite publication, then
checks source-raster cleanup. These samples verify the pipeline; they do not
assert complete survey coverage in every country.
