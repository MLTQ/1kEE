# Cache builder: throughput and portable static data

New Earth builds now store nonoverlapping cores; existing caches remain intact.
See [contour-core-storage.md](contour-core-storage.md) for the current layout,
compatibility and measured storage reductions. Earlier full-footprint measurements
below describe the legacy overlapping tiles.

## Scope

The GUI's whole-planet job runs `planet-all`: an OSM vector build. It is separate
from the Earth contour, Moon, and Mars jobs. Selecting every OSM layer does not
produce a complete terrain/imagery/labels archive.

## Implemented speed improvements (2026-09-22)

- Pass 1 decodes up to 64 PBF blobs in parallel and encodes compact node records
  before one writer appends them in source order. Nodes are synced before the
  batch-end checkpoint advances; interrupted tails are truncated on resume.
- Pass 2 keeps 64 node blocks per active Rayon task (at most 4 MiB of node bytes).
  Nearby/shared way references reuse those bytes instead of repeatedly allocating
  and reading the same 64 KiB blocks. No shared cache lock is needed.
- `planet_nodes.sparse-index-v1` persists the small lookup index beside the node
  store. Reopening validates file identity, timestamps, size, record count,
  checksum, order, and stride. Stale or corrupt indexes rebuild automatically;
  the node store and existing checkpoints keep their original formats.
- Cold sparse-index construction uses parallel positional reads. The GUI now
  reports index startup before opening it, instead of showing the message after
  the expensive operation has already finished.
- PBF decode and node-read failures stop Pass 2 before checkpoint advancement.
  Previously a failed blob or unreadable node block could silently appear to
  contain no useful features. Errors now include node byte offsets/way IDs.

The output `.1kc` schema, coordinate conversion, feature selection, polygon flags,
names, optional elevations, and node-reference order are unchanged. More workers
cannot eliminate storage throughput limits or the sequential output merge stage.

## Measurements and reproduction

Measured in release mode on the 10-core, 32 GiB machine, reading a real
48,638,183-byte regional PBF from Hilbert and writing disposable outputs to
internal temporary storage. This runs the same `planet-all --features all`
pipeline as the whole-planet job. Elevation baking was disabled.

The final alternating comparison, including sparse-index persistence, was:

| Round | Original builder | Improved builder |
|---|---:|---:|
| 1 | 3.479 s | 1.752 s |
| 2 | 2.794 s | 1.170 s |
| 3 | 2.813 s | 1.176 s |

This is about 50–58% less elapsed time for the tested pipeline.

Every run produced 39 cell files, 720,297 feature records, 6,031,416 points, and
60,054,239 bytes. Verification compares every encoded feature byte while ignoring
only HashMap feature ordering. An earlier six-run comparison also checked
matching node-file SHA-256 values before the index sidecar was added.
Normalized cell hash: `ed8556eec590dea34116acdcdbef5e58683555d6b2d8b827e76d4a8a319ce3c7`.

These short regional runs do not predict whole-planet time: they have much better
node locality, a smaller lookup index, and cheaper cell rewrites. There was no
cold-cache control or interactive frame-rate measurement. The full node store on
Hilbert is 167,594,015,248 bytes (10,474,625,953 records); its original serial
sparse-index opening was still running after more than three minutes and was
stopped before completion. That is a lower bound, not a completed timing.

The subsequent full-store index benchmark failed after 244.79 seconds with
`Input/output error (os error 5)` while reading that node file. Hilbert still had
24 GiB free. Limited eight-byte probes at its beginning, middle, and near the end
passed; they do not establish that the rest of the file is healthy. This run
provides no valid full-store throughput or index-reopen speed result. The source
node file and the user's existing checkpoints/caches were not modified.

The reusable index path is covered by tests for valid reopening, source-file
replacement, checksum corruption, truncation, and changed metadata. It avoids
reissuing roughly 2.6 million tiny source reads on a valid reopen; it cannot
repair an unreadable source block. Investigate the full-store read error before
using this benchmark to make whole-planet time estimates.

Use two retained release binaries to reproduce the complete regional comparison:

```bash
python3 scripts/benchmark_planet_builder.py \
  --before /tmp/builder-before \
  --after target/release/one-thousand-electric-eye-cache-builder \
  --source /path/to/representative-extract.osm.pbf --rounds 3
```

