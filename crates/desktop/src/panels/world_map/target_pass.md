# Target GPU pass

Renders original point positions as adjustable circular sprites, the DEM as an
indexed triangle surface, and draped context as independent lines. Flight uses
an orthonormal camera basis with a 60 degree perspective projection and reversed
infinite Z, retaining depth precision at kilometre distances. A dedicated
Depth32Float attachment resolves occlusion before the image is composited into
egui. All geometry clips to the selected radial boundary.

The offscreen target follows physical viewport size, bounded by the device and
4096 pixels per side. Immutable point batches upload in chunks with an eight
MiB per-frame budget. Uploading a new batch never removes another resident
batch. Current-session batch identities bound residency; the release callback
clears vertices, indices and textures after Target exit.

The composite uses vertex-ID arithmetic for its fullscreen triangle. Dynamic
indexing of a local three-element vertex array triggered a compiler SIGSEGV
inside NVIDIA 615.78.08 during testing; the arithmetic version passed on the
same device. Compositing handles both sRGB and non-sRGB output formats, and its
GPU test compares displayed pixels with the source render. Target textures
scale uniformly on large displays, preserving camera aspect ratio.

Vertex layout is float32 east/north/source-Z, four RGB bytes, and a packed u32
for classification, intensity, return number and RGB availability. Coordinates
are made local in float64 before upload. Uniforms contain eight aligned vec4s
and must match target_points.wgsl exactly. Class filtering and coloring alter
presentation without changing measured coordinates.
