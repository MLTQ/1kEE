# Nonoverlapping Earth contour storage

As of September 2026, new Earth contour builds persist one disjoint core per
existing cache address. The change covers the desktop's SRTM and hosted 3DEP
paths plus the offline builder's GDAL and native SRTM engines.

The current SRTM quality policy uses native-resolution samples at every view
scale, as described below. The initial core-storage conversion preserved the
older coarse sampling; the native policy supersedes that sampling choice.

## Native SRTM shapes at every scale

Earth SRTM generation now uses the original GL1 one-arc-second posts (roughly
30 m), globally aligned across all source tiers. Zoom changes tile/request
footprints and selected elevation planes, never the underlying raster detail.
Wider views generate 200/100/50/25 m planes, followed by 10/5/5 m closer in.
All retained contours keep their complete vertices; no curve smoothing invents
terrain between measurements. Existing hosted-detail sampling is unchanged.

Desktop and offline builds share `CoreTile::srtm`. Native geometry lives in
`Derived/terrain/srtm_native_v1.sqlite`, with a source-quality tag. Untagged
nonempty caches cannot be relabelled as native. Old `srtm_focus_cache.sqlite`
data stays on disk; only its hosted tiers are still used. A fresh native cache
populates on demand, so previously visited SRTM regions must be generated once
again. Existing packed coarse snapshots do not override this new database.

The native Yemen test built widest-tier core 0/9/27 using a 5,837-pixel source
raster and 200 m elevation planes. It produced 10,013 paths / 1,642,134 segments
in about eight seconds on the validation machine, with a median segment axis
span of 0.0002327 degrees. This verifies native sampling, not live frame rate.
Native geometry costs more memory/storage than the earlier coarse cache.
Reproduce with `native_yemen_build_keeps_source_resolution_at_globe_scale`,
`ONEKEE_SRTM_ROOT`, and optional `ONEKEE_NATIVE_OUTPUT`. Outputs are temporary;
the installed source/caches are untouched. Synthetic native/GDAL seam tests
also check matching neighbor endpoints.

## Why the old cache grew

Legacy tiles had width 2h, but their centers were only 0.45h apart. A fully
populated grid therefore stored approximately (2/0.45)^2 = 19.75 copies of the
same ground at each detail level. Packing those existing tiles alone does not
remove that overlap.

## Initial core-storage conversion

Each address owns the square between the midpoints to its neighbors, width
0.45h. A two-pixel halo supplies shared interpolation/contouring samples. Only
geometry clipped to the owned square is committed; crossing edges retain their
shared endpoints. Lines along north/east boundaries belong to their neighbor.

In the initial conversion, the elevation interval was unchanged. Source resolution was rounded to a whole
number of pixels per core, keeping approximately the prior sample spacing.
A former 2400-square source becomes 540 core pixels plus four halo pixels, or
544-square: 94.86% fewer source pixels per address. TIFF/GeoPackage staging is
still deleted after import.

The viewport selects enough cores for the actual oblique view plus a spare
hosted ring. More small requests can be necessary than before; this is not a
claim of a 20x end-to-end latency improvement. Network latency, worker limits,
GPU uploads and old cache contents remain relevant. Loading indicators show
core footprints. Earth local readers now preserve all paths within each core.

## Compatibility and existing storage

Tile addresses, database schema and archive encoding remain compatible. Existing
legacy rows and archives are read unchanged, including their outer geometry.
New builds coexist with them. New snapshots preserve already-clipped core data.

Existing files do not shrink automatically. Deleting their outer geometry in
place could remove the only cached coverage at a region edge. Safe reclamation
needs a separate migration that preserves those edges, verifies its output and
then reclaims old storage. No source cache or archive was rewritten here.
Moon/Mars remain on their prior storage layout.

## Runtime seam preservation

Earth local reads previously kept only the longest 120 SRTM contour paths per
tile (10,000 at hosted deep tiers). This discarded short edge fragments before
ownership clipping, revealing a grid in high-relief areas such as the Himalayas.
The reader now retains every contour in the owned core at every Earth tier.
Legacy outer geometry is clipped row-by-row before simplification, so removing
the path cap does not retain the old overlapping footprints in memory. Boundary
intersections survive simplification. SQLite and packed reads share this policy.
The bounded readers, source LOD, viewport residency and GPU upload limits remain
in effect. Existing cached data is reused; no terrain rebuild is required.

A read-only regression on four adjacent Himalayan bucket-one tiles measured
the following owned-core geometry before and after removing the path cap:

| Tile (y/x) | Core paths, before → after | East-edge endpoints, before → after |
|---|---:|---:|
| 28/88 | 630 → 5,088 | 226 → 760 |
| 28/89 | 560 → 4,576 | 409 → 845 |
| 29/88 | 195 → 3,418 | 105 → 536 |
| 29/89 | 151 → 2,765 | 64 → 351 |

The old 120 full-footprint paths can split into more than 120 pieces when
clipped. Before the vertex-fidelity fix below, these four cores' segment geometry
grew from 1.79 to 4.51 MiB, while retained paths grew from 1,536 to 15,847. These are geometry-reader
measurements, not a frame-rate claim. The synthetic SQLite/packed regression
also verifies matching endpoints for all 24 shared-edge test contours.
Reproduce with the ignored `cached_himalayan_cores_recover_boundary_detail`
test and `ONEKEE_CONTOUR_BENCH_DB` pointing to the existing cache.

## Contour vertex fidelity

Earth LODs previously discarded every second through fifth vertex after contour
generation. That produced long chords across bends, collapsed small closed
contours, and made neighboring elevation lines intersect. Restoring missing
paths exposed more of this distortion; even a single globe LOD showed it.

Earth specs now use vertex stride one in both globe and local reads. Raster
spacing and elevation interval still select source detail; ownership, fallback,
bounded readers and byte-budgeted residency still apply. Persisted data is
unchanged and does not need rebuilding. Coarse source rasters remain polygonal
when magnified; this preserves their geometry rather than adding smoothing.

Read-only measurements through the globe reader, clipped to the same core:

| Sample (z/y/x) | Segments before | Segments after |
|---|---:|---:|
| Yemen (0/9/27) | 5,431 | 27,472 |
| Himalayas (1/28/88) | 52,888 | 211,867 |

An independent geometric check of the Yemen sample counted 9,081 proper segment
crossings between different elevations before the fix and zero afterward.
Geometry/instance storage grows roughly four to five times for these samples;
these checks do not measure live frame rate. The regression covers bends, small
closed loops, reversed paths, all eleven Earth tiers, and SQLite/packed parity.
Reproduce the source-fidelity checks with the ignored
`cached_earth_lods_preserve_source_vertices` test and `ONEKEE_CONTOUR_BENCH_DB`.
Set `ONEKEE_CONTOUR_FIDELITY_OUTPUT` to export both views' comparison geometry.

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
