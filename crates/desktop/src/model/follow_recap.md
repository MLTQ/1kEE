# follow_recap.rs

Owns the Follow Events live/recap cycle and AppModel's follow entry points.
Five minutes of live touring precede each 60-second trailing-24-hour recap.
A three-second globe framing move precedes playback. All eight regions rotate
in a stable order, including empty regions; a complete cycle takes about 48 minutes.

- `FollowRecaps` retains the live deadline, next region and optional recap.
- `RegionalRecap` holds a frozen event/time snapshot, camera transition and
  elapsed clock. SQLite loading runs on a short-lived worker; framing waits for
  its result before playback, using the selected region's center and zoom. No events are invented for an empty cache.
- `tick_event_follow` suspends live camera ticks during recap while leaving
  Follow enabled and observing incoming live feed replacements. Tour scheduling
  history and configured dwell survive. The return tick selects a fresh stop.
- `active_replay` supplies either the automatic or manual replay to globe flares.
  Automatic recaps never edit manual replay dates, duration or controls.
- `stop_event_follow` cancels both phases and prevents automatic resumption.
  Toggle, manual navigation, other idle modes, replay and body changes use it.

The one-day history cache warms once on enabling follow when coverage is missing.
Each recap loads fresh cached data; a backfill completing during a recap is used
next time, without restarting playback. Live polling continues independently.

Recaps temporarily use Globe view. A saved Local view is restored before live
touring resumes. A stopped Globe recap leaves the operator at its current pose.
Brief and camera-sidebar rendering are suppressed without changing their flags.
The clock measures elapsed hours in the rolling window (00:00:00–23:59:59);
the overlay also shows the actual UTC start/end so it cannot be read as UTC midnight.
