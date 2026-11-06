# camera_feed_player.rs

## Purpose
Shares non-blocking HTTP snapshot/MJPEG playback between the floating camera
window and the Follow Events sidebar. Network reads and decoding stay on a
background worker; the UI drains a bounded channel and uploads decoded textures.

## Components

### `CameraFeedPlayer`
- `sync` retains a connection while camera ID, URL and MJPEG mode match;
  registry endpoint changes restart it even when the camera ID is unchanged.
- `new` bounds the decoded texture edge (640 for sidebar, 1920 for floating).
- `start` refreshes playback; `stop` and Drop signal cancellation and release
  textures, errors and frame timestamps so a new event cannot show stale images.

### Snapshot and MJPEG readers
- Snapshot polling uses no-cache headers, fresh `COUNTER` placeholders and a
  900 ms interval. Multipart streams extract complete JPEG frames incrementally.
- Both enforce a 12 MB input/frame limit and bounded three-message channels.
- Only credential-free HTTP(S) URLs are accepted. Retry waits are interruptible;
  an outstanding request can take up to its 20-second timeout to stop.

### `fit_size`
- Preserves aspect ratio without upscaling the decoded image.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| `camera_feed_viewer.rs` | Non-blocking sync and full-size playback state | Blocking UI or changing state fields |
| `panels/event_camera_sidebar.rs` | Independent bounded players with immediate texture cleanup | Global selected-camera coupling or unbounded queues |
| Camera sources | HTTP(S) still images and MJPEG; no HLS/MP4 decoder | Adding protocols without dedicated decoding |

## Notes
Inline tests cover JPEG extraction, aspect ratio, counters, bounded preview
decoding, full-channel frame dropping and endpoint-change cancellation. The
test-only `fixture` supplies inert playback for native sidebar rendering tests.
