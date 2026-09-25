# services.rs

## Purpose
Fetch small numeric bare-earth windows from England EA, Netherlands AHN/PDOK,
and France IGN LiDAR HD without downloading full survey tiles.

## Contracts
WCS 1.0 uses longitude/latitude EPSG:4326 bounds. IGN WMS 1.3 uses CRS:84 to
retain that axis order. The exact IGN MNT numeric WGS84G layer and GeoTIFF
format are required; SHADOW, DSM/MNS and rendered PNG layers are inappropriate.
Responses are bounded downloads and normalized/validated by raster.rs. Official
GetCapabilities endpoints and 64×64 Float32 live samples were verified during
integration. Partial or failed requests cannot replace an existing cache file.
