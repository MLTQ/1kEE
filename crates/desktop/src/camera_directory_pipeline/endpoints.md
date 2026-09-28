# endpoints.rs

## Purpose
Persist previously verified directory endpoints beside app settings in
`.1kee_camera_endpoints.sqlite`. Startup and periodic polling reuse this data
without network discovery or automatic expiry.

## Components
- `Store` serializes SQLite access, with transactions and a cross-connection
  busy timeout. Each country/global scope has independent availability and actions.
- `plan` chooses cached results, first-time discovery, or an explicit pending action.
- `record` checkpoints reachable endpoints incrementally. Failed rechecks mark
  existing entries for review; unverified discoveries are never added.
- `request` queues discovery or rechecking durably without deleting working data.
- `remove` forgets one endpoint or failed entries; even an empty cache stays valid.
- `complete` acknowledges only the current revision's finished job.

## Contracts
- All store operations run on registry/settings workers, never paint.
- Every mutation increments or checks a scope revision, so an old scan cannot
  restore a removed endpoint or acknowledge a newer refresh request.
- Persist only endpoint metadata, coordinates and check times, not media or keys.
- Cached records retain public-target and coordinate validation. Historical
  success becomes `Idle`, with verification age, rather than claiming a live check.
- Cache errors are surfaced; they do not silently trigger a replacement crawl.
- No age-based directory refresh. Explicit refresh retains last-known endpoints
  until replaced or selectively removed.

When a directory camera advertises a verified replacement URL, its older URL is
removed atomically. Selective removal returns the URLs actually deleted within
the transaction, keeping UI updates consistent with concurrent check results.
