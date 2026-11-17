# Lidar layer and source data

Enable **Layers → Base → Lidar** in the Earth sidebar. This is the only LiDAR
control. It starts off on each launch and stays enabled when switching views.

- **Globe** shows all cataloged survey boundary lines, including surveys that
  have not been downloaded or prepared. These outlines indicate published
  coverage, not a guarantee of local fused data.
- **Local** shows teal catalog survey outlines and the prepared fused elevation
  layer from **Lidar/fused/v1**. Outlines include unprepared surveys, allowing
  Target placement before source points or fused rasters have loaded.
  Newer measurements take precedence, with older measurements filling gaps.
  Missing USGS 3DEP views are prepared in the background from public EPT blocks
  and cached as numeric rasters at the current view resolution. The sidebar
  reports preparation while these tiles load. Other providers currently need
  locally prepared scans; areas without measurements stay transparent. Zoom in
  to prepare finer tiles. Cached tiles work offline.

There is no top-bar LiDAR button, survey search, regional picker, or boundary
click action. Switching views preserves the camera and the layer toggle.
Catalogs still refresh in the background to keep the globe boundaries current.

For original measured points, use the separate **Local → TARGET** tool to open
an adjustable 0.1–100 km radius cut-out with free flight, source selection and class
filters. See [Target inspection](target-mode.md) for controls and accuracy limits.

Completed 512 × 512 display tiles are cached under **Lidar/maps/v1** in the
configured Data directory. Globe boundary tiles and Local fused tiles have
separate cache variants and are reused offline. PNG metadata retains contributing
survey credits. New prepared rasters invalidate affected Local tiles as the
background converter makes progress.

See [fused LiDAR preparation and benchmarks](lidar-fusion.md) for resolution,
source coverage, resumable conversion and retention limits. Source discovery and
bulk acquisition remain available as independent command-line tools below.

## Detailed terrain contours

