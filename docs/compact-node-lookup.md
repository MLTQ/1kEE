# Compact node lookup for Build All

The cache builder can now resolve coordinates from the original compressed OSM
PBF, avoiding the expanded `planet_nodes.bin` intermediate. It records node-ID
ranges and PBF byte offsets in a small SQLite index, then decodes needed blocks
into a bounded memory cache. This changes building, not in-app tile loading or
the `.1kc`/`.1ka` formats.

## Try it

Reopen the rebuilt Cache Builder, select **Full Planet**, and enable **Compact
node lookup (experimental — saves disk space)**. Keep the original PBF available
throughout the build. The CLI equivalent is:

```sh
target/release/one-thousand-electric-eye-cache-builder planet-all \
  --planet /path/to/planet.osm.pbf --out-dir /path/to/vector-cache \
  --tmp-dir /path/to/build-work --features all --node-storage indexed-pbf
```

Flat node storage remains the default. Compact mode does not reclaim space from
existing expanded node files; it avoids creating another one. It initially
scans the whole PBF to build the index, then processes ways. It does not yet
eliminate other inputs needed for a fully source-free world archive.

## State and compatibility

- Compact files live under `WORKING_FOLDER/indexed-pbf-v1/`: `nodes.sqlite`,
  `ways_checkpoint.txt`, and an OS-managed `build.lock`.
- Legacy `planet_nodes.bin`, sparse index and `checkpoint.txt` are left intact.
  Switching modes starts a separate build; it does not adopt a legacy checkpoint.
- Index transactions persist block descriptors and scan offsets together.
  Interrupted scans resume; decoded node blocks can be discarded and reread.
- Source identity includes file length, modification time, device and inode.
  Replacing/changing the source fails rather than silently reusing an old index.
- Feature resume also binds to selected layers and canonical output/elevation
  paths. Changed settings require a new working folder. Elevation file contents
  must remain unchanged; their individual content fingerprints are not tracked.
- A completed job with the same settings is a no-op. To rebuild missing output,
  choose a new working folder. Existing cell merge/checkpoint durability behavior
  is unchanged (follow-up `1kee-0ke`).
- Node IDs must be strictly increasing throughout the PBF. Unsorted/history
  input is rejected with guidance to use flat storage.
- Each active lookup session retains up to 16 MiB of decoded node arrays plus
  up to 12 MiB of grouped way references. Temporary decode/query buffers, the
  descriptor index and normal feature-output buffers require additional RAM.
- This does not add missing admin-relation support to `planet-all`.

## Measurements, 2026-09-22

Release build, three alternating runs per mode, source extracts on Hilbert,
fresh scratch/output on the internal drive, all supported feature selections,
no elevation baking. Times are medians; OS caches were not purged. Scratch is
final logical working-file size, excluding source and output, not peak space.

| Extract | Flat scratch | Compact scratch | Flat time | Compact time |
|---|---:|---:|---:|---:|
| California, 36/-121 | 17.97 MB | 12.36 KB | 0.20 s | 0.22 s |
| Delhi, 28/77 | 88.67 MB | 36.94 KB | 1.35 s | 3.45 s |
| Colombia, 4/-75 | 45.02 MB | 24.65 KB | 0.72 s | 2.07 s |

Exact normalized feature bytes matched across modes and every repeat: 52,958,
885,103 and 317,369 stored feature records respectively. Compact scratch fell
by more than 99.9%. On Delhi, peak process memory was 634–688 MiB compact versus
497–568 MiB flat. Compact mode trades disk space for decompression and memory;
these small extracts do not establish full-planet throughput. It stays opt-in
pending larger-scale profiling (`1kee-7o5`).

Grouped way dependencies reduced the initial Delhi compact implementation from
8.85 s to 3.45 s without changing output. Reproduce the comparison with
`scripts/benchmark_node_storage.py`; it deletes only its own temporary results.

Regression coverage includes ordinary/dense nodes, shared/duplicate/missing
references, eviction/prefetch limits, index interruption and resume, stale source,
corrupt descriptors, exclusive build locking, feature parity and legacy-state
preservation. It does not run a full-planet conversion.
