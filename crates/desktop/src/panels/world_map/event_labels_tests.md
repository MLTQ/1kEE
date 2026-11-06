# event_labels_tests.rs

## Purpose

Checks persistent event-card behavior without loading settings, terrain or feeds.

## Coverage

- Fully hidden cards do not read the event iterator or emit any paint shapes.
- A real headless egui frame with no pointer paints only the selected event's
  headline/severity among ten records. Closing the Brief or projecting the
  selected marker offscreen produces no card.
- A crowded four-event cluster paints one identical card rectangle over 120 moving
  frames while its connector tracks the selected marker. Hidden travel clears
  those slots and the next arrival receives a fresh layout.
- Pure placement, edge and lifecycle checks live in `event_label_layout_tests`.
