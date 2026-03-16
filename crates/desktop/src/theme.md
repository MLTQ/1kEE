# theme.rs

## Purpose
Centralizes the initial visual language for the 1kEE desktop demo. It provides app-wide style installation and a few reusable color helpers for panels.

## Components

### `install`
- **Does**: Applies spacing, backgrounds, and interactive widget colors to the `egui` context
- **Interacts with**: `DashboardApp::new` in `app.rs`

### Color helper functions
- **Does**: Provide consistent panel, grid, camera, muted-text, and globe/HUD accent colors
- **Interacts with**: panel renderers in `panels/`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `app.rs` | `install` mutates the `egui::Context` safely during startup | Removing the function or changing its side effects materially |
| `panels/*` | Helper colors remain stable semantic tokens, including the globe wireframe palette | Renaming or removing helper functions |
