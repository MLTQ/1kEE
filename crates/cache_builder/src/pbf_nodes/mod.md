# mod.rs

## Purpose
Resolves node coordinates directly from an immutable original PBF. Replaces the
expanded node file only when the user selects compact lookup.

## Contracts
- Same i64 IDs and f32 latitude/longitude conversion as the flat store.
- Node IDs must be strictly increasing across all node blocks. Unordered/history
  inputs fail clearly; the explicit flat backend remains available.
- Source device/inode/length/mtime are checked on index reuse and around Pass 2
  batches. Deliberately changing bytes while restoring metadata is out of scope.
- Index descriptors and decoded counts/ranges are validated before use. Encoded
  blocks are bounded to 33 MiB; decoded blocks to two million nodes.
- `PbfNodes` shares positional read-only I/O; `Session` owns its bounded cache.
- `BuildState` holds the separate compact-directory lock for the whole build.
