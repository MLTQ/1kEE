# camera_feed_viewer.rs

## Purpose

Owns the floating egui live-camera window and its bounded background media
reader. Network reads and image decoding stay off the UI thread; the panel only
uploads the most recent decoded frame to an egui texture.

## Components

### `CameraFeedViewer`

- **Does**: Tracks the selected camera worker, current texture, connection
  status, last-frame time, and window lifecycle.
- **Interacts with**: `DashboardApp` in `app.rs`, camera selection/window state
  in `model/mod.rs`, and `CameraFeed` metadata.

### Snapshot reader

- **Does**: Re-fetches JPEG/PNG image endpoints at a bounded interval with
  no-cache headers, fresh `COUNTER` placeholders, and a 12 MB response limit.
- **Rationale**: Many public webcams expose a changing still-image URL rather
  than a persistent video protocol.

### MJPEG reader

- **Does**: Reads multipart responses incrementally and extracts JPEG frames
  using start/end markers without retaining an unbounded response body.
- **Rationale**: egui displays decoded textures, so MJPEG is normalized to the
  same latest-frame path as snapshots.

### `take_next_jpeg` / `fit_size`

- **Does**: Extracts complete JPEGs from a streaming buffer and fits textures
  inside the window without distortion or upscaling.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| `app.rs` | `render` is non-blocking and may be called every frame | Performing network reads on the UI thread |
| `model/mod.rs` | The window follows `selected_camera_id` while `camera_feed_window_open` is true | Decoupling the worker from model selection |
| Camera sources | Feed URLs are HTTP(S), credential-free, and return a decodable still image or MJPEG stream | Adding protocols without a dedicated decoder |

## Notes

- The worker starts only after explicit operator interaction.
- Closing or changing the selected feed signals the old worker to stop; a
  request already blocked in the HTTP client may take up to its timeout to exit.
- Video containers such as HLS/MP4 are reported as unsupported until a video
  decoder is added.
