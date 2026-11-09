Test-only egui/wgpu target used to render and optionally read back an RGBA frame.
The test widths give a 256-byte-aligned row. Each call runs callback preparation,
painting and GPU completion before comparing pixels, including residency cleanup.

`install_globe` optionally registers the production globe backdrop pipeline for
the Atlas shader/relief preview, without changing the boundary tests.
