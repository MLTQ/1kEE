# app.rs

## Purpose
Coordinates the desktop app at the highest level. It owns the root `AppModel`, installs visuals, and composes the major UI panels.

## Components

### `DashboardApp`
- **Does**: Stores shared UI state for the running app
- **Interacts with**: `AppModel` in `model.rs`, panel renderers in `panels/`

### `DashboardApp::new`
- **Does**: Installs theme configuration and seeds the demo model
- **Interacts with**: `install` in `theme.rs`, `AppModel::seed_demo` in `model.rs`

### `eframe::App::update`
- **Does**: Lays out the shell around header, sidebars, status log, and map canvas
- **Interacts with**: `render_*` functions in `panels/mod.rs`
- **Rationale**: Keeps app composition separate from the details of each view

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `main.rs` | `DashboardApp::new` returns a ready-to-render app | Constructor signature |
| `panels/*` | Shared state lives in `AppModel` and remains mutable here | Moving state ownership elsewhere |
