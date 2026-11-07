# SRTM GPU worker

`gpu_terrain::build` returns a complete in-memory tile; `TerrainTile::write`
uses the existing transactional writer and core clipping. The desktop enables
this through `ONEKEE_TERRAIN_GPU=1` and falls back to GDAL on compute/source
errors. `contours-bbox --engine gpu` explicitly requires the GPU and reports
failures instead of silently measuring a CPU fallback.

The backend accepts north-up, WGS84, PixelIsPoint, signed Int16, 3601×3601 GL1
GeoTIFFs with the canonical integer-degree origins and -32768 nodata. Other
sources remain GDAL's job. TIFF decoding stays on the CPU. A process-wide
worker selects `ONEKEE_GPU` (or prefers a discrete primary-backend adapter),
serializes device submissions, and retains at most twelve source-window buffers,
invalidated on file size/mtime changes. A resident window is reused only when it
contains every requested bilinear neighbor. Different windows from one source
may coexist within the same bounded LRU. Source files are never opened for writing.

Only TIFF tiles/strips intersecting the requested sample window are decoded,
including the bilinear neighbor halo and partial chunks at file edges. Windows
are aligned to TIFF chunk boundaries; strip-based files necessarily read the
full width of each needed strip. Only the decoded window is uploaded. Normal OS
readahead remains enabled: forcing random-access scheduling reduced block-read
counts but did not improve elapsed time on the tested USB 2.0 connection.

The host computes local sampling coordinates in f64, avoiding global longitude
precision loss in the shader. Resampling and contour extraction share the GPU
raster without a raster readback or intermediate TIFF/GeoPackage. A compute
pass initializes the output raster to nodata, avoiding a CPU-filled raster
upload of up to 144 MB. Cells scan
only intersecting elevation levels, compact segments into a bounded buffer,
and reject overflow rather than committing partial geometry. Native source
posts, shared-file edges, zero-level contours, and the separate coastline layer
are preserved. A zero-weight nodata neighbor does not invalidate a native post.

Contour ties, saddle pairing, and half-cell treatment of nodata follow GDAL's
marching-square conventions. See upstream [square.h](https://github.com/OSGeo/gdal/blob/v3.13.3/alg/marching_squares/square.h)
and [utility.h](https://github.com/OSGeo/gdal/blob/v3.13.3/alg/marching_squares/utility.h).
This is independently implemented compute code, not a general GDAL replacement.
Coordinates become f32 for the existing native stitching/writer interface;
GDAL's f64 output need not have identical vertex/path counts or byte order.

The worker releases its lock before CPU sorting/stitching. Source buffers use
up to about 594 MiB when all twelve windows span full source files; raster/segments/readback add roughly 290 MiB at the
limits of 6000² samples and four million segments. These are separate from the
renderer residency budgets. Allocation failures disable the worker for the
process; unsupported grids, bad inputs, and segment overflow reject that job.

Tests include all marching-square cases, signed levels, all/partial nodata,
exact-height ties against GDAL, shared-file boundary sampling, full real native
raster comparisons against GDAL, capacity failure, and invalid-grid rejection.
Hardware tests are ignored by default and require an explicit GPU invocation.
The desktop's `gpu_srtm_end_to_end_benchmark` also checks cache manifests,
finite/clipped geometry, and total contour length against GDAL in temporary
caches, alternating execution order and reporting cold/warm worker timings.


Cold-build diagnostics emit one `[terrain-gpu-source]` line per cache miss:
compressed bytes read by the counted file reader, chunk count, decoded pixels,
time inside reads/seeks, remaining decode/allocation time, and buffer-upload
submission time. These are host timings; completed GPU work is synchronized in
compute/readback. Per-build `worker_wait_ms` includes first-use initialization
and lock contention, and `total_ms` now includes that wait. TIFF read counts
include buffered readahead/metadata and are not physical-device I/O counters.

`window.rs` unit tests verify row-stripped TIFFs at strip/file boundaries,
negative samples, nodata, truncated files and bounded reads. Existing GPU tests
verify tiled real files and full raster parity against GDAL. The reproducible
Linux `scripts/benchmark-cold-terrain.py` requests source-file cache eviction,
uses fresh worker processes and fresh temporary caches, and reports process plus
descendant block-read counts. It copies schema only from a read-only native
cache; source geometry and source-file contents are never modified.
