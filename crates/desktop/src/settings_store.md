# settings_store.rs

## Purpose
Persists the desktop app's local configuration so it survives restarts. That now includes the Factal API key, live camera-source keys, the opt-in Project Eyes On directory scope, path overrides for the asset root, data roots, GDAL tool discovery, and the operator-selected contour line thickness.

## Components

### `AppSettings`
- **Does**: Holds the app-managed settings payload for Factal, live camera sources, bounded Project Eyes On directory settings, filesystem/tool paths, a legacy contour multiplier, and an optional physical contour width
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
| `camera_registry.rs` | Project Eyes On is disabled by default; country codes are blank or ISO-like and page counts stay within `1..=5` | Enabling network discovery by default or removing bounds |
| Map renderers | Older files retain their multiplier-derived visual width when it is already visible; new physical widths stay within the 1–16 px range | Dropping the legacy fallback or allowing invalid pixel widths through normalization |

## Notes
- The settings file now lives beside the executable so moving the app bundle/worktree to another machine keeps the local path model coherent by default.
- The Factal key and camera-source keys are still stored as plain text because this is a local demo, not a hardened credential store.
- GDAL discovery now prefers the app-configured bin directory and otherwise relies on `PATH`; it no longer assumes Postgres.app.
- Path settings are now normalized on save/load so operators can point at a parent folder like `/Volumes/BigDisk/Data` and still have the app infer nested `Data/`, `Derived/`, or `srtm_gl1/SRTM_GL1_srtm` subpaths when those exist.
- If `Asset Root` is accidentally pointed at a `Data/` or `Derived/` folder, it is normalized back to the parent asset root to avoid silently creating `Data/Data` or `Data/Derived` layouts.
- `contour_stroke_scale` remains as a compatibility field for older files.
  `contour_stroke_width_px` is optional: when absent, the legacy multiplier is
  resolved at the current display density so upgrading preserves existing
  visible output. Older sub-pixel values are raised to the new 1 px floor.
  Once an operator moves the new control, its physical width is persisted in
  the visible `1..=16 px` range.
