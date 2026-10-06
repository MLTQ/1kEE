# layer_snapshot.rs

## Purpose
Keep layer source discovery, disk access and decoding off the drawing thread.

## Contracts
`Snapshot::get` polls a channel and starts at most one worker. A blocked worker
never holds the UI cache mutex. Keys coalesce to the latest request; only the
matching key and reset epoch can publish. Compatible prior snapshots remain
visible during refresh. Failure/panic backs off one second; workers wake the app.
`clear` rejects late publication while retaining the single-flight gate.
The channel-based regression holds a worker until explicitly released and
verifies repeated non-blocking polls and stale-result rejection.
