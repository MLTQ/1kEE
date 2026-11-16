# Fused LiDAR elevation layer

The LiDAR map can read compact numeric rasters prepared from downloaded scans.
Automatic mode composites acquisitions in descending catalog-date order. A newer
scan wins only at measured pixels; an older acquisition can fill its gaps. An
explicit survey selection still shows that acquisition. Automatic maps have a
separate cache and never substitute an arbitrary historical selection.

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

GPUPDAL provided no clear build advantage: it was slower on Richmond/Ozarks and effectively tied on Switzerland. Its ordinary raster pipeline also assigned some returns to different grid cells: Richmond had 280 count-cell differences (maximum shared-cell mean difference 0.77 m); Ozarks matched; Switzerland had 43 coverage differences and a 0.015 m 95th-percentile shared-cell mean difference (maximum 2.15 m). No cause is asserted without a separate reprojection/binning investigation. The Python path was selected.

Full-density means differ from the old COPC LOD preview: shared-cell 95th-percentile height differences were 6.07 m in Richmond, 2.39 m in the Ozarks and 1.01 m in Switzerland. These are sampling/product differences, not a claim of measured ground accuracy. Fused coverage increased by 9, 55 and 30,654 pixels respectively.

The integrated Richmond and Swiss survey renders passed with raw-cloud reading and publisher access blocked. Synthetic tests independently reduce original points and cover latest-date overlap/gaps, noise/withheld rejection, failed publication, corrupt-raster repair, source receipts, resume and mount identity.

Example outputs:

- richmond-2025: [sampled preview](benchmarks/lidar-fusion-views/richmond-2025-current.png), [full-density fused tile](benchmarks/lidar-fusion-views/richmond-2025-fused.png)
- ozarks-2024: [sampled preview](benchmarks/lidar-fusion-views/ozarks-2024-current.png), [full-density fused tile](benchmarks/lidar-fusion-views/ozarks-2024-fused.png)
- swiss-2026: [sampled preview](benchmarks/lidar-fusion-views/swiss-2026-current.png), [full-density fused tile](benchmarks/lidar-fusion-views/swiss-2026-fused.png)

## Build and resume

Use the LiDAR Python dependencies (NumPy, Pillow, laspy with lazrs, pyproj,
rasterio, requests). Inspect the existing completed-scan inventory first:

```sh
uv run --no-project --with numpy --with pillow --with 'laspy[lazrs]' \
  --with pyproj --with rasterio --with requests python scripts/lidar_fused.py \
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
uv run --no-project --with numpy --with pillow --with 'laspy[lazrs]' \
  --with pyproj --with rasterio --with requests python scripts/lidar_fused.py \
  --root /run/media/m/Minkowski/Data/Lidar --volume-uuid 6FEF-269A --verify
```

The current batch supports present, completed, direct COPC scan receipts in
`bulk.sqlite`. Existing DEM/relief rasters continue through the raster path.
EBA ZIP members, separately selected NASA granules, wrapped bounds and scans
requiring more than 16 million output cells are not yet converted by this batch.
Oversized/invalid scans are recorded in the index's `failures` table and their
raw path remains available. Full-volume/offline errors pause the batch. Retention
changes need a whole-inventory coverage audit, including these exclusions;
success on three test scans does not authorize treating every raw asset as
redundant. Old content-versioned rasters are retained for active readers and
need a later reclamation pass.

The 2026-11-16 rollout is running as the transient user service
`1kee-lidar-fusion`, covering 5,225 present COPC receipts (311.7 GB of raw
scans), newest first. It runs at low CPU/I/O priority with a 4 GiB memory cap;
its sandbox permits data-drive writes only under `Lidar/fused`. Raw inputs are
read-only. Follow progress with `journalctl --user -fu 1kee-lidar-fusion` and
stop with `systemctl --user stop 1kee-lidar-fusion`. After a stop or reboot,
rerun the build command above to resume. This service converts a snapshot of
the local inventory; it does not download the world's remaining scans.

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

The [measured results](benchmarks/lidar-fusion-2026-11-16.json) retain input hashes,
machine/dependency details, individual timings, storage sizes and numeric checks.
