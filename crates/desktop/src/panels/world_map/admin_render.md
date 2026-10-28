# admin_render.rs

## Purpose
Render selected administrative outlines and optional translucent region fill in
Local Earth view using the same ground-plane projection as terrain overlays.

## Contracts
- Region bounds cull before projection; whole-region longitude shifts handle
  dateline views without splitting a fill across the world.
- Project original consecutive endpoints, then clip segments. Never filter out
  vertices and connect unrelated survivors. Segments crossing the viewport remain.
- Clip triangles to the canvas before submission, including polygons enclosing
  the entire viewport with all original vertices offscreen.
- Fill precedes every border; countries draw last. Fill colors vary deterministically
  by relation ID. Opacity/level changes affect the live view without retessellation.
- Fill requires a loader-prepared mesh; incomplete/legacy boundaries stay outlines.

Source selection and automatic refresh are shared with Globe via
`admin_layer::get_for`; Local retains its ground-plane projection and clipping.
