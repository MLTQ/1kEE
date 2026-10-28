# admin_globe.rs

Prepares immutable Globe boundary geometry on a single background worker and
draws fill before borders on Earth. Uses the exact same loaded regions, level
selection, and even-odd tessellation as Local. Invalid/open regions stay outlines.

Cache identity pins the source Arc and includes the palette. Camera movement and
opacity only update uniforms. Fine division borders precede country borders.
Longitude/latitude longest-edge bisection (at most two degrees per edge) conforms
large fill triangles to the sphere without changing their planar coverage or
filling holes. Unwrapped dateline coordinates are retained throughout subdivision.
Long outline segments are likewise subdivided before unit-sphere conversion.
