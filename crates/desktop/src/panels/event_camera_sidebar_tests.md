# event_camera_sidebar_tests.rs

## Purpose
Regression coverage for event-relative feed selection, worker lifecycle and
native sidebar layout. All events and feeds are private test fixtures; inert
channel-backed players prevent network requests and settings writes.

## Components
- Distance ordering crosses the date line, keeps distant cameras when they are
  the closest available, breaks ties deterministically and rejects unusable feeds.
- Event/registry changes replace the four-feed set, cancel removed players,
  retain overlapping textures and leave the map's proximity cache untouched.
- Native egui frames cover minimum/default widths and normal/OBS modes, a
  long camera name, single-column ordering and the Brief's non-overlap.
  Offscreen cards are clipped by the scroll viewport rather than squeezed.
- Waiting/empty states do not create players; a tour target survives paging out.
- `frame` and `prime` are shared by the optional GPU preview.

## Contracts
No source polling, credential access, network work or persistent settings changes.
