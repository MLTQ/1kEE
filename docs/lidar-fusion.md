# Fused LiDAR elevation layer

The Earth sidebar has one **Layers → Base → Lidar** toggle. Globe draws all
cataloged survey outlines. Local reads compact numeric rasters prepared from
downloaded scans and fuses acquisitions in descending catalog-date order. A newer
scan wins only at measured pixels; older measurements can fill gaps. Regions
without measurements stay transparent. Missing USGS 3DEP views now prepare
numeric overviews from public EPT blocks on the background tile workers and
then display the fused rasters. There are no survey search or picker controls.
Other providers currently require prepared local scans.

Preparation, archive auditing and fusion of prepared views run in Rust through
`crates/lidar`. The desktop reads TIFF chunks directly without Python, GDAL or
PROJ. Globe catalog outlines use the existing background boundary worker. The
Python converter prototype is retained only as a benchmark reference under
`scripts/benchmarks/lidar_fusion_reference.py`.

Each prepared scan stores observed minimum, maximum, mean and count in four
compressed, tiled float32 GeoTIFF bands. Means use float64 accumulation over all
returns except noise classes 7/18 and withheld returns. This is an all-return
elevation product, not a classified bare-earth DTM. Minimum/maximum returns are
not guaranteed ground/roof measurements. The global grid matches the app's z15
pixel grid: 1.19 m north/south at the equator and a longitude-dependent east/west
spacing. Close views can magnify this grid; sub-pixel structure, individual
returns, intensity and classifications cannot be recovered from it.

Source rasters remain separately attributable, and the viewer fuses their valid
pixels into one layer. No cross-survey averaging or vertical datum conversion is
performed. Source elevations retain their original vertical reference and units,
recorded in full CRS metadata. This does not promise seam-free engineering
elevations across incompatible surveys. Acquisition ranges are ordered by their
catalog strings, beginning with the capture start; per-point capture dates are
not available. Undated surveys follow dated surveys.

## Workstation results, 2026-11-16

Ryzen 5 5600X, RTX 4090 available, NVIDIA 615.78.08; three real COPC scans totaling 23.2 million points. Final medians after compilation finished. Sizes are decimal MB.

| Scan | Raw → numeric raster | Python / GPUPDAL build | Current / fused map read |
|---|---:|---:|---:|
| richmond-2025 | 35.88 → 1.31 MB | 3.34 / 4.41 s | 1239 / 28 ms |
| ozarks-2024 | 30.43 → 0.85 MB | 1.87 / 2.95 s | 545 / 25 ms |
| swiss-2026 | 64.24 → 4.42 MB | 9.30 / 9.40 s | 380 / 45 ms |

Fused reads were 8–45× faster on these windows. The production three-source audit found 130,562,521 raw bytes represented by 6,585,142 derived bytes (19.8× smaller), with every raster checksum and point count passing. This is a sample, not an inventory-wide storage forecast.

GPUPDAL provided no clear build advantage: it was slower on Richmond/Ozarks and effectively tied on Switzerland. Its ordinary raster pipeline also assigned some returns to different grid cells: Richmond had 280 count-cell differences (maximum shared-cell mean difference 0.77 m); Ozarks matched; Switzerland had 43 coverage differences and a 0.015 m 95th-percentile shared-cell mean difference (maximum 2.15 m). No cause is asserted without a separate reprojection/binning investigation. These measurements selected the initial prototype; production processing has since moved to Rust.

Full-density means differ from the old COPC LOD preview: shared-cell 95th-percentile height differences were 6.07 m in Richmond, 2.39 m in the Ozarks and 1.01 m in Switzerland. These are sampling/product differences, not a claim of measured ground accuracy. Fused coverage increased by 9, 55 and 30,654 pixels respectively.

The integrated Richmond and Swiss survey renders passed with raw-cloud reading and publisher access blocked. Synthetic tests independently reduce original points and cover latest-date overlap/gaps, noise/withheld rejection, failed publication, corrupt-raster repair, source receipts, resume and mount identity.

Example outputs:

- richmond-2025: [sampled preview](benchmarks/lidar-fusion-views/richmond-2025-current.png), [full-density fused tile](benchmarks/lidar-fusion-views/richmond-2025-fused.png)
- ozarks-2024: [sampled preview](benchmarks/lidar-fusion-views/ozarks-2024-current.png), [full-density fused tile](benchmarks/lidar-fusion-views/ozarks-2024-fused.png)
- swiss-2026: [sampled preview](benchmarks/lidar-fusion-views/swiss-2026-current.png), [full-density fused tile](benchmarks/lidar-fusion-views/swiss-2026-fused.png)

## Rust migration validation, 2026-11-16