The fine cyan terrain lines are a separate bare-earth elevation pipeline. In
the U.S. this uses [USGS 3DEP](https://www.usgs.gov/3d-elevation-program/about-3dep-products-services)
and now [NCEM / NC OneMap's statewide LiDAR-derived DEM](https://services.gis.nc.gov/secure/rest/services/Elevation/DEM03/ImageServer)
in North Carolina. The latter fills areas such as Kinston where the USGS query
did not advertise a standard one-metre product. Its approximately 0.95 m grid
and original elevation values in US survey feet are normalized to metres for
the existing contour and point-sampling renderer; no vertical datum conversion
is claimed. Six international source adapters also remain available where their
publishers provide data.

At close zoom, `0.5 m contours` describes vertical line spacing, not horizontal
pixel resolution or guaranteed measurement accuracy. A narrow detailed patch
with a status such as `SOURCE 2/9` can simply be the completed portion of a
larger loading view. The surrounding coarse terrain remains until detailed
source windows and geometry are ready. Fine coverage probes now include survey
edges and retry temporary catalog failures instead of marking them permanently
uncovered. Fine-only USGS exports leave actual coverage gaps empty rather than
resampling a coarse source into apparent fine detail.

## Catalogs

| Feed | What the index contains | Access |
|---|---|---|
| [OpenTopography](https://opentopography.org/developers) | International lidar surveys plus federated USGS 3DEP and NOAA coastal holdings, including shallow-water topobathymetry | Survey footprints and publisher access pages. Published NOAA and [USGS 3DEP point clouds](https://registry.opendata.aws/usgs-lidar/) stream by spatial block. Matching [raster collections](https://portal.opentopography.org/stac_browser/) provide bare-earth tiles. |
| [Toitū Te Whenua LINZ coastal elevation](https://github.com/linz/coastal) | New Zealand 1 m coastal lidar DEMs, including measured seabed | Read intersecting areas from the publisher’s Cloud Optimized GeoTIFFs. |
| [INFOMAR](https://www.infomar.ie/maps/Data-Access-and-Download) | Irish airborne bathymetric lidar survey legs | Stream the published shaded-relief GeoTIFFs. Depth grids and XYZ soundings remain available from the survey search. |
| [EBA Brazilian Amazon](https://zenodo.org/records/4968706) | Individual airborne transects from the 2016–2018 campaigns | Zenodo’s latest-release record and checksum identify changes to the footprint archive. Regional point-cloud archives are listed separately by the authors. The map outlines are simplified to five metres in the original projection. |
| [NASA ORNL DAAC Brazilian forest sites](https://doi.org/10.3334/ORNLDAAC/1644) | Geolocated granules from the 2008–2018 surveys | Individual files via Earthdata Login. |
| [NASA G-LiHT](https://gliht.gsfc.nasa.gov/) | Airborne point-cloud and digital terrain model granules | Public CMR metadata, individual files via Earthdata Login. |
| [swisstopo swissSURFACE3D](https://data.geo.admin.ch/api/stac/v1/collections/ch.swisstopo.swisssurface3d) | Geolocated Swiss lidar point-cloud tiles | Direct public scan archives and the [official lidar-derived shaded surface](https://www.swisstopo.admin.ch/en/height-model-swisssurface3d-raster). The map uses the published mosaic, which includes buildings and vegetation. |

Acquisition dates, licenses, ground classifications and resolutions vary by survey. A point cloud and its derived raster can cover the same flight. Under water, airborne bathymetric lidar coverage depends on water clarity. NOAA’s [Bathymetric Data Viewer](https://www.ncei.noaa.gov/maps/bathymetry/) also identifies multibeam sonar surveys for offshore exploration.

## Refresh and acquisition

The harvester uses Python 3.11+ and the standard library. GDAL command-line tools handle the EBA footprint reprojection and streamed terrain. The globe boundary worker uses uv to provision NumPy and Pillow on first use; Local reads the fused archive directly in Rust. Standalone raw-source rendering tools additionally use rasterio, pyproj and laspy with lazrs. INFOMAR ZIP access uses the GDAL command-line tools including `gdal vsi list`. Bulk file downloads use curl. The catalog is stored at **Lidar/catalog.sqlite** inside the configured Data directory. Cached catalog boundaries remain available offline. Enabling the layer checks the catalogs daily. Catalogs checked within 24 hours are reused, with a seven-day interval for paginated STAC inventories. Refresh compares HTTP validators when supplied and checks NASA's granule count and latest revision before paging. Each source is replaced in a separate transaction. A failed source can be retried while its last successful index remains available.

From the repository root, substitute your configured Data directory for **Data**. The CLI also works independently of the application.

~~~sh
python3 scripts/lidar_catalog.py --db Data/Lidar/catalog.sqlite refresh
python3 scripts/lidar_catalog.py --db Data/Lidar/catalog.sqlite export \
  --source eba --bbox -55 -4 -54 -2 --output tapajos.geojson
python3 scripts/lidar_catalog.py --db Data/Lidar/catalog.sqlite terrain \
  --id opentopoID:OTLAS.042022.32615.1 --lon -91.39 --lat 17.51 --km 2 --output usumacinta.tif
~~~

Import either output with **Earth → Layers → Import Layer…**. Bounding boxes use longitude/latitude in west, south, east, north order and select intersecting extents. GeoJSON properties preserve the source ID, acquisition date, publisher URL and available asset or collection links.

List direct scan files before choosing a download directory. The EBA option discovers the regional ZIPs from Zenodo and reports their published sizes in bytes. A completed ZIP is checked against the publisher checksum. A subsequent run skips files already saved.

~~~sh
python3 scripts/lidar_catalog.py --db Data/Lidar/catalog.sqlite download --eba-archives --list
python3 scripts/lidar_catalog.py --db Data/Lidar/catalog.sqlite download \
  --source nasa-brazil --bbox -55 -4 -54 -2 --list
python3 scripts/lidar_catalog.py --db Data/Lidar/catalog.sqlite download \
  --source nasa-brazil --bbox -55 -4 -54 -2 --max-files 1 --netrc --directory scans
~~~

The catalog CLI can save NASA LAS, LAZ, gzipped LAS, or GeoTIFF files under a
chosen scan directory. Protected files require your Earthdata Login for
**urs.earthdata.nasa.gov**. These raw-source tools run independently of the
single sidebar layer; their outputs need preparation before they appear there.


Use **--netrc** with your own curl/Earthdata login configuration for protected NASA files. **--all** selects all individually linked files. **--eba-archives --directory scans** downloads the Brazilian regional ZIPs. OpenTopography records with publisher-mediated access retain their survey links in the export. For other public collections, pass a lidar publisher’s HTTPS STAC collection **items** URL to **refresh --stac-items URL**, or a NASA lidar collection concept ID to **refresh --cmr-collection ID**. Repeat those arguments on subsequent refreshes to update those additional collections.

## Worldwide bulk downloads

On Linux, the bulk downloader uses the existing catalog, Python 3.11+, requests, and GDAL. It acquires actual scan files and elevation products across NOAA, OpenTopography, LINZ, INFOMAR, swisstopo and the Brazilian EBA archive. It rotates between geographic areas and runs two transfers per provider by default. Coverage depends on the contributing surveys. NASA Earthdata products are acquired through the catalog download command above.

~~~sh
python3 scripts/lidar_bulk.py --volume-uuid 6FEF-269A --db /run/media/m/Minkowski/Data/Lidar/catalog.sqlite
~~~

These examples use Minkowski’s filesystem UUID, `6FEF-269A`. For a different volume, check its UUID with `lsblk -f`. The job verifies the UUID before opening its lock or writing data. It pins its working directory to the volume and keeps temporary scan data under **Lidar/scratch**.

The default reserve is **100 GB** (100,000,000,000 bytes). The downloader checks the destination volume, accounts for simultaneous transfers, and reuses completed files. Interrupted transfers resume when the publisher supplies a checksum or supports validated byte ranges.

Open **Data/Lidar/scans** to manage storage. The first two directories are the continent and country, for example **South America/Brazil** or **Oceania/New Zealand**. For cross-border and coastal files the downloader uses the tile center and nearby coastline. National surveys and Brazilian regional archives use their published country. Each filename includes a short identifier so different acquisitions with the same publisher filename can coexist.

Delete a country directory to reclaim its scan storage. Keep **Lidar/bulk.sqlite** and **Lidar/catalog.sqlite**. They retain source URLs, geographic extents and Brazilian ZIP member names. The worldwide job skips country directories you removed. Existing rendered tiles stay available under **Lidar/maps**, and prepared rasters stay under **Lidar/fused**. USGS views can prepare new overviews from the publisher; other providers require prepared local data. Availability of new USGS views depends on the publisher and your connection.

To download a previously saved country again, use its folder name. The command also works while the worldwide job is running. A country with territory in multiple continent directories is restored across those directories.

~~~sh
python3 scripts/lidar_bulk.py --volume-uuid 6FEF-269A --db /run/media/m/Minkowski/Data/Lidar/catalog.sqlite --restore-country Brazil
~~~

For the running Minkowski job, follow progress with `journalctl --user -fu 1kee-lidar-bulk`. Stop it with `systemctl --user stop 1kee-lidar-bulk`. To resume after stopping or rebooting, rerun the bulk command above, or start it detached from the terminal with the following command from the checkout.

~~~sh
systemd-run --user --unit=1kee-lidar-bulk --working-directory="$PWD" \
  --property=ProtectSystem=strict --property=ProtectHome=read-only \
  --property=ReadWritePaths=/run/media/m/Minkowski/Data/Lidar \
  /usr/bin/python3 -Bu scripts/lidar_bulk.py --volume-uuid 6FEF-269A --db /run/media/m/Minkowski/Data/Lidar/catalog.sqlite
~~~
