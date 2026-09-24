# Packed runtime tile archive

New Earth builds now store nonoverlapping cores; existing caches remain intact.
See [contour-core-storage.md](contour-core-storage.md) for the current layout,
compatibility and measured storage reductions. Earlier full-footprint measurements
below describe the legacy overlapping tiles.

## First usable version

The desktop can read `Derived/world.1ka`, a SQLite archive containing packed map
tiles. This first version improves cached feature/contour loading. It is not yet
a complete replacement for every static source. No whole-planet conversion is
started automatically.

- Vector cells become 64 eighth-degree tiles. Small views decode only overlapping
  tiles and retain full feature geometry, names and baked heights.
- Each contour tile becomes one CTF1 payload of the f32 coordinate arrays the
  renderer uses. GeoPackage decoding and f64 conversion happen during packing.
  This preserves rendered geometry, not the original f64 source precision.
- Body/layer/grid/detail/tile coordinates are indexed separately. Earth vectors
  currently use full detail at level 0. Earth, Moon and Mars contour keys retain
  their existing zoom/bucket schemes without cross-body collisions.
- CRC32 checks and decoder bounds reject corrupt payloads. Empty imported tiles
  differ from absent coverage. Presence describes the imported cache snapshot;
  it does not prove the underlying OSM source was completely built.
- Background workers own independent read-only connections. Missing, corrupt or
  changed archive data falls back to existing cache readers per cell/tile.

The `.1kc` reader now decodes only the requested chunk. Roads load major/minor
classes together and retain ready cells when neighbors are missing. Roads,
waterways, buildings and trees reuse valid baked heights; only missing heights
require terrain sampling.

## Create and use an archive

In the rebuilt Cache Builder, select the vector cache directory and click
**Pack vector archive…**. Save it as `world.1ka` directly under the desktop's
configured Derived directory. Completed cells appear in the GUI log. Restart
the desktop after installing an archive snapshot.

The CLI can combine vectors and existing contour databases. Supply only the
sources wanted:

```sh
target/release/one-thousand-electric-eye-cache-builder pack-archive \
  --osm-cache-dir /path/to/Derived/osm \
  --earth-contours /path/to/Derived/terrain/srtm_focus_cache.sqlite \
  --moon-contours /path/to/Derived/terrain/lunar_focus_cache.sqlite \
  --mars-contours /path/to/Derived/terrain/mars_ctx_cache.sqlite \
  --out /path/to/Derived/world.1ka
```

Existing destinations are rejected. Conversion writes a unique staging database,
closes it, and publishes a new file without overwriting data. Errors clean owned
staging files. A later snapshot currently needs a new output filename;
incremental repacking is future work.

The final filename appears only after packing completes. A failed pack removes
its temporary output and reports the offending source file in the log. Existing
cells at exactly +180° longitude or +90° latitude are accepted, matching the
cache builder's inclusive boundary convention.

Packing needs space for the additional archive and preserves source files. In
the inspected Hilbert installation the Earth contour database alone is about
269 GiB, while free space is about 23 GiB. A full duplicate terrain archive needs
additional capacity or a later incremental migration strategy. Benchmarks used
small isolated copies.

The inspected vector inputs total 19.89 GB across 77,053 cells. The output can
exceed that size because features repeat across child tiles and SQLite adds an
index. A drive showing 23.8 GB free after a failed pack may have been filled by
the temporary archive and recovered that space during cleanup; it is not a
measurement of free space at the instant the write failed.

Packing now logs source count/bytes, the actual destination, temporary archive
growth and destination free space. Failures capture those numbers before cleanup.
A best-effort check keeps 1 GiB of working space, checked at most once a second;
other writers or a large tile can still consume capacity between checks. Unknown
capacity does not block packing. This reports and guards space, but does not
compress the archive or make a full conversion fit a nearly full drive. Source
bytes are not an output-size estimate. Use additional output capacity or free
space explicitly; the packer never deletes source/cache data to make room.

## Measured loading behavior

Release builds on the user's APFS NVMe, with both representations on that drive.
Vector runs alternated 49 views of 0.10° × 0.10°, opening/reading/decoding/filtering
each representation, including archive source-stamp validation. Every returned
feature byte matched after normalizing order. Conversion warmed the data;
these are not controlled cold-cache measurements.

| Dataset | Loose median | Packed median | Loose p95 | Packed p95 |
|---|---:|---:|---:|---:|
| Paris roads, 366,998 source features | 47.988 ms | 0.952 ms | 50.405 ms | 5.664 ms |
| California waterways, 11,878 source features | 3.462 ms | 0.489 ms | 3.791 ms | 0.775 ms |

The Paris source cell was 37,000,310 bytes and its archive 37,965,824 bytes.
The waterway source was 5,984,738 bytes and its archive 7,176,192 bytes. Whole
features may occur in multiple child tiles, increasing storage.

Whole-cell Paris reads took 120.594 ms packed versus 46.322 ms loose; waterways
took 15.453 ms versus 3.699 ms. The desktop therefore prefers an available original
binary cell when a view touches more than 16 of its 64 children. Archive-only wide
views still work, but retain this cost until prebuilt detail levels exist.

The actual desktop contour reader was benchmarked on a 71,220-contour tile using
temporary legacy/packed copies on the same NVMe. Four alternating samples each
gave approximately **48.23 ms legacy versus 41.00 ms packed** median, about 15%
less elapsed time. Geometry, part/elevation ordering, simplification and selection
matched. Bundling original WKB blobs was slower and was replaced by CTF1 before
shipping.

These measure read/decode/filter or contour preparation, **not pan-to-visible-frame
latency**. Rendering, missing-height sampling, imports, concurrent app activity,
tile density and cold storage can dominate the complete interaction.

Reproduce vector measurements with a new disposable destination:

```sh
cargo run --release -p tile-archive --example read_latency -- \
  /path/to/road_cell_+048_+0002.1kc /path/on/same/drive/benchmark.1ka
```

The contour benchmark creates/removes only its own subdirectory:

```sh
ONEKEE_CONTOUR_BENCH_DB=/path/to/srtm_focus_cache.sqlite \
ONEKEE_ARCHIVE_BENCH_DIR=/path/to/disposable/benchmark-parent \
cargo test --release -p one-thousand-electric-eye-desktop \
  benchmark_packed_contour_tile -- --ignored --nocapture --test-threads=1
```

## Current boundaries and next steps

- Original contour databases still supply manifests and scheduling. A changed
  database/WAL fingerprint disables packed contour reads until repacked.
- Vector files without baked heights still need runtime elevation sources.
- Live-import water areas, admin/labels, imagery, terrain height pyramids and
  other static assets are not yet fully contained in the archive.
- Raw PBF/Overpass fallback imports are not globally disabled. This is not yet
  the proposed source-free runtime mode.
- Detail levels, progressive vector publication, replacement invalidation during
  a running session and incremental/direct builder writes remain follow-up work.

Tracked under `1kee-vgj` (this slice), `1kee-i7c` (complete archive), and `1kee-6ws`
(runtime integration). See [cache-builder.md](cache-builder.md) for the wider plan.
