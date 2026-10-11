# event_labels_tests.rs

## Purpose

Checks persistent event-card behavior without loading settings, terrain or feeds.

## Coverage

- Fully hidden cards do not read the event iterator or emit any paint shapes.
- A real headless egui frame with no pointer paints all eight visible headlines
  and severity labels, exceeding the six-stop tour size. Offscreen and absent
  markers do not generate cards; each visible event appears exactly once.
- A crowded four-event cluster paints identical card rectangles over 120 moving
  frames while every connector tracks its current marker. Hidden travel clears
  those slots and the next arrival receives a fresh layout.
- Pure placement, edge and lifecycle checks live in `event_label_layout_tests`.
