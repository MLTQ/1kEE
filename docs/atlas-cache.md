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

The map saves terrain it generates while browsing, even before an Atlas builder
job has run. Completed native windows are saved losslessly in
`Derived/terrain/atlas_recent_views_v1.sqlite`, beside the other terrain caches.
Both measured heights and prepared color data are retained; lighting, contours,
projection and exaggeration stay live. Reloading a saved window avoids GDAL and
color preparation. Existing height-only entries are reused and upgraded on load.

Small pans reuse padded coverage. Returning to a retained region uses its
prepared RAM data immediately, including while another region is still loading.
There is no eight-view RAM cap or sixteen-view disk cap. Retention is by bytes:

| Setting | Normal launch | `./scripts/run-4090.sh` |
| --- | --- | --- |
| `ONEKEE_ATLAS_RAM_MIB` | 1024 MiB | 8192 MiB |
| `ONEKEE_ATLAS_DISK_MIB` | 8192 MiB | 32768 MiB |

Environment overrides take precedence. RAM accepts 256–32768 MiB; disk accepts
512–262144 MiB. These are lazy cache budgets, not up-front allocations. RAM evicts
least recently used prepared views; disk removes oldest written windows only
when compressed heights plus colors reach its byte budget. SQLite allows 25%
plus 256 MiB above that payload budget for pages; its journal is additional.

Globe and Local keep their last GPU terrain and rendered targets when hidden,
so switching modes/themes back does not upload them again. Normally GPU sources
are bounded to current and incoming windows in each mode. The Globe residency
experiment below keeps older completed Globe windows too. Current rendering resources and the bounded background
save queue have separate RAM lifetimes. Cache Blast/source invalidation clears
RAM and GPU views explicitly. Recent saved windows survive a desktop restart.

New regions still need their first native-source read. Persistent keys include
source identities, exact padded bounds and dimensions; corrupt heights are
regenerated, while missing/stale/damaged colors are rebuilt from cached heights.
A bounded writer keeps compression and disk writes off the rendering path. A
full write queue can skip a save; read/write failures fall back without hiding
terrain. The recent-view cache is independent of OSM builds and Atlas prebuilds.

## Globe residency experiment

The 4090 launcher now sets `ONEKEE_ATLAS_KEEP_GLOBE=1`. Every completed Atlas
Globe relief window stays in RAM and, once uploaded, VRAM until **Cache Blast**
or desktop exit. Panning, zooming, visiting other regions and switching views
retain it. Returning to covered terrain reuses the original raster and uploaded
textures. Only currently selected terrain is drawn; old resident regions add no
extra terrain draw calls. Incomplete abandoned GPU uploads are discarded.

This option applies only to **Globe**. Local keeps its existing RAM budget and
GPU replacement policy. The pinned Globe pool is separate and bypasses the RAM
LRU budget; memory grows as new Globe regions finish loading. Startup logs the
mode, and each new retained region logs the count and prepared RAM size. No
whole-planet preload is performed. Persistent disk cache behavior is unchanged.
Cache Blast also rejects old workers so they cannot re-pin terrain after clear.

Use `./scripts/run-4090.sh` to test. To compare against bounded residency:

```sh
ONEKEE_ATLAS_KEEP_GLOBE=0 ./scripts/run-4090.sh
```

Ordinary launches default to bounded residency unless the variable is set.
