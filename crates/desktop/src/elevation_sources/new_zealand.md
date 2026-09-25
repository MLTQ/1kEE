# new_zealand.rs

## Purpose
Read small windows of LINZ's national merged 1 m bare-earth DEM using its
public STAC catalog and range-readable cloud GeoTIFFs, without an API key.

## Contracts
EPSG:2193 bounds select Topo50 24×36 km cells from the published AS21 origin.
Selected codes must exist in the current national collection; their item
metadata supplies the actual TIFF URL. A request reads at most 16 tiles, never
enumerates hundreds of per-item metadata documents, and never downloads the
full large TIFF. Raster decoding uses LIBERTIFF for LERC compression. The
adapter targets New Zealand's main-island grid; unsupported areas retain SRTM.
