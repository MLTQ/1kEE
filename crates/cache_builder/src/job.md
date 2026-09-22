# job.rs

## Purpose
Runs cache-builder work off the UI thread and streams progress back to the egui app. This keeps the builder responsive while large planet scans are running.

## Components

### `BuildJob`
- **Does**: Enumerates the background build tasks the GUI can launch
- **Interacts with**: `app.rs`, `roads.rs`

### `BuildEvent`
- **Does**: Carries progress and completion state from the worker thread back to the GUI
- **Interacts with**: `app.rs`

### `spawn_job`
- **Does**: Launches a background worker for the selected build job and returns the event receiver
- **Interacts with**: `roads.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `app.rs` | receives `Progress` updates followed by one terminal `Finished` event | Removing event ordering or changing the event types |

## Notes
- Vector, planetary terrain, and archive jobs share this event stream.

- `PackArchive` streams completed-cell log entries and one Finished result. Packing is a background snapshot job; the UI does not invent a percentage while enumerating source files.
