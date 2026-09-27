# flock_tsv.rs

## Purpose
Parse the public camera TSV into compact map positions using the standard CSV
reader in tab-separated mode. Quoted tabs/newlines, CRLF and reordered columns
are handled without misaligning geographic fields.

## Contracts
- Requires latitude, longitude, OBJECTID, active and status columns.
- Rejects structurally malformed, oversized (>128 MiB / one million rows), or
  empty inventories so a failed reload cannot replace useful data.
- Invalid/nonfinite coordinates, `(0,0)` placeholders, invalid identifiers and
  repeated OBJECTIDs are counted and skipped. Co-located distinct IDs remain.
- In-service means both `active=1` and `status=inService`; it is a snapshot
  classification, not confirmation of current operation.
- No source metadata is written back or discarded from the original TSV.

Tests cover quoting, coordinate order, service states, duplicates, missing
positions and malformed files. An ignored integration test loads the full file
specified by `ONEKEE_FLOCK_TSV` through the actual production parser.
