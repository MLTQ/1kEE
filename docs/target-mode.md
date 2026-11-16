# Target inspection

In **Local → Earth**, click **TARGET**. Move the pointer to position the red
selection cylinder. Its ground footprint has a **1,000 metre radius** measured
on WGS84. Left click opens an independent 3D cut-out; **Escape** returns to the
previous Local camera, including its position, zoom and viewing angles. Escape
also cancels the selection tool before entry.

Click the Target canvas or **Fly** to capture the mouse. Move the mouse to look,
use **WASD** to fly, **Space / C** to move vertically, and **Shift** to boost.
The mouse wheel changes speed. **Right click** releases the pointer for the
controls; **Home** resets the Target camera. Flight can pass through surfaces
and go outside the fixed cut-out. It never requests a new geographic region as
the camera moves. The boundary remains one kilometre from the selected center.

## Data and accuracy

- **Point cloud** streams original returns from intersecting public USGS EPT
  surveys, trying the newest candidate first. The survey selector permits
  inspection of another indexed acquisition. Dates and publisher links stay
  visible. Acquisitions are inspected individually, preserving their identity.
- **DEM terrain** loads numeric bare-earth heights from the existing state,
  national and international elevation adapters. Its approximately one-metre
  requested grid is retained for ground placement; a coarser, explicitly
  labelled display mesh keeps flight responsive. Missing heights stay holes.
- **Map context** includes available cached roads, water and prepared vector
  cells, visible imported vector layers, and nearby event/camera markers.
  Overlays are draped on valid DEM heights. Building footprints remain
  footprints; unprovided building heights are not invented. Context is a
  snapshot of available data at entry, not a new bulk acquisition.

Source XYZ is decoded as float64. WGS84 selection and local metre offsets are
calculated before conversion to GPU float32; the local-coordinate regression
requires less than 0.1 mm of display-coordinate conversion error on its fixture.
This checks software precision, not survey measurement accuracy. Source
classification, intensity, return number and available RGB are retained in the
display attributes. Noise classes 7/18 and withheld returns are excluded. Point
size changes only the display footprint, not measured positions.

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
