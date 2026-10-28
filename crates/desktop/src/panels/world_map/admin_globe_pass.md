# admin_globe_pass.rs

GPU fill callback for administrative regions on the unit sphere. Triangle
instances contain three sphere vertices and a linear color; the 64-byte uniform
contains the same yaw/pitch and perspective inputs as the globe backdrop.

Resources initialize lazily using the existing contour renderer's target format.
Uploads stage under a 4 MiB / 2 ms budget with bounded individual buffers. A changed
source/selection hides stale fills until its exact version is ready. Opacity is a
uniform, and fills use premultiplied blending before the separate outline pass.

The budget is shared with globe lines, not an additional per-pass allowance.
Canvas lifecycle hooks release fill buffers when shading is hidden or Local is
active. Colors use egui's linear-light premultiplication and target-format-aware
conversion, matching Local opacity rather than applying an extra gamma fade.
