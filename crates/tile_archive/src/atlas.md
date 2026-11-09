# atlas.rs

Shared Atlas height-cache format: SQLite application ID 0x314b4154, schema 1,
filename atlas_heights_v1.sqlite. Rows are keyed by detail tier and geographic
x/y indices, with a source fingerprint, little-endian Float32 metres and CRC32.
NaN is nodata. Values must otherwise be finite and within -12000..10000 metres.

Each core tile includes one sample of halo on every side, north-first row order.
Grid-aligned bounds and bilinear pixel-centre sampling avoid seams. Longitude
wraps at the date line. The writer rejects unrelated databases, holds an OS file
lock, and publishes whole tiles with synchronous SQLite transactions in WAL
mode. Invalid saved payloads are returned as errors for the builder to repair.

The read-only reader validates version, dimensions, value range and checksum.
It returns missing coverage separately from corruption. Tile-major sampling
avoids scanline cache thrashing in wide views; the LRU retains at most 64 tiles.
Output is limited to 8192 pixels per edge and 32 million pixels total. Readers
never repair or overwrite data. Atlas can fall back to original sources.
