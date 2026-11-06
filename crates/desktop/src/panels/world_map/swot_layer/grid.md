# grid.rs

## Purpose
Read prepared SWOT_06 vertical gravity gradients with bounded runtime memory.
The globe overview remains 4096×2048. Zoomed globe views request up to
4096×4096 pixels from the native one-arcminute grid, subject to the GPU limit;
Local retains its 1024×1024 bound. Small windows retain every source pixel.

## Components
- `Region::around` pads and snaps local requests; `bounds` returns unwrapped
  west/south/east/north edges for geographically correct texture projection.
- `read_image` validates exact file length before allocation. `read_region`
  seeks only the requested rows, wraps longitude, and leaves polar gaps clear.
- `color` applies a fixed diverging ±20 Eötvös ramp with transparent nodata.
- `Region::from_bounds` snaps a geographic viewport outward to native pixels.

## Contracts
The importer writes north-up global pixel grids, west edge −180°, north edge
90°, little-endian Int16, scale 0.1 Eötvös and nodata −32768. This format is
visual gravity data, never elevation. All reads and pixel conversion run on
the layer worker. The native reader seeks only the requested columns, with
at most two reads per row across the dateline. Its maximum row is 43.2 KB.
