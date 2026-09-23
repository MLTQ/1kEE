# road_import.rs

## Purpose
Moves automatic focused-road queue checks and inventory discovery off the UI
thread. They can open SQLite, ensure schemas and inspect the external drive.

## Contracts
- One scheduling worker at a time, with focus and viewport requests submitted
  together so one focus does not starve the other. Existing job deduplication
  and selected-source behavior remain in `osm_ingest`.
- The UI only polls a channel and applies completed messages/inventory. Results
  from a previous asset root cannot replace the current root's inventory.
- Disconnect/spawn failure releases the gate for the next rate-limited attempt.
- Road visibility changes do not cancel already-authorized queued imports.
