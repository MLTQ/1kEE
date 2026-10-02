# globe_residency.rs

## Purpose
Retain recently used Earth globe terrain under a memory budget. Leaving the
viewport no longer destroys geometry that may be used again on the next pan.

## Components
- `Viewport` tests a conservative sphere around each geographic patch against
  the actual globe projection's screen, hemisphere and near clipping planes.
  Curved edges and date-line crossings cannot disappear from corner-only tests.
- `Residency` tracks projected visibility, reader-measured bounds/byte costs,
  last-use timestamps and viewport revisions. Late new reads must still be
  visible, so obsolete navigation cannot fill the retained cache.
- `prune` keeps offscreen sources until the 3 GiB terrain instance target or
  4 GiB decoded CPU target is exceeded, then evicts least recently used
  offscreen sources. Returning to a tile refreshes its last use. A 4096-entry
  guard bounds empty-tile metadata. Visible coverage takes priority over these
  targets; this is a residency policy, not the device's single-buffer limit.
- `gpu_bytes` uses the actual 28-byte instance layout; composition reports
  output/fallback bytes to account for clipping expansion and reserve outgoing
  LOD. Bounds and byte measurements happen on reader/merge workers.
- `source_spec` tries a 5–7-ring fine grid, then the coarser grid if the screen
  extends past it. Requests stay at most 225 cells; viewport checks filter reads.

## Contracts
Paint tests small metadata only; camera-only movement never dirties geometry.
Eviction ordering is computed only when the retention targets are exceeded.
Composition includes all retained sources, so there is no artificial clipping
at the current request window. Legacy halos still partition without overlap.
Inactive bodies/data roots reset as before. Lunar/Mars readers do not accumulate
Earth residency metadata. Invalid transforms conservatively preserve coverage.
