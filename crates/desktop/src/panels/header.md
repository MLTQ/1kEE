# header.rs

## Purpose
Renders the top operational banner for the desktop app. It gives the analyst immediate status context for the demo feeds and current focus area.

## Components

### `render_header`
- **Does**: Builds the top bar, exposes the data-root picker, and displays stream, registry, terrain, selection summaries, and the resolved SRTM root when one is auto-detected
- **Interacts with**: mutable `AppModel` in `model.rs`, `TerrainInventory` via `AppModel`, `terrain_assets.rs`, `rfd` folder picker, theme helpers in `theme.rs`

### `metric_chip`
- **Does**: Draws a compact labeled status pill
- **Interacts with**: `render_header`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `app.rs` | Header rendering can mutate the model for root-selection changes while remaining the top-bar entrypoint | Changing the entrypoint signature materially |
