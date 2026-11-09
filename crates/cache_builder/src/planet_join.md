# planet_join.rs

Compact planet-way coordinate resolution via an external sequential join.
It replaces repeated node-block reads across way batches. Administrative
relations still use their existing independent assembly/coordinate pipeline.

## Stages

1. **Collect way dependencies** scans from the existing durable way checkpoint.
   Groups stop after roughly 32 MiB of PBF input or 8 million raw references
   (at a blob boundary). Selected references are sorted/deduplicated per group
   and written as `(node_id, group_number)` 16-byte records, partitioned by
   ranges of 128 original node blocks. Unselected ways do not generate requests.
2. **Join coordinates** externally sorts one partition at a time by node ID.
   It decodes each required original block once, using up to 16 consecutive
   required blocks in parallel and one million requests per window. The last
   block carries over across windows, so skewed requests do not cause rereads.
   Matching coordinates append to the corresponding group file as 16-byte
   `(id, f32 latitude, f32 longitude)` records. Missing IDs remain missing.
3. **Building prepared ways** rereads the way groups in their original order,
   loads their verified coordinate tables, and uses the unchanged classification,
   geometry, elevation and cell-writing code. Missing IDs in an exhaustive table
   return None, never falling back to original-source reads. Every emitted group
   flushes before the existing way checkpoint advances.

## Recovery and compatibility

State lives under `indexed-pbf-v1/way-join-v1` and is protected by the existing
compact-build directory lock. The JSON manifest binds version, source metadata
(excluding remount device number), original start offset and exact build options.
The outer node index still verifies source fingerprint and metadata before use.
Each stage checks live source identity before checkpointing. Old indexes, output
cells and way checkpoints remain compatible; no fresh folder is needed.

Append spools contain checksummed segments. Their data is synced before manifest
replacement (temporary file, sync, rename, directory sync). Recovery truncates
only uncommitted suffixes and rejects missing committed data or checksum errors.
Collection commits one group; joining commits one node partition. A crash may
replay the current partition, but never publishes a half-prepared group as done.
Cell emission resumes only at a known prepared-group boundary. An old cleared
way checkpoint can safely replay from this join's original start after a crash
between way completion and overall completion publication.

Request partitions and external-sort runs are removed after their join commit.
Coordinate files are retained for replay. Their total size is at most the initial
request spool size (both use 16-byte records). Sorting needs additional scratch:
headroom checks reserve three times the current request partition plus 1 GiB;
collection checks its next request append plus 1 GiB. These are best-effort `df`
checks, not disk reservations or estimates of eventual output cell size. Errors
preserve stage checkpoints and are resumed using the same working directory.

Progress reports separate stages, source traversal, scratch volume, partitions
and node reads. These are work counters, not geographic coverage or an overall
ETA. Node-position traversal includes skipped source ranges; physical reads can
be smaller. The running older executable is never switched or stopped by a build.
