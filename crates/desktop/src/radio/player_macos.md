# player_macos.rs

## Purpose
Implements native macOS streaming audio with AVPlayer. HTTP buffering, decoding
and output remain inside AVFoundation rather than the UI's render loop.

## Components
- `Player` retains one main-thread AVPlayer and polls player/item errors.
- `status` distinguishes buffering, playback and failure; a 30-second initial or
  rebuffering timeout releases unusable streams with a buffering error.
- Stop/Drop pauses and removes the current item, releasing the network resource.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| `player.rs` | Main-thread ownership, immediate controls and no external tools | Moving Objective-C objects across threads |

## Notes
AVFoundation supports common MP3/AAC/HLS feeds; provider availability and codecs
still vary. `Playing` comes from AVPlayer's actual time-control state.
