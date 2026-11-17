# Target session

Owns the selected WGS84 Region, independent flight camera, immutable measured
point batches, numeric DEM, draped map context and cancellation markers.
AppModel keeps Local's GlobeViewState untouched while Target is active.

Radius ranges from 100 m to 100 km and persists across Target exit/re-entry in
the current app session. Selection preview updates immediately; changing an
open cut-out uses explicit Apply radius, retaining survey, budget and display
options while resetting the independent camera to fit the new extent. Large
extents use bounded overview terrain and central-window cached vector detail.
All surveys (merge) is the default; explicit single-source inspection remains
available and the choice survives resizing. A coverage pass shares the point
budget before source-record cursors resume unfinished surveys for detail.

The point worker uses a bounded channel and a shared EPT permit. It converts
float64 geographic offsets to 20-byte GPU vertices off the UI thread. Survey
headers publish with their first actual batch. A failed or empty refresh keeps
the old source and geometry; the first replacement batch atomically changes
both. Later contributing surveys append without clearing earlier batches.
Each batch shares an immutable source header; the session keeps all contributing
headers and the final per-source receipt. Partial successful loads remain visible
on later failures. Transfer, decoded-record, node and time limits are shared
across all candidates and passes, rather than restarting per survey.

Discovery ranks actual survey polygons before dates. Original USGS EPT and
supported OpenTopography single-file bulk LAS/LAZ are decoded in Rust. Bulk
coordinates use a declared horizontal CRS and bounded XY-only GDAL batches;
source Z is untouched. Unverified original heights may be displayed together,
with an explicit label; known conflicting vertical references remain separate.
Unsupported collections and finished empty workers are
shown explicitly. The selector names a discovered survey while reading it.
Retained, verified metre-height ground rasters are tried before remote DEM
services; they remain separate from points with unverified matching Z.

DEM and point workers are independent. Missing or undeclared vertical reference
prevents combined DEM/point display and prevents DEM-draped context being misrepresented
as source-point geometry. Native point Z is never adjusted to fit terrain.
Source heights are not guaranteed metres when metadata omits that information.

Escape consumption ends the egui input borrow before cursor commands are sent.
Dropping the session removes cancellation markers and channel receivers, so
blocked sends unwind and stale workers cannot publish into another Target.
Native source blocks remain in the existing bounded cache. The GPU release
callback runs on the next Local/Globe map frame.

While Target is active the ordinary LiDAR raster layer cancels its preparation
jobs and yields the shared EPT permits, retaining completed Local textures.
Returning to Local resumes that layer without discarding its ready data.
