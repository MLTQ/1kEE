# history.rs

## Purpose
Own the explicit replay backfill worker independently of the minute-based live
poll lifecycle. Use the same validated pagination and ID deduplication as live.

## Components
- `trigger_history_fetch` starts at most one backfill for the requested day range
  and persists each completed page to the existing event store.
- `history_tick` joins completed work, reports completion or error and rebuilds
  an open replay after success.
- `is_history_fetching` reports whether a handle is still pending consumption.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| `factal_stream.rs` | Existing public replay entry points are re-exported | Signature changes |
| `model/mod.rs` | Replay alone requests backfills; repeated frames never launch extras | Triggering history every minute |
| `api.rs` | Receives a cutoff and page callback; counts unique records per run | Counting page duplicates as new records |

## Notes
Completed pages remain cached after an error. History preserves its existing
60-second rate-limit backoff and retries the same page. It does not restart a
completed page traversal when a single request is rate limited.
