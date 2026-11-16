# Target session

Owns the selected WGS84 Region, independent flight camera, immutable measured
point batches, numeric DEM, draped map context and cancellation markers.
AppModel keeps Local's GlobeViewState untouched while Target is active.

The point worker uses a bounded channel and a shared EPT permit. It converts
float64 geographic offsets to 20-byte GPU vertices off the UI thread. Survey
headers publish with their first actual batch. A failed or empty refresh keeps
the old source and geometry; the first replacement batch atomically changes
both. Partial successful loads remain visible on later failures.

DEM and point workers are independent. Missing or undeclared vertical reference
prevents combined display and prevents DEM-draped context being misrepresented
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
