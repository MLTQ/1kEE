# main.rs

## Purpose
Entrypoint for the desktop demo. It owns native window setup and hands control to the `DashboardApp`.

## Components

### `main`
- **Does**: Configures the `eframe` native window, compiles the desktop support modules, and launches the app
- **Interacts with**: `DashboardApp` in `app.rs`, module declarations for terrain, city catalog, settings persistence, and live Factal polling
- **Rationale**: Keeps platform/bootstrap concerns separate from UI state and rendering

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `cargo run` | Native app boot succeeds through `eframe::run_native` | Changing app bootstrap signature or crate target type |
