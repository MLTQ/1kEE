# Retained Target ground DEM

Finds intersecting `kind='raster'` entries in the existing read-only bulk index,
checks their catalog identity and selects the newest available numeric ground
raster with verified metre heights. Paths must resolve under the LiDAR root.
Images and unknown/foot height units are not accepted as metre elevation data.
The Israel 2006 survey's metre units are explicitly supported from the publisher
metadata; horizontal CRS units alone are insufficient evidence for Z units.

Configured GDAL tools normalize a bounded 2048-square window to Float32
EPSG:4326, preserving nodata holes and applying no vertical shift. The source
file remains untouched. Native spacing, where verified, and the output sample
and display mesh spacing are labeled separately. Original point loading is
independent; combined point/DEM display still requires verified matching Z.
Unsupported or absent local rasters allow the existing service adapters to run.
