# event_labels_tests.rs

## Purpose

Checks persistent event-card behavior without loading settings, terrain or feeds.

## Coverage

- Eight colocated events receive separate, in-bounds cards when space permits.
- Anchors at each viewport corner keep readable cards inside the map.
- A real headless egui frame with no pointer paints all eight visible headlines
  and severity labels, exceeding the six-stop tour size. Offscreen and absent
  markers do not generate cards; each visible event appears exactly once.
