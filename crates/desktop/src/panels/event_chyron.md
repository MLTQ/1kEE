# event_chyron.rs

## Purpose
Replace the wall of event-map cards with five legible single-line news crawls
during Follow Events, including Focus capture. No network or model mutations.

## Components
- `draw` paints a bottom-aligned five-row panel on the map layer. Each row shows
  the full normalized `title - level - summary`, with colored severity and a
  subtle highlight for the currently selected story. Text is never ellipsized.
  Summary text uses a bright theme-matched color for contrast over busy maps.
- Overflow scrolls left at 42 logical pixels/second after a two-second reading
  pause, then holds the final text. Short rows remain readable for ten seconds.
- Context-owned `State` retains stories and start times through camera/poll
  changes. Each row finishes its summary before replacement. A new selection
  gets the next available row; the initial row starts with the followed event.
- Other stories prefer on-screen events, severity and recency, then advance a
  persistent cursor through the live Factal pool. Active IDs/identical lines are
  excluded and recent IDs are avoided when alternatives remain.
  Inserting a new selection does not move the ordinary coverage cursor.
- Small/duplicate-only feeds retry unfilled slots at most once per second,
  avoiding repeated sorting and text comparison on every animation frame.
- `HEIGHT` lets the Brief reserve space; `clear` discards session state when
  follow stops. Empty feeds paint nothing, and small feeds never fabricate rows.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| `world_map.rs` | Returns the painted bounds so the selected map card stays above the chyron | Covering map labels or consuming pointer input |
| `factal_brief.rs` | Fixed five-row height is available for window sizing | Changing height without reserving it |
| Tests | Fixed-time real egui output can be inspected without feeds | Requiring the running app or real API data |

## Notes
The ticker remains readable during camera travel. The selected map card retains
the existing arrival/departure fades and is shown only while its Brief is open.
