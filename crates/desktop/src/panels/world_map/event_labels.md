# event_labels.rs

## Purpose

Paints only the selected event's tooltip card during Follow Events while its
Brief is open, including Focus capture. Other stories use the five-row chyron.

## Components

- `draw` requires a selected ID (supplied only for an open Brief) and joins the
  shared globe/local marker positions to `map_events`, including
  a followed record retained after it leaves the live page. Stable event-ID order
  and `event_label_layout` retain its screen-space card slot during dwell.
  egui caches text layout and owns the session-only placement state.
- Camera drift never reruns placement for the existing selected card. Its leader
  follows the live marker; closed briefs drop the saved layout immediately.
- Fully hidden travel discards the layout so the next arrival gets fresh slots.
- The supplied controller opacity fades text, backgrounds, borders and leader
  lines together. Zero opacity skips event lookup, layout and placement entirely.

## Contracts

- `world_map.rs` calls this only while Follow Events is enabled, before the Focus
  early return, supplies its arrival/departure opacity, and suppresses the
  duplicate event hover card in that mode. The card is hidden during wide travel,
  and its viewport excludes the bottom chyron.
- Uses scene visibility and marker filtering; never reprojects geographic data
  or resurrects hidden layers/far-side globe events. New anchors must be on-screen;
  existing cards have a small exit margin to prevent viewport-edge chatter.
- Painter-only output does not capture clicks or hover, create egui windows,
  request extra repaints, or cover floating brief/camera windows.
- `event_labels_tests.rs` checks only-selected output, closed-brief suppression,
  steady card bounds, moving leaders and hidden/offscreen/absent exclusion.
