# event_camera_sidebar.rs

## Purpose
Shows the current Follow Events stop's nearest camera feeds in a resizable,
scrollable column on the right, including OBS view. It reserves actual panel
space so the map, event labels and chyron lay out beside the feeds.

## Components

### `EventCameraSidebar`
- Owns at most four independent 640-pixel `CameraFeedPlayer`s outside `AppModel`.
- Recomputes selection only when event coordinates or camera-registry revision
  change, without invalidating the map's nearby-camera snapshot.
- Keeps overlapping connections across tour stops. Removed cameras and leaving
  Follow Events drop players, cancel workers and release textures.
- A new event resets the scroll position to its closest camera.
- Waiting/no-feed states remain honest; runtime records are never synthesized.

### `nearest_feeds` / `supports_preview`
- Select the four nearest candidate HTTP(S) image/MJPEG endpoints globally,
  using canonical haversine distance and camera ID for deterministic ties.
- Exclude known unreachable feeds, invalid positions, credentials, non-HTTP
  schemes and known unsupported video/page formats. Unknown endpoint formats
  can still fail decoding; cards show retry state with error details on hover.
- Distances are always labeled; geographic proximity implies no incident link.

### `render_feed`
- Uses compact local spacing independent of the dashboard's roomier theme.
- The two-line sidebar header shows the event title and feed count, omitting
  redundant sorting text and raw location metadata.
- Each card places camera name/distance on one row, followed by a letterboxed
  image and one small status/provider/Open row. Padding is six points.
- Connecting/failed feeds without a texture collapse to two text rows;
  retries continue and the image expands automatically when a frame arrives.
- Clicking an image or Open uses the existing floating viewer.
- Wrap-free truncated names have full hover text; images preserve aspect ratio.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| `app.rs` | Called before the Brief and central map, including OBS view | Rendering after CentralPanel or hiding it in OBS |
| `model/mod.rs` | Registry replacement/status changes advance the revision | Mutating cameras without invalidation |
| `camera_feed_player.rs` | Playback stays independent of selected-camera state | Coupling all players to one model selection |
