# terrain_assets.rs

## Purpose
Detects the locally available terrain datasets for the app, including repository-local assets and the external-drive SRTM mirror. It gives the UI a simple inventory/status layer without forcing the app to parse large rasters at startup.

## Components

### `TerrainInventory`
- **Does**: Summarizes which GEBCO, Natural Earth, SRTM, and derived runtime terrain assets are present and which source should drive runtime terrain work first
- **Interacts with**: `AppModel` in `model.rs`, `header.rs`

### `TerrainInventory::detect_from`
- **Does**: Scans the selected root, its parents, and the local workspace for known raw and derived terrain asset locations
- **Interacts with**: local filesystem
- **Rationale**: Keeps asset discovery lightweight until a preprocessing pipeline produces app-specific runtime assets

### `find_data_root`
- **Does**: Resolves the terrain data root from explicit app settings first and otherwise falls back to `Data/` or `data/` under the selected asset root / executable directory
- **Interacts with**: `settings_store.rs`, filesystem

### `find_derived_root`
- **Does**: Resolves the derived-asset root from explicit app settings first and otherwise falls back to `Derived/` under the selected asset root / executable directory
- **Interacts with**: `settings_store.rs`, filesystem

### `find_srtm_root`
- **Does**: Resolves an SRTM GL1 tile root from explicit app settings first and otherwise searches the selected asset root / executable directory
- **Interacts with**: `settings_store.rs`, filesystem

### Status helpers
- **Does**: Produce compact status labels and user-facing summary lines
- **Interacts with**: operator log and header metrics

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `model.rs` | `detect_from` is cheap and safe to call during startup and after user root changes | Making detection expensive or fallible in a way that crashes startup |
| `header.rs` | `status_label` and `status_summary` return concise human-readable strings | Renaming/removing status helpers |

## Notes
- This module intentionally stops at inventory. Real terrain ingestion should consume preprocessed outputs rather than opening the raw source rasters directly from the UI thread.
- The current preferred runtime path is streamed SRTM land tiles with GEBCO fallback for everywhere else.
- The executable directory is now the default asset root, and the app will create `Data/` and `Derived/` there if they do not already exist.
- External-drive assumptions have been removed from automatic discovery; anything outside the asset root should now be configured explicitly in the app settings UI.
- Configured path overrides are now forgiving: if the operator points `Data Root` or `Derived Root` at a parent folder, discovery will still prefer the nested `Data/` or `Derived/` child when present, and SRTM root resolution now searches for `srtm_gl1/SRTM_GL1_srtm` under that configured folder instead of treating the parent as the tile root itself.
