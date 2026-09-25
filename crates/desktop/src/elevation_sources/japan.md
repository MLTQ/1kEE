# japan.rs

## Purpose
Decode Japan GSI's numeric PNG elevation pyramids into a small WGS84 height
grid, using 1 m DEM1A first and 5 m DEM5A/B/C and 10 m DEM10B to fill gaps.

## Contracts
The official signed 24-bit value represents centimetres; RGB(128,0,0) is nodata,
not a large height. Web Mercator coordinates are sampled bilinearly across tile
edges without blending nodata. Pixel-centre heights are written north to south.
Each layer chooses a pyramid zoom appropriate to output spacing, capped at 64
source tiles; large point-sampling chunks may therefore use a coarser pyramid.
Only explicit 404/nodata permits a lower source; transient HTTP failures remain
retryable and cannot silently lock a tile to a coarser source.

At most 128 decoded/missing PNG tiles are retained for ten minutes (32 MiB of
height arrays). PNG dimensions are checked before decompression. Transfers are
limited to 2 MiB per image, and progress reports actual received bytes. These
sources are not national 1 m coverage; the UI credits advertise 1–10 m.

Official specifications: https://maps.gsi.go.jp/development/demtile.html and
https://maps.gsi.go.jp/development/ichiran.html#dem

Every source-tile lookup observes the enclosing acquisition deadline, including
cache hits, so a patchy five-source mosaic has a bounded total acquisition time.
