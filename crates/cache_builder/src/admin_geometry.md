# admin_geometry.rs

## Purpose
Stitch relation member ways using exact node IDs, independent of node storage.
Build complete rings or honest open fragments without bridging missing nodes.

## Components
- `AdminRegion` holds relation metadata, all outer/inner rings and completeness.
- `assemble` resolves stitched chains and breaks them at absent coordinates.
- `stitch` joins deterministic member order, reverses ways and extends both ends.

## Contracts
- Lookup returns one optional coordinate per reference, preserving order.
- `complete` requires closed chains and all nodes. Caller also checks all member
  ways were found before the binary writer permits polygon fill.
- Inner rings are retained; the renderer fills each relation with even-odd winding.

`CoordinateResolver` names the batched optional-coordinate contract shared by both backends.