The script creates/removes its own outputs and fails if any cell data differs.
Do not use the application's existing cache as benchmark output.

The ignored node benchmark reads a bounded sample from a real matching PBF/node
pair, measures index construction/reopening and repeated way lookups separately:

```bash
ONEKEE_PLANET_BENCH_PBF=/path/to/planet.osm.pbf \
ONEKEE_PLANET_BENCH_NODES=/path/to/planet_nodes.bin \
ONEKEE_PLANET_BENCH_OFFSET=68582930787 \
  cargo test --release -p one-thousand-electric-eye-cache-builder \
  benchmark_real_planet_node_lookups -- --ignored --nocapture --test-threads=1
```

The offset must be a known blob boundary for that exact input; the example is
from the inspected local checkpoint. Node/source files are read-only. The index
benchmark writes only a disposable sidecar in system temporary storage.

## What a single self-contained cache needs

A single indexed static-data archive is a useful target. It must package the
data the renderer actually reads, rather than only collecting contour databases.
The first packed archive and desktop loading slice is now implemented; see
[runtime-archive.md](runtime-archive.md) for usage, measured latency and limitations.
The complete source-independent archive described below remains the larger goal.

| Content | Present state | Archive requirement |
|---|---|---|
| OSM vectors | Separate `.1kc` cells; selected way-based layers | Preserve binary payloads and index by layer/cell; cover relations and point features too |
| Land contours | Separate per-body SQLite caches | Include body, zoom and tile identity; prebuild selected USGS deep-zoom coverage |
| Elevation/bathymetry | Optional baked vector heights plus separate raster/preview inputs and contour-derived fill | Package height/preview pyramids for terrain, shading, and sampling at declared resolution |
| Relief imagery | Separate Natural Earth/terrain assets | Store tiled images/previews and their geographic transforms |
| Labels and boundaries | Separate catalogs/assets; planet-all currently skips admin relations | Package required catalogs and explicit layer coverage |
| Coverage and provenance | Individual tile manifests and filesystem conventions | Record source snapshot, build version, resolution, bounds, completeness, hashes and missing regions |

SQLite is a practical archive backend to prototype because the app already reads
it. It can hold existing binary cell/terrain/raster payloads behind indexed keys,
with one transactional writer and bounded readers. A sealed export can checkpoint
WAL into the main file; active builds can still need temporary journal files.
The public artifact can be one file without forcing the builder to avoid scratch
space while creating it.

The work should proceed in this order:

1. Add a shared archive reader/writer and import existing outputs without
   regenerating their geometry. Keep current file-based readers as a migration
   path. Include body in keys so planetary tile coordinates cannot collide.
2. Write build batches to durable staging/transactions and materialize completed
   cells in bulk. Avoid repeatedly loading and rewriting growing cell files.
   Commit resume positions with their output, and validate input/settings identity.
3. Package terrain heights, relief, catalogs, and missing vector categories.
   Report completeness by layer, region, and resolution; an absent tile must be
   distinguishable from an intentionally empty/ocean tile.
4. Add an explicit archive-only mode: static data comes from the archive;
   raw-source fallbacks and static-network downloads stay disabled. Live event,
   camera, aircraft, vessel, and other enabled live feeds remain independent.
5. Verify representative Earth/Moon/Mars views with source folders unavailable
   and static-network access disabled before treating original datasets as
   unnecessary. Reopen and validate a copied sealed archive as a single file.

Source independence is defined for the archive's declared coverage and resolution.
Planet-wide maximum-detail contours can be much larger than the source rasters;
storage estimates and a coverage budget belong in the build planner. One file
improves portability, but does not itself reduce the amount of geometry.

## Tracked follow-up

- `1kee-i7c`: single-file archive, missing static layers, coverage validation.
- `1kee-6ws`: desktop archive preference and explicit source-free runtime mode.
- `1kee-8ce`: persistent terrain storage budgeting/reclamation.
- `1kee-0ke`: existing cell-update/resume issues: direct file overwrites,
  count-only update skips, missing source/settings identity, and repeat way scans
  after completion. These require durable incremental-output work; they are not
  claimed fixed by the node-throughput changes above.
- `1kee-jw1`: investigate the full node store's OS read error before any repair
  or replacement. The evidence does not isolate hardware versus filesystem/file
  damage; no source deletion or destructive repair was attempted.
