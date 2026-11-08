# cache.rs

## Purpose
Groups coordinate requests by compressed PBF block and reuses recently decoded
blocks in a worker-local cache. No decoded nodes are written to disk.

## Contracts
- Results align with input IDs, including duplicates and missing IDs.
- Cache retains at most 16 MiB of node vector capacity per active session, plus
  map bookkeeping. One oversized decoded block can be used transiently uncached.
- Least-recently-used blocks are discarded; the original PBF remains available
  for rereading them. Failed reads never enter the cache.
- Separate Rayon sessions have no shared cache mutex or mutable file cursor.
- `prefetch` groups all references in one way blob before individual feature
  lookups, decoding each needed source block at most once even with LRU eviction.
  Stores sorted optional coordinates (including missing IDs) for at most 524,288
  references, at most 12 MiB additional retained data on 64-bit targets. Larger
  blobs fall back to LRU-only lookup. Unprefetched IDs still use normal lookup.
  Temporary request/result vectors add bounded memory during preparation.

## Shared planet-way preparation
`prepare_batch` sorts/deduplicates selected IDs across a bounded group of way
blobs. It reads each required node block once in parallel, retaining only the
requested coordinates in an immutable `Arc<Prepared>`. Worker sessions search
this table without locks. Missing IDs are retained too, preventing rereads.
Legacy callers still use the bounded LRU described above. Decode errors abort
preparation; no partial table is published. Activity counters include these reads.
