# header.rs

## Purpose
Renders the top operational banner for the desktop app. It gives the analyst immediate status context for the demo feeds and current focus area.

## Components

### `render_header`
- **Does**: Builds the top bar, exposes settings and asset controls, displays
  source status, and offers a one-click explicit opt-in when the camera registry
  is still in Demo mode.
- **Interacts with**: mutable `AppModel` in `model/mod.rs`,
  `camera_registry::invalidate`, terrain/OSM inventories, uploaded layers,
  file/folder pickers, and theme helpers.

### `metric_chip`
- **Does**: Draws a compact labeled status pill
- **Interacts with**: `render_header`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `app.rs` | Header rendering can mutate the model for root-selection changes while remaining the top-bar entrypoint | Changing the entrypoint signature materially |
| Camera registry | Enabling live cameras persists the opt-in and invalidates the registry immediately | Turning the button into an implicit startup fetch |
