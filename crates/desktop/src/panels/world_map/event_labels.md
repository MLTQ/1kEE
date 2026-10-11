# event_labels.rs

## Purpose

Paints persistent event tooltip cards during Follow Events, including Focus
capture. Every event with a scene-projected anchor inside the map receives its
severity, wrapped headline and location; this is independent of the six-stop
tour ranking and of pointer position.

## Components

- `draw` joins the shared globe/local marker positions to `map_events`, including
  a followed record retained after it leaves the live page. Stable event-ID order
  and `event_label_layout` retain existing screen-space card slots during dwell.
  egui caches text layout and owns the session-only placement state.
- New/resized cards fit around established cards; camera drift never reruns
  candidate placement for existing cards. Leader lines follow the live markers.
- Fully hidden travel discards the layout so the next arrival gets fresh slots.
- The supplied controller opacity fades text, backgrounds, borders and leader
  lines together. Zero opacity skips event lookup, layout and placement entirely.

## Contracts

- `world_map.rs` calls this only while Follow Events is enabled, before the Focus
  early return, supplies its arrival/departure opacity, and suppresses the
  duplicate event hover card in that mode. Cards are hidden during wide travel.
- Uses scene visibility and marker filtering; never reprojects geographic data
  or resurrects hidden layers/far-side globe events. New anchors must be on-screen;
  existing cards have a small exit margin to prevent viewport-edge chatter.
- Painter-only output does not capture clicks or hover, create egui windows,
  request extra repaints, or cover floating brief/camera windows.
- `event_labels_tests.rs` checks real multi-frame egui output: steady card bounds,
  moving leader anchors, hidden-travel reset, and offscreen/absent exclusion.
