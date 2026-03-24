# main.rs

## Purpose
Entry point for the offline cache-builder companion binary. It keeps the CLI surface small and delegates real work to focused modules so the desktop app can eventually stop parsing `planet.osm.pbf` interactively.

## Components

### `main`
- **Does**: Runs the CLI and exits non-zero on failure
- **Interacts with**: `run`

### `run`
- **Does**: Parses CLI arguments and dispatches the selected offline cache-building command
- **Interacts with**: `args.rs`, `roads.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| shell users / scripts | non-zero exit on failure and stderr error text | Silently swallowing failures |
| future automation | subcommands dispatch through `args::Command` | Removing the command enum |

## Notes
- The first implemented command is intentionally focused: build road-cell GeoJSON caches for a requested bbox.
