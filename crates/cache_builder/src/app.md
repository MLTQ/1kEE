# app.rs

## Purpose
Provides the minimal egui desktop shell for the offline cache-builder. It is the user-facing companion app for launching background cache jobs, selecting export assets, and inspecting generated cache files.

## Components

### `BuilderApp`
- **Does**: Owns the form state, asset toggles, progress display, background job handle, and cache inspector
- **Interacts with**: `job.rs`, `args.rs`

### `BuilderApp::start_build`
- **Does**: Validates the current form, starts a background build worker, and resets the UI progress state
- **Interacts with**: `job.rs`

### `BuilderApp::poll_job`
- **Does**: Consumes background progress/log/result events and updates the visible status/progress bar
- **Interacts with**: `job.rs`

### `BuilderApp::refresh_inspector`
- **Does**: Scans the selected cache directory and summarizes the generated road cell files
- **Interacts with**: filesystem

### Mars cache form

- **Does**: Defaults Mars builds and folder-picker selections to
  `mars_ctx_cache.sqlite`, the file consumed by the desktop renderer.
- **Interacts with**: `mars.rs` and the desktop Mars terrain cache contract.

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `main.rs` | can construct `BuilderApp::new()` and hand it to `eframe` | Changing construction semantics |
| users | no-arg launch opens a GUI instead of exiting with CLI usage | Removing GUI default launch |

## Notes
- Roads are the only implemented export asset today. Water, buildings, and boundaries are present as disabled planned toggles so the intended builder shape is visible immediately.
- The inspector intentionally stays lightweight: it reports road-cell count, node-cache count, approximate cache size, and the most recently touched files so users can sanity-check output directories without opening another tool.
- Mars defaults intentionally do not rename, merge, or import legacy
  `mars_focus_cache.sqlite` files; operators can still choose a custom path
  explicitly when they need to inspect an old cache.
