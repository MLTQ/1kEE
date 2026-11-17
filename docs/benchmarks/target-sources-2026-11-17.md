# Israel and Manhattan Target source validation

The Israel survey was published and retained locally but unavailable to Target:
its original points are a public OpenTopography bulk LAZ, outside the former
USGS-only point reader, and its downloaded numeric ground DEM was not considered
by the Target DEM loader. The Manhattan screenshot selected a New Jersey
survey whose bounding box overlapped Manhattan while its actual polygon covered
only the western part of the selected circle.

Validation on 2026-11-17 used the production native readers, Target projection
and GPU renderer on NVIDIA RTX 4090 / Vulkan / driver 615.78.08.

| Check | Result |
| --- | --- |
| Israel center and radius | 34.84° E, 32.30° N; 1 km |
| Original Israel source | `opentopoID:OTLAS.102016.2039.1`, 2006 |
| Source file | 24,960,180-byte public LAZ; all 13,268,620 records inspected |
| Retained circle returns | 377,003; complete after noise/withheld and radial filtering |
| Initial original-file scan | 65.32 s, including cold cache transfer; bounded background work |
| Original coordinates | EPSG:2039 XY transformed to WGS84; source Z preserved |
| Retained ground DEM | Actual native cells approximately 2.03 m |
| Requested DEM / display mesh | Approximately 1 m samples / 3.9 m mesh spacing |
| DEM geometry | 232,554 valid triangles; no invented fill outside data |
| Manhattan center and radius | 74.012680° W, 40.709930° N; 1 km |
| Automatically selected survey | `USGS_3DEP_ID:NY_NewYorkCity`, catalog date unspecified |
| NYC original-point check | 500,000 returns, 88 occupied 200 m cells, central 100 m covered |
| NYC east/west coverage | Verified original returns on both sides of the circle |

The NYC integration check deliberately limits its point budget to 500,000 and
reports incomplete source detail at that limit. It verifies correct source
selection and geographic coverage, not a full-density or timing benchmark.
The normal Target controls still support 1–16 million source points.

The Israel GPU check waits for all progressive vertex-buffer uploads before
saving previews, then renders the point cloud and retained DEM separately.
Neither an unknown vertical datum nor a match between their heights is inferred.
A separate raw-LAZ check found approximately −0.46 to 105.87 m numeric Z in the
same radial neighborhood, without modifying those values.

The regression fixture contains the actual public NYC, New Jersey and adjacent
New York survey polygons. Tests verify polygon holes, center/radial ranking,
original LAS coordinate/attribute preservation, noise/withheld exclusion,
point-budget accounting, bounded/validated public listings, coordinate-process
precision/cancellation, local-file containment and verified Z-unit handling.
Native tests and strict Clippy pass. Desktop Target, LiDAR and elevation tests
pass; desktop Clippy completes with existing warnings and the previously tracked
approximate-constant suppression.

Publisher metadata: [Israel Mediterranean Sharon sea cliff, 2006](https://portal.opentopography.org/datasetMetadata?otCollectionID=OT.102016.2039.1).
The publisher describes the coastal footprint, acquisition, horizontal reference
and metre units. The retained raster itself reports approximately 2.026 m cells;
the publisher's nominal one-metre product description is not used to overstate
this downloaded asset's detail.

Machine-readable results: [validation receipt](target-sources-2026-11-17.json).
