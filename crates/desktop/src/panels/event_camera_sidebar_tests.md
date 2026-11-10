# event_camera_sidebar_tests.rs

Brief rendering supplies an inert radio controller; these fixtures do not query radio or play audio.

## Purpose
Regression coverage for event-relative feed selection, worker lifecycle and
native sidebar layout. All events and feeds are private test fixtures; inert
channel-backed players prevent network requests and settings writes.

## Components
- Distance ordering crosses the date line, keeps distant cameras when they are
  the closest available, breaks ties deterministically and rejects unusable feeds.
- Tour and registry changes replace the eight-feed set, cancel removed players,
  retain overlapping textures and leave the map's proximity cache untouched.
- Native egui frames cover minimum/default widths and normal/OBS modes, a
  long camera name, single-column ordering and the Brief's non-overlap.
  A failed feed collapses so the first four camera headings fit without scrolling;
  raw WKT location strings do not occupy the compact header.
- Cards omit the provider/Open row; a 4:3 image occupies its full aspect and
  clicking it exercises the same open action as clicking its overlay.
- Waiting/empty states do not create players; a tour target survives paging out.
- Scrolling reaches the eighth preview. Moving to another report at the same
  coordinates returns the list to its nearest feed.
- `frame`, `prime` and `fail_second_feed` are shared by the optional GPU preview.

## Contracts
No source polling, credential access, network work or persistent settings changes.
