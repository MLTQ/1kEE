# main.rs

## Purpose
Entry point for the offline cache-builder companion binary. It keeps the CLI surface small and delegates real work to focused modules so the desktop app can eventually stop parsing `planet.osm.pbf` interactively.

## Components

### `main`
- **Does**: Runs the CLI and exits non-zero on failure
- **Interacts with**: `run`

### `run`
- **Does**: Parses CLI arguments and dispatches either the GUI or the selected offline cache-building command
- **Interacts with**: `args.rs`, `roads.rs`, `app.rs`

### `launch_gui`
- **Does**: Starts the minimal egui cache-builder companion app
- **Interacts with**: `app.rs`, `eframe`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| shell users / scripts | non-zero exit on failure and stderr error text for CLI runs | Silently swallowing failures |
| future automation | subcommands dispatch through `args::Command` | Removing the command enum |

## Notes
- No-arg launch now opens the GUI by default so the builder behaves like a companion desktop tool when launched directly.
- The first implemented CLI command remains focused: build road-cell GeoJSON caches for a requested bbox.
