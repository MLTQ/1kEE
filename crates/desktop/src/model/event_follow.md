# event_follow.rs

Owns the live-event idle mode's camera trajectory and stop timing. It has no
rendering, disk or network work. `AppModel` supplies completed Factal payloads
and advances it with egui's absolute time; `event_tour.rs` owns weighted selection.

- Enabling starts with the highest-priority eligible record, or waits for real
  data. All eligible Factal records in the live window join a severity-weighted
  rotation. Higher-severity stories get more visits, while waiting credit gives
  other stories turns and recent-stop exclusion spaces out repeats.
- Refreshes retain scheduling history and update metadata without interrupting
  flight/dwell, even if a newer or more severe headline arrives. A single record
  stays in orbit; an empty payload schedules no new stops. Network failures leave
  the last successful tour available.
- Each stop gets its flight time plus ten seconds orbiting with its brief open.
  Selection continues from the current weighted rotation between payloads.
  Missed frames do not cause catch-up hops through multiple unread briefs.
- `tick` returns each newly followed ID for selection without a focus snap. It
  retains the active brief/marker if the record ages out of the live page.
  `tour_position` supplies the current severity rank/count for the brief.
- Flights preserve the chosen Globe/Local view. Duration scales with great-circle
  travel from the current camera pose, including short date-line/polar distances,
  with a 0.65-second minimum and a hard ten-second cap. Large zoom changes get a
  modest extra allowance within that cap. Local arrival uses regional zoom 9.5
  and a yaw orbit; globe arrival circles the location at zoom 35.
- Quintic Hermite curves preserve position/velocity on retarget and join the
  orbit velocity at arrival. Widen/approach zoom phases scale with duration;
  local heading advances at the orbit rate so nearby hops do not swivel sharply.
- `event_label_opacity` fades the selected event's card in over 0.45 seconds after the
  flight reaches its arrival pose/zoom. Departure fades from the current opacity
  to zero over 0.25 seconds, then stays hidden throughout travel. Retargeting
  during either fade preserves opacity; no separate zoom threshold can drift
  from the trajectory's actual destination in Globe or Local.
- The separate five-row chyron remains readable throughout travel; camera
  opacity never hides or restarts a summary crawl.
- Manual input, another idle mode, replay or leaving Earth cancels the controller
  and discards the tour. No synthetic events enter runtime state.

`event_follow_tests.rs` checks ranking, queue refresh/cycling, dwell, adaptive
travel timing, smooth joins, model selection/brief behavior, and cancellation.
