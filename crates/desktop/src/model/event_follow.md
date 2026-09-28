# event_follow.rs

Owns the event-follow idle mode's feed identities and camera trajectory. It has
no rendering, disk or network work. `AppModel` supplies completed Factal polls
and advances it with egui's absolute time.

- Enabling starts with the newest available geolocated Factal event, or waits
  for the first poll. Record IDs are remembered even while off. Repeated polls,
  revisions and records returning after falling out of a page do not restart
  travel. If a poll contains a burst, the newest timestamp wins.
- `tick` returns a newly followed ID for selection without the normal focus snap.
  It retains the target's brief when it ages out of the live page.
- Flights preserve the chosen Globe/Local projection, last ten seconds, take the
  short longitude path across the date line, and widen on long journeys. Local
  arrival uses regional terrain (zoom 9.5) and a slow yaw orbit; globe arrival
  circles the event in a small geographic orbit at zoom 35.
- Quintic Hermite curves preserve position/velocity on retarget and join the
  orbit velocity at arrival. Zoom follows a continuous widen/approach curve.
  Manual input, another idle mode, replay or leaving Earth cancels the controller.
- No fabricated events: the module only animates actual supplied records.

Tests in `event_follow_tests.rs` cover timing, feed identity, model replacement, both view modes, wrapping, interruption and velocity continuity at trajectory joins.
