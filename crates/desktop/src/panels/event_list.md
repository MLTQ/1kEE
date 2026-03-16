# event_list.rs

## Purpose
Shows the currently loaded event queue and lets the analyst choose which incident is driving map focus and camera lookup.

## Components

### `render_event_list`
- **Does**: Renders event cards with severity, timing, source, summary, and selection controls
- **Interacts with**: `AppModel::select_event` in `model.rs`, theme helpers in `theme.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `app.rs` | Function renders the left sidebar and can mutate event selection | Changing the entrypoint signature or removing selection behavior |
