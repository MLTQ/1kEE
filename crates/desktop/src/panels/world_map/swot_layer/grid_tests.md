# grid_tests.rs

Pins the binary-grid contracts: north-up row order, signed little-endian
Eötvös scaling, dateline wrapping, transparent polar/nodata samples, truncated
file rejection, fixed palette saturation and stable padded request bounds.
Small in-memory rows exercise the real seek/read path without external data.
An alternating 2048-pixel signal proves native globe reads preserve fine detail
that the former 1024-pixel path aliases away. An ignored installed-data smoke
test verifies a real native window and optionally exports its rendered colors.
