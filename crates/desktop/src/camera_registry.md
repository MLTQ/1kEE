# camera_registry.rs

## Purpose
Runs the live camera registry polling loop for the desktop app. This module owns provider-specific adapters, including the opt-in Project Eyes On directory pipeline, and normalizes them into the shared `CameraFeed` model so the rest of the UI can stay source-agnostic.

## Components

### `tick`
- **Does**: Advances the background polling lifecycle, joins finished work, applies fresh camera records to the `AppModel`, and spawns the next provider poll when needed
- **Interacts with**: `AppModel` in `model.rs`, `fetch_camera_registry`
- **Rationale**: Keeps network fetches off the UI thread while letting the app refresh camera metadata around the current focus

### `invalidate`
- **Does**: Forces the next app tick to treat the current camera-source configuration and focus location as stale and refresh again immediately
- **Interacts with**: `factal_settings.rs`

### `fetch_511ny_cameras`
- **Does**: Calls the official 511NY cameras endpoint and maps statewide camera metadata into normalized `CameraFeed` records
- **Interacts with**: `reqwest`, `serde_json`
- **Rationale**: 511NY exposes camera ids, roadway names, and precise coordinates directly, so it is a strong first high-confidence source

### `fetch_windy_cameras`
- **Does**: Calls the Windy Webcams API for a bbox around the current terrain focus and maps returned webcams into normalized `CameraFeed` records
- **Interacts with**: `reqwest`, `serde_json`
- **Rationale**: Windy gives much broader geographic coverage than traffic-only feeds, but the query is intentionally focus-bounded so the first implementation stays cheap

### Project Eyes On directory adapter

- **Does**: Invokes the bounded Insecam discovery, detail-page geolocation, feed
  classification, and deduplication pipeline when explicitly enabled.
- **Interacts with**: `camera_directory_pipeline.rs` and persisted camera settings.
- **Rationale**: Keeps the Project Eyes On integration inside the same
  non-blocking registry worker and source-normalization boundary as keyed feeds.

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `app.rs` | `tick` is cheap enough to call every frame and only mutates the model when a finished poll result exists | Making `tick` blocking or removing it |
| `model.rs` | Successful polls arrive as normalized `CameraFeed` values and can replace the current camera registry atomically | Returning provider-specific camera types without normalization |
| `factal_settings.rs` | `invalidate` triggers a fresh camera sync after camera-source settings change | Removing the invalidation hook |

## Notes
- The registry currently supports a concrete 511NY adapter and a best-effort Windy Webcams adapter.
- Project Eyes On directory discovery is an explicit opt-in and does not include the upstream search-engine dorking path.
- The registry now also supports declarative no-key public sources loaded from `Data/camera_sources/public_sources.json` under the asset root.
- The registry also supports curated scraped webcam-directory seeds loaded from `Data/camera_sources/scrape_sources.json` under the asset root.
- Generic no-key adapters currently support three shapes: plain JSON arrays, GeoJSON feature collections, and ArcGIS feature service query responses.
- Curated scrape adapters still prefer operator-supplied coordinates, but they can now fall back to lightweight embedded-map coordinate extraction for pages that expose stable map URLs.
- The app stays in demo camera mode until a keyed adapter, declarative public
  source, curated scrape seed, or the explicit Project Eyes On opt-in is active.
- Registry adapters remain metadata-oriented. The Project Eyes On adapter adds
  a bounded header/reachability probe; actual snapshot/MJPEG reading starts
  only after a user opens a pip in `camera_feed_viewer.rs`.
