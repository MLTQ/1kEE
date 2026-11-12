# Worldwide lidar survey discovery

Turn on **Lidar** in the Earth map toolbar or under **Layers → Base**. On narrow screens the toolbar switch is in **View**. The layer starts with survey boundaries in both Globe and Local. Hover over a boundary or its interior, then right-click. When a single survey covers the point and has map data, it loads immediately. Where surveys overlap, choose a name and acquisition date from the menu. At a globe overview, choosing a survey opens Local at the clicked trace. In a detailed view you keep your position and zoom. Archive-only entries link to the publisher’s survey record. Unsampled areas stay transparent.

Open **Lidar surveys** on the map toolbar or **Browse by region** in the Lidar surveys drawer. In **Worldwide**, pick a continent and country, or open **Oceans** and pick a named sea, gulf or ocean basin. Select a survey to open its footprint in Local view. Within each location surveys are alphabetical, with acquisition dates beside their names. Larger lists have a name or ID filter.

Choose **At map center** in the same picker to switch between overlapping surveys while keeping your position and zoom. **Automatic** loads visible measurements as you zoom. **Boundaries only** clears the active survey and returns to browsing outlines. The worldwide directory uses the center of each survey’s catalog extent and known national collections. Coastal surveys can appear under both a country and a marine region. The local list uses the actual survey footprint at the point where you opened the menu. The app ships the country outlines and marine regions for offline lookup. [Natural Earth marine regions](https://www.naturalearthdata.com/downloads/110m-physical-vectors/110m-physical-labels/)

To find a survey elsewhere, open **Layers → Public Data → Lidar surveys**. Search by part of a name or ID, such as **NOAAID:38**, and select the result. This enables the layer and opens a close Local view. **Clear selection** returns to boundaries only. The survey record and download links open the publisher's material.

1kEE saves each completed 512 × 512 map tile under **Lidar/maps/v1** in the configured Data directory. Both views reuse that cache across launches and offline. Tiles use a north-up longitude/latitude grid. PNG metadata records the contributing surveys. Point-cloud blocks and service responses are reused from **Lidar/map-source-cache**. Catalog changes invalidate saved path outlines and empty results. Lidar and webcam markers start hidden on each launch. Turning either layer on lasts for the current session. Saved lidar tiles remain available on disk.

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

The harvester uses Python 3.11+ and the standard library. GDAL command-line tools handle the EBA footprint reprojection and streamed terrain. The map worker uses uv to provision NumPy, Pillow, rasterio, pyproj and laspy with lazrs on first use. A Python environment with those packages also works when uv is absent. INFOMAR ZIP access uses the GDAL command-line tools including `gdal vsi list`. Bulk file downloads use curl. The catalog is stored at **Lidar/catalog.sqlite** inside the configured Data directory. Cached results can be searched offline. Enabling the layer checks the catalogs daily. Catalogs checked within 24 hours are reused, with a seven-day interval for paginated STAC inventories. Refresh compares HTTP validators when supplied and checks NASA's granule count and latest revision before paging. Each source is replaced in a separate transaction. A failed source can be retried while its last successful index remains available.

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

For protected NASA terrain GeoTIFFs, choose **Data/Lidar/scans** as the download directory to make a locally acquired raster available to the map worker.

Use **--netrc** with your own curl/Earthdata login configuration for protected NASA files. **--all** selects all individually linked files. **--eba-archives --directory scans** downloads the Brazilian regional ZIPs. OpenTopography records with publisher-mediated access retain their survey links in the export. For other public collections, pass a lidar publisher’s HTTPS STAC collection **items** URL to **refresh --stac-items URL**, or a NASA lidar collection concept ID to **refresh --cmr-collection ID**. Repeat those arguments on subsequent refreshes to update those additional collections.

## Worldwide bulk downloads

On Linux, the bulk downloader uses the existing catalog, Python 3.11+, requests, and GDAL. It acquires actual scan files and elevation products across NOAA, OpenTopography, LINZ, INFOMAR, swisstopo and the Brazilian EBA archive. It rotates between geographic areas and runs two transfers per provider by default. Coverage depends on the contributing surveys. Earthdata login products are available separately through the catalog download command above.

~~~sh
python3 scripts/lidar_bulk.py --volume-uuid 6FEF-269A --db /run/media/m/Minkowski/Data/Lidar/catalog.sqlite
~~~

These examples use Minkowski’s filesystem UUID, `6FEF-269A`. For a different volume, check its UUID with `lsblk -f`. The job verifies the UUID before opening its lock or writing data. It pins its working directory to the volume and keeps temporary scan data under **Lidar/scratch**.

The default reserve is **100 GB** (100,000,000,000 bytes). The downloader checks the destination volume, accounts for simultaneous transfers, and reuses completed files. Interrupted transfers resume when the publisher supplies a checksum or supports validated byte ranges.

Open **Data/Lidar/scans** to manage storage. The first two directories are the continent and country, for example **South America/Brazil** or **Oceania/New Zealand**. For cross-border and coastal files the downloader uses the tile center and nearby coastline. National surveys and Brazilian regional archives use their published country. Each filename includes a short identifier so different acquisitions with the same publisher filename can coexist.

Delete a country directory to reclaim its scan storage. Keep **Lidar/bulk.sqlite** and **Lidar/catalog.sqlite**. They retain source URLs, geographic extents and Brazilian ZIP member names. The worldwide job skips country directories you removed. Existing rendered tiles stay available under **Lidar/maps**. When a new view needs an evicted scan, 1kEE reads it from the publisher and saves the resulting tile. Availability then depends on the publisher and your connection.

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
