# settings_store.rs

## Purpose
Persists the desktop app's local configuration so it survives restarts. That now includes the Factal API key, live camera-source keys, and path overrides for the asset root, data roots, and GDAL tool discovery.

## Components

### `AppSettings`
- **Does**: Holds the app-managed settings payload for Factal, live camera sources, and filesystem/tool paths
- **Interacts with**: `model.rs`, `terrain_assets.rs`, `osm_ingest.rs`, `srtm_focus_cache.rs`, `factal_settings.rs`, `camera_registry.rs`

### `load_app_settings` / `save_app_settings`
- **Does**: Reads and writes the full settings JSON from the executable directory
- **Interacts with**: `model.rs`, `factal_settings.rs`

### `effective_asset_root`
- **Does**: Resolves the current asset root, falling back to the executable directory when no override is saved
- **Interacts with**: `model.rs`, `terrain_assets.rs`, `header.rs`

### `ensure_default_asset_layout`
- **Does**: Creates `Data/` and `Derived/` under the effective asset root if they do not already exist
- **Interacts with**: `model.rs`

### `resolve_gdal_tool`
- **Does**: Resolves `ogr2ogr`, `gdalwarp`, and `gdal_contour` from the configured GDAL bin directory or leaves them on `PATH`
- **Interacts with**: `osm_ingest.rs`, `srtm_focus_cache.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `model.rs` | Settings loading is cheap enough to use at startup and returns executable-directory defaults when unset | Making settings resolution expensive or removing the default asset-root fallback |
| `factal_settings.rs` | Saving an empty key clears the on-disk value and blank path fields revert to auto-detect/default behavior | Changing clear semantics or making blank path fields invalid |

## Notes
- The settings file now lives beside the executable so moving the app bundle/worktree to another machine keeps the local path model coherent by default.
- The Factal key and camera-source keys are still stored as plain text because this is a local demo, not a hardened credential store.
- GDAL discovery now prefers the app-configured bin directory and otherwise relies on `PATH`; it no longer assumes Postgres.app.
- Path settings are now normalized on save/load so operators can point at a parent folder like `/Volumes/Hilbert/Data` and still have the app infer nested `Data/`, `Derived/`, or `srtm_gl1/SRTM_GL1_srtm` subpaths when those exist.
- If `Asset Root` is accidentally pointed at a `Data/` or `Derived/` folder, it is normalized back to the parent asset root to avoid silently creating `Data/Data` or `Data/Derived` layouts.
