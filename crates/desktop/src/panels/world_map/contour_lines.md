# contour_lines.wgsl

GPU line projection and antialiasing for globe contour and public pipeline
instances. Uniforms must match Rust ContourUniforms exactly (80 bytes).

`horizon_z` replaces a padding float without changing layout. Existing contour
layers retain zero; surface pipelines use 1/camera_distance to cull the part of
the front hemisphere occluded by a perspective unit sphere. Segments with hidden
endpoints are culled rather than connected across invisible geometry.

All callers use four-vertex triangle strips (A−, B−, A+, B+); geometry and
coverage are identical to the previous six-vertex triangle list.
