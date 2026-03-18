# camera_registry.rs

## Purpose
Runs the live camera registry polling loop for the desktop app. This module owns provider-specific adapters and normalizes them into the shared `CameraFeed` model so the rest of the UI can stay source-agnostic.

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

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `app.rs` | `tick` is cheap enough to call every frame and only mutates the model when a finished poll result exists | Making `tick` blocking or removing it |
| `model.rs` | Successful polls arrive as normalized `CameraFeed` values and can replace the current camera registry atomically | Returning provider-specific camera types without normalization |
| `factal_settings.rs` | `invalidate` triggers a fresh camera sync after camera-source settings change | Removing the invalidation hook |

## Notes
- The registry currently supports a concrete 511NY adapter and a best-effort Windy Webcams adapter.
- The registry now also supports declarative no-key public sources loaded from `Data/camera_sources/public_sources.json` under the asset root.
- The registry also supports curated scraped webcam-directory seeds loaded from `Data/camera_sources/scrape_sources.json` under the asset root.
- Generic no-key adapters currently support three shapes: plain JSON arrays, GeoJSON feature collections, and ArcGIS feature service query responses.
- Curated scrape adapters intentionally rely on operator-supplied coordinates and only do lightweight HTML extraction for titles and embed/page URLs.
- The app stays in demo camera mode until at least one camera-source key is configured.
- This is intentionally a metadata registry sync, not a live video probe; stream URLs are passed through for later connection attempts.
