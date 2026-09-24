# Nonoverlapping Earth contour storage

As of September 2026, new Earth contour builds persist one disjoint core per
existing cache address. The change covers the desktop's SRTM and hosted 3DEP
paths plus the offline builder's GDAL and native SRTM engines.

## Why the old cache grew

Legacy tiles had width 2h, but their centers were only 0.45h apart. A fully
populated grid therefore stored approximately (2/0.45)^2 = 19.75 copies of the
same ground at each detail level. Packing those existing tiles alone does not
remove that overlap.

## New builds

Each address owns the square between the midpoints to its neighbors, width
0.45h. A two-pixel halo supplies shared interpolation/contouring samples. Only
geometry clipped to the owned square is committed; crossing edges retain their
shared endpoints. Lines along north/east boundaries belong to their neighbor.

The elevation interval is unchanged. Source resolution is rounded to a whole
number of pixels per core, keeping approximately the prior sample spacing.
A former 2400-square source becomes 540 core pixels plus four halo pixels, or
544-square: 94.86% fewer source pixels per address. TIFF/GeoPackage staging is
still deleted after import.

The viewport selects enough cores for the actual oblique view plus a spare
hosted ring. More small requests can be necessary than before; this is not a
claim of a 20x end-to-end latency improvement. Network latency, worker limits,
GPU uploads and old cache contents remain relevant. Loading indicators show
core footprints. Deep-tier per-tile reader budgets are preserved.

## Compatibility and existing storage

Tile addresses, database schema and archive encoding remain compatible. Existing
legacy rows and archives are read unchanged, including their outer geometry.
New builds coexist with them. New snapshots preserve already-clipped core data.

Existing files do not shrink automatically. Deleting their outer geometry in
place could remove the only cached coverage at a region edge. Safe reclamation
needs a separate migration that preserves those edges, verifies its output and
then reclaims old storage. No source cache or archive was rewritten here.
Moon/Mars remain on their prior storage layout.

## Validation

Synthetic tests verify core/halo alignment, shared boundary ownership, polyline
clipping without bridges, malformed-geometry rejection, legacy/packed decoding,
transaction rollback, manifest counts, and adjacent native/GDAL seam continuity.

Read-only clipping of three existing deepest-tier cache samples measured:

| Cache address (z/y/x) | Original geometry | Owned-core geometry | Reduction |
|---|---:|---:|---:|
| 10/5317/-12463 | 128,877,612 B | 6,118,602 B | 95.3% |
| 10/6360/-10670 | 19,191,358 B | 1,289,486 B | 93.3% |
| 10/6364/-10670 | 18,943,735 B | 780,217 B | 95.9% |

These compare stored geometry for individual addresses, not equal-coverage
whole-country totals, physical SQLite file reclamation or load-time speedups.
They do not establish a national compression ratio.

Reproduction: the tile-archive ignored benchmark_real_core_storage test takes
ONEKEE_CONTOUR_BENCH_DB and opens it read-only. The cache-builder ignored
gdal_core_tiles test takes GDAL_BIN_DIR and uses only synthetic temporary data.

A live USGS check fetched two neighboring 544-square Boston-area clips (about
1.64 MB each). All 2,176 overlapping pixel positions matched; the maximum elevation
difference was 0.0000009537 m, within Float32 rounding. This checks source-grid
alignment, not a nationwide guarantee about source-project seam quality.
