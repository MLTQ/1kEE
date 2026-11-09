# tree_geometry.rs

## Purpose
Prepare arbitrary concave forest footprints before projecting their measured
vertex elevations. Filled egui paths support convex shapes only; their closed
antialias miters can expand almost reversing edges far across the screen.

## Components
- `tessellate`: Lyon even-odd triangles in local metre coordinates, with baked
  height as an interpolated vertex attribute. Only complete, finite closed rings
  receive fill; invalid/open input stays unfilled.
- `unwrap`: preserve short edges at the date line; frame projection shifts a
  whole feature near focus.
- `project_fill`: keep triangle indices intact, cull offscreen triangles and let
  GPU clipping handle remaining edges. Invalid vertices never reconnect.
- `outlines`: bounded open-path strokes, retaining a source ring's repeated
  endpoint, with breaks at missing/invalid points. No forced polygon closures.

## Contracts
Tessellation runs on the tree loader worker; changing camera or Layer Spread
only reprojects the retained topology. Terrain heights remain source-derived.
No filled `PathShape` or closed-path miter goes into the tree paint stream.
