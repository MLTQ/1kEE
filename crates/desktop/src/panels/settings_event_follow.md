# settings_event_follow.rs

## Purpose
Offers Follow Events dwell timing in its own Settings section so operators can
experiment during a tour and save the preferred duration for future sessions.

## Components
### `render`
- Shows an editable seconds slider from 5–300, with logarithmic spacing for
  finer control over short stops. The controller defaults to 30 seconds.
- Reads/writes the controller's normalized dwell duration. Changes affect the
  current stop and preserve elapsed arrival time and camera motion.
- Explains that travel time is separate and Save Settings retains the choice.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| `settings.rs` | Renders only the section body, without saving or opening another window | Implicit disk writes or a second window |
| `model/event_follow.rs` | Setter keeps values within the shared supported range | Divergent bounds or restarting the tour on edits |

The section explains the automatic five-live-minute / one-minute recap cycle
and temporary camera-sidebar/Brief suppression independently of dwell timing.
