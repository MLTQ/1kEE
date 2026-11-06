# factal_stream.rs

## Purpose
Run one non-blocking live Factal sync at a time and publish complete snapshots to
the desktop model. Pagination, item parsing and replay history have focused
modules under `factal_stream/`.

## Components
- `tick` joins finished work and launches the next eligible worker. Normal polls
  wait 60 seconds after completion, including failures, so a slow multi-page sync
  cannot cause back-to-back polling. Worker handles are installed under the lock.
- `invalidate` requests an immediate refresh. An active worker finishes first;
  its obsolete generation cannot overwrite the model or delay the queued refresh.
- `shutdown` prevents additional live workers and invalidates in-flight results.
- `apply_outcome` retains the previous complete snapshot on auth/rate-limit,
  malformed-response or later-page errors.
- `apply_snapshot` persists and replaces the Factal subset while preserving USGS
  records. Activity reports new/updated/removed IDs, current geolocated total, raw
  item count and page count; unchanged successful refreshes are silent.
- Re-exports preserve the existing replay entry points from `history.rs`.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| `app.rs` | `tick` and `history_tick` are cheap per-frame calls | Blocking the UI thread |
| `settings_actions.rs` | `invalidate` queues a fresh poll after configuration changes | Dropping a requested refresh |
| `model/mod.rs` | Replay entry points retain their signatures | Changing replay integration |
| `api.rs` | Only complete successful live snapshots reach the model | Publishing partial pages |

## Notes
- The live range is severity ≥2 over the past 24 hours, with every `next` page
  loaded; the 50-item API page size is never treated as the feed size.
- Log comparisons use only Factal IDs/payloads, independently of earthquake
  counts. Same-size sets containing different events still produce an update.
- No configured key means inactive; no synthetic events are substituted.
- Tests cover mixed-source log deduplication, replacements/revisions, stale/error
  preservation, one active worker and completion-based scheduling.
