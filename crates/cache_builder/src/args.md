# args.rs

## Purpose
Parses the cache-builder CLI without pulling in a heavier argument framework yet. It defines the initial offline command surface for focused road cache generation.

## Components

### `Command`
- **Does**: Enumerates supported offline build commands
- **Interacts with**: `main.rs`, `roads.rs`

### `RoadsBboxCommand`
- **Does**: Holds the required planet source, cache output directory, and bbox for focused road-cell generation
- **Interacts with**: `roads.rs`

### `parse`
- **Does**: Parses the top-level subcommand and dispatches to a command-specific parser
- **Interacts with**: `parse_roads_bbox`

### `parse_roads_bbox`
- **Does**: Validates the focused-road bbox CLI flags
- **Interacts with**: `RoadsBboxCommand`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `main.rs` | returns `Command::Gui` when launched without arguments and a readable usage/error string for invalid CLI input | Changing the no-arg behavior or return contract |
| users / scripts | stable `roads-bbox` flag names | Renaming flags without migration |

## Notes
- This is intentionally simple for the first slice. If the cache-builder grows more commands, switching to `clap` will probably be worth it.
- The parser now treats no-arg launch as a GUI request so the compiled binary can behave like a normal desktop companion app.
