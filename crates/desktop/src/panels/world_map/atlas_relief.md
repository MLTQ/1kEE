# atlas_relief.rs

## Purpose
Own the optional Atlas base-map image and its cached Globe/Local projections.
Source discovery, decoding, shaded relief and texture preparation happen on one
generation-safe `layer_snapshot` worker. Drawing never reads the terrain drive.

## Components
- `load_grid` prefers the existing global signed-metre GEBCO preview TIFF, then
  the scaled UInt16 PNG (-11,000..9,000 m), then the 1440×720 little-endian BIL.
  All are existing derived products; selecting Atlas never downloads/builds data.
- `draw_globe` paints the north-up image on the existing unit sphere.
- `draw_local` paints the same image on the local sea-level plane. Longitude
  quarters wrap independently to preserve coverage at the dateline.
- `paint_cached` shares immutable Arc meshes and rebuilds projections only when camera/layout/texture
  inputs change. `clear` rejects old worker results and releases both caches.

## Contracts
Only Earth + Atlas invokes this layer. It draws below optional scientific,
contour and operational layers; marker geometry and selection remain unchanged.
Source snapshots key by asset root, and cache reset clears the image and mesh.
Missing data retries once per second and leaves the paper canvas visible.
Relief is limited to the prepared overview resolution; local detailed contours
and explicitly enabled hillshade remain separate. No geology is inferred.