All three fixtures match the Python oracle exactly in all four raster bands, geographic grids, rendered coverage and visible RGB values. Native TIFF files are 17–19% smaller because bands are compressed separately. The converter retains compatibility with the original pixel-interleaved files.

| Scan | Python / Rust build | Python / Rust read and PNG write | Native raster |
|---|---:|---:|---:|
| richmond-2025 | 3.11 / 3.43 s | 64 / 64 ms | 1.09 MB |
| ozarks-2024 | 1.61 / 1.87 s | 64 / 60 ms | 0.71 MB |
| swiss-2026 | 8.26 / 8.55 s | 114 / 136 ms | 3.60 MB |

These read timings include PNG encoding in both paths and executable/native-library startup for Rust. They differ from the initial raster-read-only table above. The desktop calls the Rust reader directly and does not load GDAL/PROJ; its direct-call latency is not measured by this CLI comparison. There is no general Rust speedup claim: native builds were 4–16% slower in this run, in exchange for removing the additional production processing runtime.

The native verifier accepted all 154 previously completed rasters (17,817,243,589 source bytes represented by 1,159,491,790 raster bytes), with no failures. Completed receipts are reused, and raw scans remain intact. Native unit/CLI tests cover independent statistics, noise/withheld flags, LAS/LAZ, failed decode, resume/corruption/date changes, latest acquisitions and gaps, TIFF layouts and edge chunks, viewport cancellation, Unicode provenance and volume identity. The desktop integration test exercises its actual prepared-view load entry point.

The desktop release was rebuilt; its dynamic dependency check shows no GDAL, PROJ or Python libraries. A prepared-source render also succeeded with `PATH=/nonexistent`. Catalog and download tools still use their prior Python path; the sidebar display now uses prepared data exclusively in Local.

[Raw migration measurements](benchmarks/lidar-native-2026-11-16.json).

## Build and resume

Build the native processor with GDAL development headers, pkg-config and
libclang available. The converter uses GDAL/PROJ for projection and GeoTIFF
publication; these libraries are excluded from the desktop reader dependency.
Inspect the existing completed-scan inventory first:

```sh
cargo build --release -p lidar-pipeline --features builder
target/release/one-thousand-electric-eye-lidar \
  --root /run/media/m/Minkowski/Data/Lidar --volume-uuid 6FEF-269A --plan
```

Remove `--plan` to convert, or add `--max-files 10` for a bounded batch. Newest
catalog dates are processed first. Rerunning resumes: an unchanged source and a
matching derived checksum are reused. Changed/corrupt outputs are rebuilt. A
single-writer lock prevents concurrent builds; readers continue using committed
entries. Output lives under **Lidar/fused/v1**. The index retains publisher URL,
survey, acquisition date, original CRS, raw size/mtime/SHA-256, raster checksum,
and decoded/accepted point counts. The converter verifies every stored numeric
cell before publication, atomically replaces the raster, then commits its index
entry. Revisions invalidate affected rendered map tiles in the app.

The converter writes only to the verified data volume and leaves at least 10 GB
plus its bounded raster scratch free. This uses some of the raw downloader's
100 GB headroom; the downloader still keeps its own reserve. Raw scans are never
deleted by this command. Verify published output with:

```sh
target/release/one-thousand-electric-eye-lidar \
  --root /run/media/m/Minkowski/Data/Lidar --volume-uuid 6FEF-269A --verify
```

The current batch supports present, completed, direct COPC scan receipts in
`bulk.sqlite`. Other downloaded DEM/relief rasters still need native preparation
before they can appear through this single fused layer (tracked in `1kee-1048`).
EBA ZIP members, separately selected NASA granules, wrapped bounds and scans
requiring more than 16 million output cells are not yet converted by this batch.
Oversized/invalid scans are recorded in the index's `failures` table and their
raw path remains available. Full-volume/offline errors pause the batch. Retention
changes need a whole-inventory coverage audit, including these exclusions;
success on three test scans does not authorize treating every raw asset as
redundant. Old content-versioned rasters are retained for active readers and
need a later reclamation pass.

The 2026-11-16 rollout uses the native processor as the transient user service
`1kee-lidar-fusion`, covering 5,225 present COPC receipts (311.7 GB of raw
scans), newest first. It runs at low CPU/I/O priority with a 4 GiB memory cap;
its sandbox permits data-drive writes only under `Lidar/fused`. Raw inputs are
read-only. Follow progress with `journalctl --user -fu 1kee-lidar-fusion` and
stop with `systemctl --user stop 1kee-lidar-fusion`. After a stop or reboot,
rerun the build command above to resume. This service converts a snapshot of
the local inventory; it does not download the world's remaining scans.

## Missing Local coverage

