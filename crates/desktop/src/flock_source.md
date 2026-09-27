# flock_source.rs

## Purpose
Loads the user-downloaded public Flock inventory from `Data/Flock/cameras.tsv`
without replacing or mislabelling the independent DeFlock/OpenStreetMap source.

## Components
- `Source::tick` resolves the configured data root, starts one local reader,
  and applies completed immutable snapshots without blocking a frame.
- `reload` schedules another read; failure retains the last usable snapshot.
  Changing roots clears the old snapshot and discards old worker results.
- `visible_positions` shows active, in-service records by default; an explicit
  toggle includes planned, decommissioned and otherwise inactive records.

## Contracts
The original TSV retains all metadata. The compact render snapshot stores only
validated positions and service eligibility; it does not invent OSM identities
or claim every device is an ALPR camera. Revision and filter state invalidate
the shared globe/local marker meshes. No automatic HTTP requests or live feeds.
Hidden inventories are skipped before iteration rather than scanned per marker.
The local layer defaults on, remains empty until its file loads, and supports
manual reload after the file is replaced. Reload work runs only in a worker.

`rotationAngle` is retained in the original file but not rendered as a compass
bearing: its orientation convention has not been established.

Lifecycle tests verify that malformed reloads retain the displayed snapshot and
that changing the data root removes old positions, even if the new file is missing.
