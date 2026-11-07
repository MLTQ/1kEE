# Terrain tile recovery

Background contour readers distinguish malformed SQLite/GeoPackage geometry from
transient busy, permissions, I/O and storage failures. A failed tile is never
published as successfully empty. Row counts must match a readable manifest.

After releasing the reader snapshot, recovery takes a writer transaction,
rechecks the tile, and deletes only its contour/coastline rows and manifests.
Rechecking prevents a late failure from deleting a newer valid replacement.
Manifest revision, retry state and repaint are updated so the normal bounded
on-demand builder can regenerate the tile from the original sources.

SQLite page corruption may prevent even a transactional delete. The transaction
rolls back; recovery creates a persistent per-tile invalidation directory:

```
terrain/.tile-repairs/<original database>/<zoom>_<lat>_<lon>/<original database>
```

The directory shadows that key in the original database even before replacement
geometry exists. Builders write a fresh isolated database there; readers and
manifest/status lookups prefer the completed replacement. Keeping the original
filename preserves native SRTM quality tagging. Healthy neighboring tiles still
use their original storage. The corrupt bytes remain in the old database because
SQLite cannot safely delete them; reclaiming that space requires a separate
verified database salvage/compaction. Removing repair directories would expose
the old damaged entries again.

A damaged manifest window tries individual keys to retain readable neighbors and
allows missing keys to build into isolated databases. An unreadable original
header likewise does not prevent replacement scheduling. Ordinary transient
errors remain errors and never activate this fallback. Once a database is marked
damaged, newly requested missing tiles also build outside it. Builds retain the
existing source checks, worker limits, low-space protection and retry backoff.

The headless maintenance command repairs/validates one native SRTM tile without
opening a window or starting a planet job:

```
target/release/one-thousand-electric-eye-desktop --repair-native-tile \
  /path/to/terrain/srtm_native_v1.sqlite /path/to/SRTM_GL1_srtm 4 126 143
```

It requires source coverage, preserves already healthy tiles, and checks the
regenerated manifest and every geometry row. This command writes the requested
cache; it is not a read-only diagnostic. Hosted detail, Moon and Mars use their
normal on-demand schedulers with the same invalidation/routing behavior.

Tests cover atomic invalidation/rebuild, stale-reader races, malformed blobs,
missing rows, valid empty tiles, neighbor retention, restart-persistent shadowing,
invalid SQLite headers and physically corrupted manifest index pages. Runtime
integration verifies a malformed blob cannot become a successful empty read.

## Isolated recovery on a different disk

`ONEKEE_TERRAIN_REPAIR_ROOT=/absolute/path` or the persistent application setting
`terrain_repair_root` places replacements and build scratch on another filesystem.
An original-path hash separates identically named caches on different volumes.
With this setting, original caches are never opened for deletion: bad entries are
logically invalidated and regenerated in the isolated store. All newly missing
terrain tiles go there too, while readable original tiles remain available.

Original database reads use SQLite immutable mode in this configuration to avoid
even shared-memory sidecar writes on unreliable source storage. This reads the
last checkpointed database, not uncheckpointed WAL rows. Missing coverage can be
regenerated; original WAL/SHM files are retained for separate salvage. Do not use
this mode with another process actively modifying the source database.

A live Minkowski repair on 2026-11-06 exposed kernel USB/UAS write aborts,
`critical target error` and writeback errors on /dev/sde. In-place repair was
stopped; this workstation uses internal isolated recovery instead. Regeneration
cannot repair the USB device/enclosure/filesystem write failure itself.

## Live validation and remaining source damage

On Minkowski, native tile 4/126/143 rebuilt into internal btrfs storage in about
0.4 s: 3,633 contour records, 4,871,535 geometry bytes, with all geometry and the
manifest validated. A bounded serial pass over 103 unique tile failures from the
recent desktop journal verified that replacement and attempted the remaining 102.
Those remaining attempts failed on unreadable raw SRTM TIFF inputs (invalid TIFF
headers or truncated encoded raster blocks). Their invalidation markers remain,
so the original bad geometry is not treated as a completed tile. Regeneration can
retry after clean source data is restored. No whole-cache scan or source rewrite
was performed, and this does not claim the remaining 102 tiles were repaired.

The workstation's persistent recovery destination is
`/home/m/.local/share/1kee/terrain-repairs`; its original settings were backed up
beside `target/release/.1kee_settings.json`. The repair batch preserved at least
10 GiB of free internal storage. Full desktop tests: 394 passed, 54 opt-in tests
ignored; the explicit source-byte/sidecar preservation regression also passed.

### Correction after drive reconnection

At 23:22 the user power-cycled/reconnected Minkowski. All 29 SRTM TIFFs named by
the earlier failed builds then passed complete GDAL checksum reads. The device
had been offline during the previous failures; those failures do **not** establish
permanent source-file damage or a need to redownload these 29 files. Original
source files were not modified. The six sampled checkpointed SQLite cache queries
still reported malformed database pages, so isolated cache regeneration remains
appropriate. These checks do not prove equality to the Mac originals or validate
every file on the drive.

SMART after reconnection reported PASSED, critical warning 0, 26% estimated
lifetime used, 100% spare, 0 media/data-integrity errors, 50 C, 5,088 power-on hours,
about 379 TB written and 17 unsafe shutdowns. It also reported 52 cumulative error
log entries, but the USB bridge truncated the detailed error/self-test log pages.
No short/long device self-test or full surface scan was performed. Kernel logs
showed UAS resets, read/write errors, an offline device and an actual read-only
filesystem remount at 23:07:14 before reconnection. This suggests an intermittent
connection/enclosure/controller problem rather than exhausted flash endurance,
but the faulty component has not been isolated and SMART PASS does not certify
filesystem integrity or reliable writes. Device diagnosis remains `1kee-986`.

After reconnection, the bounded regeneration pass completed successfully for all
103 logged native tile failures. All replacements passed manifest/geometry
validation in the internal recovery store. The source device remained running,
and the kernel recorded no new storage errors during these post-reconnect checks.
No repairs or writes were made to the original source volume during this retry.
The earlier 102-source-failure blocker is therefore resolved for these requests;
`1kee-987` is closed. This is not a full database/drive integrity certification.
