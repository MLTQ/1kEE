# Native LiDAR pipeline

`lidar-pipeline` owns numeric scan preparation, v1 receipt verification and
latest-acquisition rendering. Production processing does not invoke Python.

The default library contains SQLite lookup, bounded windowed GeoTIFF decoding,
pixel fusion, shading and atomic PNG publication. The desktop calls it directly
on its existing background worker. It uses the Rust TIFF decoder and does not
link GDAL or PROJ. The sidebar shows catalog boundary lines on the globe and prepared data in
Local. The desktop enables the optional `ept` feature to prepare view-resolution
USGS 3DEP overviews on its background workers when local data is missing. This
also runs entirely in Rust without GDAL/PROJ. Other providers currently need
prepared local scans. Catalog harvesting and bulk downloads remain separate tools.

EPT overviews live in `fused/v1/previews` with a separate `previews.sqlite`,
never in the full-density receipt index. Each numeric TIFF includes acquisition,
source CRS, fetched-node hashes and sampled counts. The viewer orders acquisitions
by date, prefers full-density data within an acquisition, then finer overviews.
Only measured cells are visible. Source Z remains unchanged. Source block cache
size is capped at 1 GiB after each preparation, with at most two preparations per
process, 256 MiB transferred per tile, 32 MiB per response, 64 million decoded
points per survey, a three-minute preparation deadline and a 10 GB free-space
reserve. Moving the camera or disabling the layer cancels work between requests
and decode batches. A blocking request can take up to its 30-second timeout to
finish. Overviews never authorize deleting full-resolution source data.

An isolated preparation/render can be run with both features:

```sh
cargo build --release -p lidar-pipeline --features builder,ept
target/release/one-thousand-electric-eye-lidar prepare-view \
  --catalog /path/to/Data/Lidar/catalog.sqlite \
  --bounds=-88.2421875,36.9140625,-88.06640625,37.08984375 \
  --zoom 10 --output /tmp/kentucky-lidar.png
```

The optional `builder` feature adds LAS/LAZ decoding through `las`/`laz` and
GDAL/PROJ through checked, owned C API wrappers. It builds a standalone processor:

```sh
cargo build --release -p lidar-pipeline --features builder
target/release/one-thousand-electric-eye-lidar --root /path/to/Data/Lidar --plan
target/release/one-thousand-electric-eye-lidar --root /path/to/Data/Lidar
target/release/one-thousand-electric-eye-lidar --root /path/to/Data/Lidar --verify
```

Building the converter requires GDAL development headers, pkg-config and libclang
for `gdal-sys` binding generation. Running it needs the matching GDAL/PROJ native
libraries. GDAL's stable `GDALRasterIO` API avoids version-specific RasterIO
extra-argument structures. CRS operations always use longitude/easting first;
vertical CRS is demoted only for XY transformation, while original Z values and
the full source CRS remain preserved. No cross-datum height normalization occurs.

The v1 four-band GeoTIFF and SQLite schema remain compatible with the initial
prototype. Existing unchanged receipts are verified and reused. `convert --input
scan.copc.laz --output scan.tif` and `render --catalog catalog.sqlite --bounds
west,south,east,north --preferred survey --output map.png` support isolated
benchmarks. The production batch pins its working directory to the data volume,
checks an optional `--volume-uuid`, holds the same single-writer file lock,
observes the reserve, verifies reopened raster cells and commits a receipt only
after atomic publication. It never removes raw scans.

Run `cargo test --release -p lidar-pipeline --features builder` for independent
statistics, filtering, LAS/LAZ, rollback, receipt reuse, corruption/date repair,
native rendering, TIFF layout and CLI tests. The desktop has an additional test
that loads a prepared view through its real background-load entry point.

Python is retained under `scripts/benchmarks/lidar_fusion_reference.py` only as a
comparison oracle for this pipeline. `scripts/benchmark_native_lidar.py` compares
complete conversions and native reads against that reference on real scans.
