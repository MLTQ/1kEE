# Worldwide lidar survey discovery

On Earth, open **Layers → Public Data → Lidar surveys**. Opening the section starts a background catalog check. Search for a survey, file or provider, or move the map, check **100 km box at map center** and press **Search surveys**. Select a result to center the map and show its location and published coverage in Globe and Local. The search box matches substrings, for example **Brazilian Amazon EBA**, **Tapajós**, **Middle Usumacinta**, or **NOAA coastal**. Use Next for additional results.

**Survey record** opens the publisher's metadata. **Download scan file** opens an individual asset when available. The EBA records identify the transect and source filename and link to regional archives. Some publishers report a single location or a bounding rectangle. A footprint indicates where a survey took place. It carries no elevation values.

For OpenTopography surveys with a matching raster collection, pan to the part of the footprint you want to inspect and choose **Stream terrain at map center**. The worker reads intersecting bare-earth Cloud Optimized GeoTIFFs over HTTP and creates a georeferenced, 1024-pixel hillshade covering 2 km. The image appears alongside imported imagery in both Earth views and lasts for the session. Survey gaps can be transparent. If the catalog only offers a point cloud, use its access links. The existing Local **Hillshade → USGS 3DEP** and **WA LiDAR: bare earth** layers also stream terrain.

## Catalogs

| Feed | What the index contains | Access |
|---|---|---|
| [OpenTopography](https://opentopography.org/developers) | International lidar surveys plus federated USGS 3DEP and NOAA coastal holdings, including shallow-water topobathymetry | Survey footprints and publisher access pages. Matching [raster collections](https://portal.opentopography.org/stac_browser/) can supply streamable ground tiles. |
| [EBA Brazilian Amazon](https://zenodo.org/records/4968706) | Individual airborne transects from the 2016–2018 campaigns | Zenodo’s latest-release record and checksum identify changes to the footprint archive. Regional point-cloud archives are listed separately by the authors. The map outlines are simplified to five metres in the original projection. |
| [NASA ORNL DAAC Brazilian forest sites](https://doi.org/10.3334/ORNLDAAC/1644) | Geolocated granules from the 2008–2018 surveys | Individual files via Earthdata Login. |
| [NASA G-LiHT](https://gliht.gsfc.nasa.gov/) | Airborne point-cloud and digital terrain model granules | Public CMR metadata, individual files via Earthdata Login. |
| [swisstopo swissSURFACE3D](https://data.geo.admin.ch/api/stac/v1/collections/ch.swisstopo.swisssurface3d) | Geolocated Swiss lidar point-cloud tiles | Direct public scan archives. |

Acquisition dates, licenses, ground classifications and resolutions vary by survey. A point cloud and its derived raster can cover the same flight. Under water, airborne bathymetric lidar coverage depends on water clarity. NOAA’s [Bathymetric Data Viewer](https://www.ncei.noaa.gov/maps/bathymetry/) also identifies multibeam sonar surveys for offshore exploration.

## Refresh and acquisition

The harvester uses Python 3.11+ and the standard library. GDAL command-line tools handle the EBA footprint reprojection and streamed terrain. Bulk file downloads use curl. The catalog is stored at **Lidar/catalog.sqlite** inside the configured Data directory. Cached results can be searched offline. An open survey panel checks daily. Catalogs checked within 24 hours are reused, with a seven-day interval for paginated STAC inventories. Refresh compares HTTP validators when supplied and checks NASA's granule count and latest revision before paging. Each source is replaced in a separate transaction. A failed source can be retried while its last successful index remains available.

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

Use **--netrc** with your own curl/Earthdata login configuration for protected NASA files. **--all** selects all individually linked files. **--eba-archives --directory scans** downloads the Brazilian regional ZIPs. OpenTopography records with publisher-mediated access retain their survey links in the export. For other public collections, pass a lidar publisher’s HTTPS STAC collection **items** URL to **refresh --stac-items URL**, or a NASA lidar collection concept ID to **refresh --cmr-collection ID**. Repeat those arguments on subsequent refreshes to update those additional collections.