The Kentucky report exposed an acquisition gap: catalog coverage was present,
but no downloaded or fused scans overlapped the view. Converting downloaded
Swiss COPCs could never populate that area. Local now prepares bounded USGS
[EPT](https://entwine.io/en/latest/entwine-point-tile.html) overviews as needed,
using native Rust HTTP, LAZ decoding, EPSG:3857/4326 XY conversion, numeric
binning and GeoTIFF publication. The desktop still has no GDAL/PROJ dependency.
The existing full-density batch continues independently.

Overviews are 512 × 512 at each requested geographic tile's resolution. They
use hierarchical samples rather than every return; zooming in prepares finer
samples. Four bands retain observed min/max/mean/count with noise and withheld
points excluded. No gap interpolation or vertical conversion is performed.
Date ordering is shared with full-density rasters, which take precedence within
the same acquisition; finer overviews precede coarser ones. Missing referenced
files no longer hide other available measurements.

These overviews have a separate `fused/v1/previews.sqlite` index and TIFF
metadata identifying the sampling product, source CRS and individual EPT node
hashes. They are excluded from full-density audit/compression receipts and never
authorize raw-scan reclamation. Preparation is limited to two concurrent jobs,
256 MiB of downloads per tile, 32 MiB per response, 64 million decoded points
per survey and three minutes; it preserves 10 GB of free space. Source blocks
are cached up to 1 GiB after preparation. Camera movement/layer changes cancel
requests between network/decode operations. A network request has a 30-second
timeout. Prepared numeric and rendered tiles remain usable offline.

The reported Kentucky region was prepared from the actual public surveys and
rendered through the native desktop loader. Unit fixtures cover EPT hierarchy
continuations, both projections, filtering, cancellation, incomplete nodes,
numeric TIFF round trips, overview-only archives and acquisition/density order.
An independent laspy/pyproj/NumPy/rasterio reduction of 4,202,195 source returns
matched every stored band and mask exactly; see the
[Kentucky verification record](benchmarks/lidar-kentucky-2026-11-16.json).
The actual desktop load entry point also passed with an initially empty prepared
archive and cached public source blocks, alongside the RTX 4090 scene test.

After the location was clarified as western Tennessee, actual USGS surveys were
also prepared and rendered near Paris/Kentucky Lake, Jackson, Dyersburg and
Memphis. Measured pixels covered 97.03–99.96% of these four display tiles, with
unmeasured areas left transparent. Independent reduction of 31,018,101 decoded
returns matched every min/max/mean/count value and coverage mask; see the
[western Tennessee verification record](benchmarks/lidar-tennessee-2026-11-16.json).
No additional code change was needed for these Tennessee surveys.

## Benchmark protocol

`scripts/benchmark_lidar_fusion.py` compares the original COPC map read with the
prepared raster, and full-file raster construction with Python and pinned
GPUPDAL 0.1.0. The manifest lists `name`, `path`, `survey`, and app tile `z/x/y`.

```sh
uv run --no-project --with numpy --with pillow --with 'laspy[lazrs]' \
  --with pyproj --with rasterio python scripts/benchmark_lidar_fusion.py \
  --manifest fixtures.json --output /tmp/lidar-fusion-benchmark \
  --gpupdal /path/to/gpupdal --runs 3
```

One warmup precedes three alternating timing pairs. Full-file builds include
decoding, filtering, reprojection, binning and raster compression; Python also
checks the stored raster round trip. CLI startup is included for GPUPDAL;
interpreter startup is excluded for Python. Map-read timings include shading,
but exclude app scheduling, Python startup, GPU upload and frame presentation.
OS and drive caches are uncontrolled. GPUPDAL is measured through its ordinary
public pipeline; a CUDA-capable `doctor` result does not establish that these
particular stages ran on the GPU.

Current COPC reads choose a resolution-dependent sample. The fused product uses
every valid return, so the two rendered maps are deliberately not pixel-identical.
The report records shared coverage and height differences separately from the
Python/GPUPDAL full-density raster comparison. Float32 masks are read using each
GeoTIFF's nodata metadata; zero counts are empty cells. Claims about full-density
numeric parity must not be inferred from similarity to the sampled preview.

The [initial measured results](benchmarks/lidar-fusion-2026-11-16.json) retain input hashes,
machine/dependency details, individual timings, storage sizes and numeric checks.

For native/Python migration comparisons, run:

```sh
uv run --no-project --with numpy --with pillow --with 'laspy[lazrs]' \
  --with pyproj --with rasterio python scripts/benchmark_native_lidar.py \
  --manifest docs/benchmarks/lidar-fusion-fixtures-2026-11-16.json \
  --native target/release/one-thousand-electric-eye-lidar \
  --output /tmp/native-lidar-benchmark
```
