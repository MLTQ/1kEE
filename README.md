# 1kEE

One Thousand Electric Eye is a Rust + `egui` desktop demo for an OSINT-style operations surface:

- a world map or globe as the primary canvas
- curated live events, starting with a Factal-style event stream
- openly published camera metadata layered onto geography
- click-through workflows from event -> nearby cameras -> attempted feed connection

This repository currently contains the initial application scaffold and architecture boundaries for the MVP.

## Current Scope

The first slice is intentionally narrow:

- desktop shell built with `eframe` / `egui`
- mock event and camera data to prove the interaction model
- modular Rust files with companion `.md` docs
- `beads` issue tracking for multi-session project memory
- local terrain datasets under `Data/` with a documented GDAL preprocessing path

There is no real camera or Factal network integration yet. The current UI simulates feed connection attempts so the app boundary is in place before data-source work starts.

## Workspace

- Root workspace: [`Cargo.toml`](/Users/max/Code/1kEE/Cargo.toml)
- Desktop app crate: [`crates/desktop/Cargo.toml`](/Users/max/Code/1kEE/crates/desktop/Cargo.toml)
- Architecture notes: [`docs/architecture.md`](/Users/max/Code/1kEE/docs/architecture.md)
- Terrain pipeline: [`docs/terrain-pipeline.md`](/Users/max/Code/1kEE/docs/terrain-pipeline.md)

## Run

```bash
cargo run -p one-thousand-electric-eye-desktop
```

## Notes

- Camera-source ingestion should stay limited to openly published metadata and feeds, subject to source terms and legal review.
- The `beads` tracker is initialized in this repo for planning the next slices.
