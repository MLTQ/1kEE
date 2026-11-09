# atlas_surface_tests.rs

Parse/validate both actual GPU shaders. Compare surface uniforms and vertex
projection arithmetic with the production contour projector for negative and
positive terrain heights, three camera pitches and layer spreads 1, 12 and 100.
This pins the common height/exaggeration contract rather than matching a picture.

Shader validation composes the exact production lighting shader and checks its 256-byte uniform layout.
