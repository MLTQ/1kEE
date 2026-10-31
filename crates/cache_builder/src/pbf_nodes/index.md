# index.rs

## Purpose
Persists a compact SQLite index of node-ID ranges and byte offsets in the
original PBF, with a durable scan checkpoint after every 32 compressed blocks.

## Contracts
- One transaction writes descriptors and the next source offset. Restarting
  repeats at most an uncommitted batch; decoded nodes are never persisted.
- The index scans all input blocks, so later node blocks/interleaved ways cannot
  be silently omitted. Node IDs must be globally strictly increasing.
- Application/schema IDs, source identity, ordering, descriptor bounds, counts
  and completed scan length are validated on reuse. Unsupported or changed data
  fails; it never resets legacy builds or silently adopts a different source.
- `build.lock` uses an OS lock released on close/crash, held through Pass 2.
  Lock failures identify the working folder without assuming a stale lock file.
- `bind_build` binds compact completion/resume to output, feature and elevation
  settings. `finish` marks completion only after feature processing succeeds.
- Up to eight million descriptors are supported; decoded work is bounded by
  32 compressed blocks plus active decoders, independently of total planet size.

Resume now separates ephemeral device numbers from persistent identity evidence.
An optional JSON source_fingerprint upgrades existing schema-v1 databases without
recreating tables. On first upgrade, up to 64 spread node blocks must decode with
the saved lengths/counts/ID ranges. Future opens require the persisted sampled
SHA-256 signature. Only verified identity fields are updated; offsets, descriptors,
node counts, build settings, completion, and other scan databases are retained.
Metadata/content mismatches fail before writes to an existing checkpoint.
