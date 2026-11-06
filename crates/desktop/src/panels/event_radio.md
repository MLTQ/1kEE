# event_radio.rs

## Purpose
Owns the event-area radio window, explicit directory searches and a single audio
player. Audio and event context never follow tour changes automatically.

## Components
- `button` / `open_for` snapshot a selected event and launch an explicit search.
- `title_bar_button` draws an accessible, clickable radio glyph in the Brief's
  existing title bar without increasing content height.
- `search` runs one bounded, cancellable metadata worker. Results are cached for
  15 minutes; geographic/category/text filtering is local.
- Source labels distinguish GWES weather, independent dispatch/scanner streams
  and user-saved URLs. Only the latter expose Remove. Catalog totals separate
  weather from scanner/dispatch; area-center attribution credits OSM and Census.
- `render` takes only the optional selected event, drains completion, renders
  feed cards, and offers player controls.
- `new_feed` / `persist` save user-pinned streams separately from API settings.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| Brief/event list | A click queries radio for that event | Automatic playback or tour-following |
| `app.rs` | Window renders after event clicks and stops audio on Close/Drop | Detached player lifecycle |
| Feed list | Unknown coordinates excluded from Near searches | Presenting unknown locations as nearby |
| Saved list | Failed writes retain previous records | Silently losing saved streams |

## Notes
`Browse all` explicitly includes remote/unlocated stations. Directory distance is
not reception coverage. Provider links open their own sites; Broadcastify/OpenMHz
APIs/audio are not consumed without separate permission. Saved locations are
explicit user pins. No radio data/audio enters Gruve or event replay.

The Add stream form appears before the bounded, vertically shrinking feed list
so short result sets and the form stay usable in a compact window.
