# mod.rs

## Purpose
Defines public radio feed metadata, event-area snapshots and geographic matching.
This layer carries no Factal-specific state and does not start discovery or audio.

## Components
- `RadioArea` captures the event identity, location and title when the user searches.
- `RadioFeed` retains optional directory coordinates, categories, provider page and UUID.
- `distance_km` uses great-circle distance, including antimeridian and polar cases.
- `stream_url` rejects non-HTTP(S), credentials, obvious local targets and providers
  whose third-party access requires a separate agreement.
  URLs are capped at 4 KiB before parsing.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| `catalog.rs` | Unknown coordinates stay unknown | Assigning an event's location to remote feeds |
| `panels/event_radio.rs` | Distances describe published station locations | Claiming coverage or incident relevance from proximity |
| `player.rs`, `saved.rs` | Shared URL checks before playback or persistence | Accepting file URLs or restricted provider audio |

## Notes
Metadata checks are not a complete redirect/DNS security boundary. Native players
handle their own HTTP transport; no server-side proxy or shared radio endpoint exists.
