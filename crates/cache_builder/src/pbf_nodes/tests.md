# tests.rs

## Purpose
Exercises compact lookup correctness and durable indexing independently of user
data, using committed plain/dense fixtures and generated multi-block PBFs.

## Coverage
- Exact flat-store coordinate parity, duplicate/shared/missing references and
  cache reuse; bounded-cache eviction/reread and physical I/O failure propagation.
- OS build locking, completed-index reuse, settings binding and completion.
- Interruption after a committed scan batch followed by exact restart without
  duplicate descriptors; changed sources preserve existing state and fail.
- Reversed and duplicate node IDs reject unsupported input explicitly.
- A corrupted persisted descriptor is rejected before it can hide coordinates.
- Grouped way dependencies survive LRU eviction, cache missing IDs, reset between
  blobs, and fall back correctly for oversized dependency lists/unprepared IDs.
- Test-only source/index files live in unique owned temporary directories.
