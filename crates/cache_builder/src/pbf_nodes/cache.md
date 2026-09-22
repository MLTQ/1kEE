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
