# Original radial EPT streams

`sources` queries indexed intersecting USGS survey candidates in descending
catalog date order. `stream` emits a source Header and original-return Batches;
it never bins, averages, interpolates or manufactures point locations. XYZ,
classification, intensity, return number and RGB come from decoded LAS points.
The radial selection uses Region's float64 WGS84 geodesics.

EPT hierarchy traversal covers all intersecting XY nodes and all return heights
from coarse through available leaf detail. It excludes noise classes 7/18 and
withheld returns. Geographic and Web Mercator sources are supported; other
horizontal references fail explicitly. Source vertical metadata is preserved
without inferred conversion. Date-line windows test both wrapped source charts.

The existing trusted publisher URLs, block cache, size limits and shared worker
permits are reused, including the ten-GB free-space reserve. Point, node, decoded-point, traversal-time and depth ceilings produce
an explicitly incomplete Summary. Source and transfer-limit failures return an error so the UI
can retain already published batches. Every worker checks its cancellation
marker between network requests and decode batches. These receipts never
authorize reclaiming original scans.
