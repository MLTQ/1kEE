# grid.rs

## Purpose
Read prepared SWOT_06 vertical gravity gradients with bounded runtime memory.
The globe uses a 4096×2048 overview; Local samples at most 1024×1024 pixels
from the native 21600×10800 one-arcminute grid.

## Components
- `Region::around` pads and snaps local requests; `bounds` returns unwrapped
  west/south/east/north edges for geographically correct texture projection.
- `read_image` validates exact file length before allocation. `read_region`
  seeks only the requested rows, wraps longitude, and leaves polar gaps clear.
- `color` applies a fixed diverging ±20 Eötvös ramp with transparent nodata.

## Contracts
The importer writes north-up global pixel grids, west edge −180°, north edge
90°, little-endian Int16, scale 0.1 Eötvös and nodata −32768. This format is
visual gravity data, never elevation. All reads and pixel conversion run on
the layer worker. The native reader holds one 43.2 KB source row at a time.
