# event_labels.rs

## Purpose

Paints persistent event tooltip cards during Follow Events, including Focus
capture. Every event with a scene-projected anchor inside the map receives its
severity, wrapped headline and location; this is independent of the six-stop
tour ranking and of pointer position.

## Components

- `draw` joins the shared globe/local marker positions to `map_events`, including
  a followed record retained after it leaves the live page. Stable event-ID order
  prevents polling order from reshuffling cards. egui caches the text layout.
- `place_card` evaluates a fixed set of nearby positions, preferring no overlap
  and then proximity. Cards stay inside the viewport where their size permits.
  If the viewport is too crowded, cards still render rather than being capped.
- Leader lines connect displaced cards to their original marker positions.

## Contracts

- `world_map.rs` calls this only while Follow Events is enabled, before the Focus
  early return, and suppresses the duplicate event hover card in that mode.
- Uses scene visibility and marker filtering; never reprojects geographic data
  or resurrects hidden layers, far-side globe events or offscreen anchors.
- Painter-only output does not capture clicks or hover, create egui windows,
  request extra repaints, or cover floating brief/camera windows.
- `event_labels_tests.rs` checks clustered/edge placement and multiple visible
  labels with no pointer, including offscreen/absent marker exclusion.
