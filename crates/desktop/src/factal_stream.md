# factal_stream.rs

## Purpose
Runs the live Factal event polling loop for the desktop app. This file translates the private API shape inferred from the user's historical collector into normalized `EventRecord` values that the UI can render.

## Components

### `tick`
- **Does**: Advances the background polling lifecycle, joins finished work, applies fresh events to the `AppModel`, and spawns the next minute poll when needed
- **Interacts with**: `AppModel` in `model.rs`, `fetch_latest_events`
- **Rationale**: Keeps the network call off the UI thread while still letting the app poll on a predictable cadence

### `invalidate`
- **Does**: Forces the next app tick to treat the current Factal configuration as stale and poll again immediately
- **Interacts with**: `factal_settings.rs`

### `shutdown`
- **Does**: Marks the poll manager as shut down so the app stops launching new Factal workers during teardown
- **Interacts with**: `app.rs`

### `fetch_latest_events`
- **Does**: Calls Factal `GET /api/v2/item/?severity__gte=2` with `Authorization: Token ...`, parses the JSON payload, and extracts geolocated events
- **Interacts with**: `reqwest`, `serde_json`, `parse_event`
- **Rationale**: Uses only fields proven by the attached legacy Python collector instead of guessing undocumented API behavior

### `parse_event`
- **Does**: Maps one raw Factal item into a normalized `EventRecord` while preserving a richer Factal-only payload for brief/detail inspection
- **Interacts with**: `EventRecord`, `EventSeverity`, `GeoPoint` in `model.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `app.rs` | `tick` is cheap enough to call every frame and only mutates the model when a poll result is ready | Making `tick` blocking or removing the function |
| `factal_settings.rs` | `invalidate` triggers a fresh poll after saving or clearing a key | Removing the invalidation hook |
| `model.rs` | Parsed events arrive as `EventRecord` values with stable ids and valid coordinates | Changing id construction or returning ungeolocated records |

## Notes
- This first pass only consumes the latest page of events because that is the lowest-risk interpretation of the private API and keeps polling lightweight.
- The app ignores stale results when the key changes while a request is still in flight.
- In addition to headline/summary/location fields, the parser now preserves the raw pretty-printed JSON item, vertical/subvertical topic tags, topic names, point WKT, and numeric severity when Factal provides them.
