# Target inspection

In **Local → Earth**, click **TARGET**. Move the pointer to position the red
selection cylinder. **km radius** adjusts its WGS84 ground radius from **0.1 to
100 km**, starting at 1 km. The mouse wheel can zoom the Local map while placing
the circle. Left click opens an independent 3D cut-out; **Escape** returns to the
previous Local camera, including its position, zoom and viewing angles. Escape
also cancels the selection tool before entry.

Inside Target, change **km radius** and click **Apply radius** to reload the
cut-out around the same center. The chosen survey, point budget and display
options are retained. Framing and initial flight speed scale to the new size.
The last chosen radius is retained when returning to Local during this session.
Teal published survey outlines also appear while selecting, even if the normal
LiDAR raster layer is off.

Click the Target canvas or **Fly** to capture the mouse. Move the mouse to look,
use **WASD** to fly, **Space / C** to move vertically, and **Shift** to boost.
The mouse wheel changes speed. **Right click** releases the pointer for the
controls; **Home** resets the Target camera. Flight can pass through surfaces
and go outside the fixed cut-out. It never requests a new geographic region as
the camera moves. The boundary stays at the selected radius from its center.

## Data and accuracy

- **Point cloud** streams original returns from intersecting public USGS EPT
  surveys, prioritizing center-covering candidates and their newest acquisitions. The survey selector permits
  inspection of another indexed acquisition. Dates and publisher links stay
  visible. Acquisitions are inspected individually, preserving their identity.
- **DEM terrain** loads numeric bare-earth heights from the existing state,
  national and international elevation adapters. Its numeric requested grid is
  retained for ground placement; a coarser, explicitly
  labelled display mesh keeps flight responsive. Missing heights stay holes.
- **Map context** includes available cached roads, water and prepared vector
  cells, visible imported vector layers, and nearby event/camera markers.
  Overlays are draped on valid DEM heights. Building footprints remain
  footprints; unprovided building heights are not invented. Context is a
  snapshot of available data at entry, not a new bulk acquisition.
  For radii above 5 km, detailed cached roads/cells are limited to the central
  5 km radius. Imported context is still clipped to the whole Target.

Large targets use bounded regional overviews rather than uniform one-metre
coverage. For example, the 100 km NC check requested a 2048 × 2048 height grid
at approximately 98 m sampling, with a roughly 392 m display mesh. Both spacings
are reported. Wide numeric exports currently use USGS, NCEM and supported
England/Netherlands/France services; other fine-source adapters retain their
small-window limits to prevent a large COG/tile download fanout. Point-cloud
detail remains controlled by its separate point and transfer budgets. A
100 km radius covers 10,000 times the area of a one-kilometre radius.

Source XYZ is decoded as float64. WGS84 selection and local metre offsets are
calculated before conversion to GPU float32; the local-coordinate regression
requires less than 0.1 mm of display-coordinate conversion error on its fixture.
This checks software precision, not survey measurement accuracy. Source
classification, intensity, return number and available RGB are retained in the
display attributes. Noise classes 7/18 and withheld returns are excluded. Point
size changes only the display footprint, not measured positions.
Horizontal coordinates use a WGS84 geodesic distance chart around the center;
original source XYZ remains authoritative.

Some EPT feeds declare no vertical coordinate system or Z units. Target keeps
original numeric Z and displays that limitation. It permits combined DEM/point
display only when compatible NAVD88 metre references are explicitly known;
otherwise the two are inspected separately. No vertical offsets, datum
conversion or automatic alignment are inferred from similar-looking heights.
DEM-draped context follows the same rule. There is no synthetic filling of
point-cloud gaps or conversion of grid cells into fake laser returns.

## Loading and resource limits

Nodes load progressively, keeping already displayed points visible. The
default budget is eight million points; the control supports one to sixteen
million and **Refine / reload** requests the chosen budget. A point, transfer,
time or hierarchy limit is reported as partial detail, never as a complete
cloud. Failed refinement preserves the prior display until a replacement batch
arrives. Full source density can exceed these budgets in a one-kilometre circle.

Existing EPT blocks are reused through the bounded LiDAR download cache. This
does not delete raw scans or turn raster receipts into point-cloud archives.
Published cloud coverage and locally indexed catalogs determine availability;
the viewer does not yet decode every provider's local LAS/COPC holding.
Escape cancels jobs, releases the cursor, and frees Target GPU buffers on the
next Local frame. Loading animation preferences do not affect flight controls.

The native desktop is rebuilt from `one-thousand-electric-eye-desktop`. Opt-in
live checks use `ONEKEE_TARGET_TEST_CATALOG` and disposable output under `/tmp`;
they do not replace the user's terrain or fused raster archives.

See the [Kinston benchmark and real-data previews](benchmarks/target-nc-2026-11-16.md).
The [100 km validation](benchmarks/target-radius-2026-11-16.md) records the
regional overview and Local catalog-outline checks.
