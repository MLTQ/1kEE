# event_camera_sidebar.rs

## Purpose
Shows the current Follow Events stop's nearest camera feeds in a resizable,
scrollable column on the right, including OBS view. It reserves actual panel
space so the map, event labels and chyron lay out beside the feeds.

## Components

### `EventCameraSidebar`
- Owns at most eight independent 640-pixel `CameraFeedPlayer`s outside `AppModel`.
- Recomputes selection when the tour moves to another event, the event coordinates
  change, or the camera registry updates. The map keeps its nearby-camera snapshot.
- Keeps overlapping connections across tour stops. Removed cameras and leaving
  Follow Events drop players, cancel workers and release textures.
- A new event resets the scroll position to its closest camera.
- Waiting/no-feed states remain honest; runtime records are never synthesized.

### `nearest_feeds` / `supports_preview`
- Select the eight nearest candidate HTTP(S) image/MJPEG endpoints globally,
  using canonical haversine distance and camera ID for deterministic ties.
- Exclude known unreachable feeds, invalid positions, credentials, non-HTTP
  schemes and known unsupported video/page formats. Unknown endpoint formats
  can still fail decoding; cards show retry state with error details on hover.
- Distances are always labeled; geographic proximity implies no incident link.

### `render_feed`
- Uses compact local spacing independent of the dashboard's roomier theme.
- The two-line sidebar header shows the event title and feed count, omitting
  redundant sorting text and raw location metadata.
- Camera name/location and distance share a translucent band over the image.
  There are no separate header or status/provider/Open rows; padding is two points.
- Previews use source aspect up to 4:3, giving taller feeds the space reclaimed
  from chrome while preserving image proportions. Wider feeds remain compact.
- Connecting/failed feeds without a texture collapse to a 46-point tile;
  retries continue and the image expands automatically when a frame arrives.
- Clicking anywhere on the tile uses the existing floating viewer. Provider,
  status and error details remain available on hover; retries appear over the tile.
- Wrap-free truncated names have full hover text; images preserve aspect ratio.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| `app.rs` | Called before the Brief and central map, including OBS view | Rendering after CentralPanel or hiding it in OBS |
| `model/mod.rs` | Registry replacement/status changes advance the revision | Mutating cameras without invalidation |
| `camera_feed_player.rs` | Playback stays independent of selected-camera state | Coupling all players to one model selection |

Automatic recaps clear sidebar players/textures and reserve no panel width. The
next live phase recreates the nearest-feed column from the newly followed stop.
