# 1kEE Architecture

## Purpose
This document captures the first-pass structure of the 1kEE desktop demo. It is meant to keep implementation aligned while the data connectors, rendering strategy, and OSINT-specific constraints are still being explored.

## MVP Flow

1. Render a global operations canvas.
2. Surface curated events with severity and location.
3. Allow an analyst to select an event.
4. Show nearby camera records for the selected event.
5. Let the analyst attempt a feed connection from the camera list.

## Initial Modules

- `model.rs`: shared UI/domain state for demo events, cameras, selection, and interaction log
- `app.rs`: high-level layout orchestration
- `panels/world_map.rs`: geographic canvas and click interaction
- `panels/event_list.rs`: event queue and selection controls
- `panels/camera_list.rs`: nearby-camera inspection and connection attempts
- `panels/status_log.rs`: operator-facing action history
- `theme.rs`: visual setup for the demo shell

## Planned Integration Boundaries

### Event Ingest
- Input: curated event records with title, severity, timestamp, summary, and coordinates
- Output: normalized event objects pushed into the app state
- Future work: background worker, retry policy, dedupe, TTL, event aging

### Camera Registry
- Input: open camera metadata and feed URLs from public sources
- Output: normalized camera records with provider, location, type, and reachability state
- Future work: source adapters, geocoding, provenance, health checks, legal review flags

### Map / Globe Layer
- Current implementation: stylized orthographic globe in `egui` with projected graticules, continent wireframes, and HUD overlays
- Likely next step: decide whether to keep the custom globe renderer and feed it higher-fidelity coastline/topography data, or embed a richer map/globe stack
- Constraint: keep event and camera hit-testing deterministic and simple for analyst workflows

## Safety / Scope Notes

- The scaffold only simulates network connection attempts.
- Any real feed connector should stay inside an allowlisted, provenance-aware adapter layer.
- Source-specific terms, licensing, and jurisdictional constraints should be tracked before live ingestion is enabled.
