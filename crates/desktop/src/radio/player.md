# player.rs

## Purpose
Exposes play, pause/resume, volume, status and stop without leaking platform media
objects into the domain model. Audio begins only after an explicit Play action.

## Components
- `RadioPlayer` validates URLs, owns one platform player, and releases it on Stop/Drop.
- `PlaybackStatus` distinguishes connecting, playing, paused and failed states.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| Event radio window | One audible feed at a time; Close stops it | Detached audio surviving window closure |
| Platform backends | No blocking network work in UI calls | Decoding a remote response synchronously |

## Notes
macOS uses native AVFoundation; other desktop targets require ffplay on PATH.
Live providers may buffer or delay audio. Paused audio is not an incident archive.
