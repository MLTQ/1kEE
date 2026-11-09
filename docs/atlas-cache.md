# Building the Atlas terrain cache

Open the rebuilt Cache Builder and select **Atlas**. Choose the Data folder
containing GEBCO, an optional SRTM folder, and the output file. The normal output
is `Derived/terrain/atlas_heights_v1.sqlite` under the desktop's configured
Derived folder. Select **Full planet** or **Bounding box**, choose detail tiers,
and click **Build / verify / resume Atlas cache**.

| Tier | Source and spacing | Core tile | Full-planet scope |
| --- | --- | --- | --- |
| Overview | GEBCO, 5 arcminutes | 10° | 648 tiles, about 39 MB |
| Terrain | GEBCO, 15 arcseconds | 1° | 64,800 tiles, about 15.2 GB |
| Land detail | GEBCO plus SRTM, 1 arcsecond | 0.25° | Available SRTM cells only; potentially hundreds of GB |

Sizes describe uncompressed height payloads, including edge samples, before
SQLite overhead. Overview and Terrain are selected initially. Land detail is
optional. No source downloads occur. GDAL must be installed, or its bin folder
selected. The Data folder is expected to contain
`GEBCO/gebco_2025_sub_ice_topo_geotiff`; a directory containing the original
`gebco_*.tif` topography files also works. Do not select GEBCO's TID rasters:
they contain source identifiers rather than elevations.

The dashboard updates built/reused/remaining counts, bytes written, elapsed time,
and verification/build throughput. GDAL work emits a heartbeat each second.
Each completed tile is published independently. Restarting a stopped or failed
job with the same output verifies tile checksums and source identities, skips
matching tiles, and rebuilds damaged or stale entries. A failed tile is not
published. A single-writer lock prevents two builders from writing this cache
at once; the desktop can read committed tiles while a build runs.

Changing coverage or tiers does not discard earlier tiles. A bounding-box run
builds the complete tiles intersecting that box. A date-line crossing uses an
east longitude above 180°, for example west 179° and east 181°. SRTM cells outside
the selected source collection are counted separately from remaining work.

The cache holds measured floating-point heights, not screenshots. Atlas loads
its globe overview and detailed Globe/Local windows from the cache. Lighting,
palette, terrain exaggeration, isolines, and forest overlays remain live.
Original raster reads remain the fallback when a requested tier is incomplete
or unreadable. If raw SRTM covers a close view, Atlas keeps that native detail
until the matching Land tier is built; it does not silently replace it with the
coarser Terrain tier. Existing fine-provider chunks remain preferred at close
zoom. Restart the desktop to discard any already-loaded Atlas snapshots.

The Atlas job is independent of OSM cache building and its working folders. It
does not modify an existing planet build or clear vector/contour caches.

## CLI

```bash
cargo build --release -p one-thousand-electric-eye-cache-builder -p one-thousand-electric-eye-desktop

target/release/one-thousand-electric-eye-cache-builder atlas \
  --data-root /run/media/m/Minkowski/Data \
  --out /run/media/m/Minkowski/Derived/terrain/atlas_heights_v1.sqlite \
  --tiers overview,terrain
```

For a bounded high-detail build, add `--tiers terrain,land`,
`--srtm-root /path/to/srtm_gl1`, and
`--min-lon 7 --min-lat 46 --max-lon 8 --max-lat 47`. Without explicit bounds the
scope is the full planet. Without `--srtm-root`, Land builds look for `srtm_gl1`
inside the selected Data folder. `--gdal-bin-dir /path/to/bin` overrides PATH.

## Verification

`cargo test --release -p tile-archive -p one-thousand-electric-eye-cache-builder -- --test-threads=1`
checks storage, shared cache contracts and builder regressions. With GDAL installed,
run the private synthetic raster tests with
`cargo test --release -p one-thousand-electric-eye-cache-builder gdal_atlas -- --ignored --test-threads=1`.
Desktop cache consumption is covered by
`cargo test --release -p one-thousand-electric-eye-desktop atlas -- --test-threads=1`.

## Interactive map reuse

The map now caches terrain it generates while browsing, even if you have not
run an Atlas builder job. Small pans reuse the current padded window. Recently
prepared views stay in a bounded RAM cache; completed native windows are also
saved losslessly in `Derived/terrain/atlas_recent_views_v1.sqlite`. This uses at
most 16 windows / 512 MiB of compressed payloads, with a 768 MiB database ceiling
plus temporary journal space. Saving happens on a bounded background worker.

Revisiting a retained window avoids GDAL, and the disk cache survives desktop
restarts. New terrain still needs its first native-source read. Source changes
invalidate matching disk entries; unreadable entries are regenerated. The
recent-view cache is independent of the OSM job and Atlas prebuild database.
RAM reuse is capped at eight windows / 384 MiB of retained height and color data;
current rendering resources and the bounded save queue have separate lifetimes.
