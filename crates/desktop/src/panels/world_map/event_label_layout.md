# event_label_layout.rs

## Purpose

Keeps each event card in a stable screen-space slot during an event-tour dwell.
Marker movement updates the connector, not the card arrangement. This prevents
tiny orbit movements from flipping a crowded cluster between equivalent layouts.

## Components

- `Layout::begin_frame` expires slots unused for two seconds and resets on map
  viewport changes. The renderer discards the whole layout during hidden travel.
- `accepts_anchor` requires new anchors inside the viewport. Existing cards have
  a 24-point exit margin so small edge crossings cannot toggle them repeatedly.
  Only scene-provided markers qualify; hidden globe-side points stay hidden.
- `place` reserves existing slots before allocating new or resized cards, so
  a new event sorting earlier cannot move its neighbors. Missing cards reserve
  their space briefly to survive visibility changes without rearranging.
- `place_card` evaluates nearby candidates only when a card needs a new slot.
  Crowded views continue to show every card; it never culls by overlap.

## Contracts

- Call `begin_frame` before visibility filtering and batch placement.
- Requests have stable, unique event IDs and use the same ordering as drawing.
- The egui context owns this cloneable, session-only layout; no model or disk
  state, repaints, global locks or projection work live here.
- `event_label_layout_tests.rs` verifies temporal stability, arrival/departure
  of neighboring cards, edge hysteresis, expiry and viewport changes.
