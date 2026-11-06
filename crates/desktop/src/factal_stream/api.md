# api.rs

## Purpose
Load complete, time-bounded Factal result sets instead of treating the API's
50-item first page as the entire live feed. Share pagination with replay history.

## Components
- `fetch_live_events` collects all pages for severity ≥2 in the past 24 hours;
  returns a snapshot only after every page succeeds.
- `fetch_events_since` streams normalized pages to a live or history consumer,
  with a 20-second request timeout and 600 ms spacing between pages.
- `walk_pages` follows absolute or relative `next` links until exhausted,
  deduplicates IDs across the entire run, and counts raw items/pages/unique events.
- `Page` requires a `results` array: malformed payloads fail rather than silently
  clearing the current feed. `FetchError` distinguishes auth and rate limits.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| `factal_stream.rs` | Successful snapshots contain the complete live range | Publishing partial pages or changing the window silently |
| `history.rs` | Each completed page can be persisted; errors stop traversal | Removing the page callback |
| `api_tests.rs` | Page traversal accepts an injected transport | Coupling traversal to real API credentials |

## Notes
- `date__gte` uses an exact UTC timestamp; the live window is explicitly 24 hours.
- Pagination never forwards the token outside the original origin/item path;
  HTTP redirects are disabled and repeated page URLs fail explicitly.
- Live rate limits return to the caller for its normal cooldown. Replay preserves
  its existing 60-second backoff and retries the same page before continuing.
