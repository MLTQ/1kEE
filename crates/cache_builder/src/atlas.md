# atlas.rs

Offline measured-height tile generation for the Atlas GUI and CLI. Discovers
GEBCO topography and optional SRTM rasters, plans geographic coverage through
`tile_archive::atlas`, runs bounded GDAL windows, and commits one checksummed
tile at a time. GEBCO ocean depths survive SRTM sea-zero pixels.

Source identity includes an algorithm revision plus canonical source paths,
lengths and modification timestamps. Matching tiles are checksum-verified before
reuse. Sources changing during a warp prevent publication. This is a metadata
identity, not a full input-file content hash; VRT dependencies are not followed.

Private per-tile temporary directories are removed on normal completion/error.
The output lock and SQLite transactions provide resumability independently of
OSM scratch folders. GDAL has a 120-second timeout, 64 MB cache/warp budgets and
one thread; progress heartbeats run off the GUI thread. Empty raster cores and
invalid raster dimensions fail without publishing blank tiles. Storage checks
leave at least 1 GiB of headroom before new writes. No downloads or background
planet-wide build are triggered merely by opening the tab.

User workflow and tier sizes: `../../../docs/atlas-cache.md`.
