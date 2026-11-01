# camera_feed_viewer.rs

## Purpose

Owns the floating egui live-camera window. Shared background reading, decoding,
texture uploads and cancellation live in `camera_feed_player.rs` so the Follow
Events sidebar uses the same playback path.

## Components

### `CameraFeedViewer`

- **Does**: Tracks the window lifecycle and delegates playback to one
  `CameraFeedPlayer`.
- **Interacts with**: `DashboardApp` in `app.rs`, camera selection/window state
  in `model/mod.rs`, and `CameraFeed` metadata.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| `app.rs` | `render` is non-blocking and may be called every frame | Performing network reads on the UI thread |
| `model/mod.rs` | The window follows `selected_camera_id` while `camera_feed_window_open` is true | Decoupling the worker from model selection |
| Camera sources | Feed URLs are HTTP(S), credential-free, and return a decodable still image or MJPEG stream | Adding protocols without a dedicated decoder |

## Notes

- The floating player starts only after a camera pip or Open Feed action.
- Closing or changing the selected feed signals the old worker to stop; a
  request already blocked in the HTTP client may take up to its timeout to exit.
- Video containers such as HLS/MP4 are reported as unsupported until a video
  decoder is added.
- The feed window omits the `Project Eyes On · ` provider prefix, showing only
  the underlying source name (for example, Insecam). Registry metadata is kept
  intact and other provider names display as supplied.
