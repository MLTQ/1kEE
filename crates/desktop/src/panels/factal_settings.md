# factal_settings.rs

## Purpose
Renders the app settings window for the desktop app. This file is the user-facing control surface for storing, clearing, and manually refreshing the Factal API key, configuring live camera-source keys and the opt-in Project Eyes On directory scope, and configuring asset and tool paths.

## Components

### `render_factal_settings`
- **Does**: Draws the Settings window, persists the configured Factal key, camera-source keys, bounded Project Eyes On opt-in/scope, and path overrides, triggers live refreshes, and reports failures to the activity log
- **Interacts with**: `AppModel` in `model.rs`, `settings_store.rs`, `factal_stream.rs`, `camera_registry.rs`, theme helpers in `theme.rs`
- **Rationale**: Keeps credential entry and machine-specific path configuration in one explicit place instead of scattering assumptions across startup fallbacks

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `app.rs` | Calling the renderer is enough to keep the settings window functional | Renaming or removing the render entrypoint |
| `header.rs` | The window opens when `factal_settings_open` is toggled on the model | Ignoring the open flag or relocating the window state |
| `settings_store.rs` | Saving empty path fields reverts to executable-directory defaults / PATH discovery and saving an empty Factal key clears the stored token | Changing blank-field semantics |

## Notes
- The key is masked in the UI but still stored as plain text because this is still a local demo build.
- 511NY and Windy Webcams keys are optional; leaving them blank still permits
  explicit Project Eyes On discovery or declarative no-key camera sources.
- No-key public camera sources can be declared in `Data/camera_sources/public_sources.json`, and curated scraped webcam-directory seeds can be declared in `Data/camera_sources/scrape_sources.json` under the asset root.
- Project Eyes On directory discovery is off by default and must be explicitly enabled; the UI caps it at five pages per poll and accepts an optional two-letter country scope.
- Fresh configurations default to all five bounded pages; the UI explains that
  a one-page scan contains only a handful of directory listings.
- Asset/data/derived/SRTM/planet/GDAL overrides are intentionally optional; leaving them blank means “use the executable folder defaults and PATH-based GDAL tools.”
