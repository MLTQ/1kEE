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
- **Does**: Walks up from the current working directory until it finds the repository `Data/` directory
- **Interacts with**: process current directory, filesystem
- **Rationale**: The desktop app may be launched from the workspace root or the crate directory, so detection cannot assume a fixed cwd or a single directory-case convention

### `find_derived_root`
- **Does**: Walks up from the current working directory until it finds the repository `Derived/` directory
- **Interacts with**: process current directory, filesystem
- **Rationale**: Runtime readiness should reflect generated terrain assets, not just raw source downloads

### `find_srtm_root`
- **Does**: Resolves an SRTM GL1 tile root from the selected folder, its ancestors, or known external-drive locations
- **Interacts with**: process current directory, filesystem, external mounted volumes
- **Rationale**: The high-resolution land dataset lives outside the repo and should still be usable without manual copying

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
- Repository-local `Data/` and `Derived/` assets are also resolved from the compiled workspace root so launching the app from a different cwd does not hide them.
