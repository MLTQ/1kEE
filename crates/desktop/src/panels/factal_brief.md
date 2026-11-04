# factal_brief.rs

## Purpose
Renders the operator-facing Brief window for the currently selected live event. This keeps the raw/private-source payload display separate from the main map shell and event list.

## Components

### `render_factal_brief`
- **Does**: Shows a small window with the selected event headline, parsed Factal fields, and a collapsible pretty-printed raw payload when a Factal-backed event is selected and the brief window is open
- **Interacts with**: `AppModel` in `model.rs`, `FactalBrief` carried on `EventRecord`
- **Rationale**: Keeps the high-signal normalized event UI lightweight while still giving the operator access to the full payload for inspection

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `app.rs` | This renderer can be called every frame and only opens a window when `factal_brief_open` is set | Removing the function or changing its state contract |
| `world_map.rs` | The selected event can expose an optional `FactalBrief` payload | Removing `EventRecord::factal_brief` without replacing the detail source |

## Notes
- Non-Factal demo events close the window automatically because they do not carry a raw Factal payload.
- The raw JSON is shown read-only so the window is an inspection surface, not an editor.
- Visible window and field labels use neutral wording and omit the source-brand
  row. Source metadata and raw payload contents remain intact. The original
  window ID preserves ordinary brief placement across the title change.

Event-follow mode uses a separate, narrower right-anchored window placement with vertical scrolling to keep the map center clear. Ordinary brief placement is preserved separately. The window can be closed during an orbit; the next new target reopens it.
Its anchor is constrained to the remaining map rectangle after side panels,
keeping the Brief beside the nearest-camera column instead of covering feeds.

During event follow, the brief displays “Priority rank · N of M” for the current
record's severity rank in the entire live pool. This is independent of the
weighted visit order. A record that has left the refreshed pool keeps its brief
but omits the rank indicator.

The follow window reserves the five-row chyron and map footer at the bottom of
the screen, including the compact coordinate row. Long briefs scroll inside
that height instead of covering the crawl or readout.

Automatic continental recaps suppress the live Brief without changing its open
flag. Live touring reopens/selects the next stop through its normal path.
