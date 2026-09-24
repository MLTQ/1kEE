# submarine_cables.rs

## Purpose

Supplies the built-in submarine cable layer — cable routes and landing points —
from a TeleGeography snapshot compiled into the binary. It owns no network
access, no cache, and no rendering: the shared `GeoJsonLayer` pipeline draws it.

## Components

### Bundled snapshot (`submarine_cables/`)
- **Does**: Holds `cables.geojson` and `landings.geojson`, trimmed to the
  properties the renderer uses (cable name and colour, landing name) with
  coordinates rounded to 4 dp (~11 m). Pulled in with `include_str!`.
- **Interacts with**: `tools/fetch_submarine_cables.py`, which regenerates it.

### `tick`
- **Does**: On the first frame the layer is enabled, parses the snapshot on a
  background thread; once parsed, installs the layers into the model and wakes
  the UI. Returns immediately while the layer is off or already installed.
- **Interacts with**: `app.rs`, `AppModel`, `crate::app::request_repaint`.

### `build_layer`
- **Does**: Parses one payload into a named built-in layer: fixed layer colour,
  visible, labels off.
- **Interacts with**: `model::GeoJsonLayer::parse`.

### Provenance constants
- **Does**: Supply TeleGeography attribution text and link for the layer drawer.
- **Interacts with**: `panels/layer_drawer.rs`.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| `AppModel` | Both layers arrive together, fully parsed | Installing one layer without the other |
| `app.rs` | `tick` is non-blocking and does nothing while the layer is off | Parsing on the UI thread, or before the operator opts in |
| World-map renderers | Input is a plain `&[GeoJsonLayer]`, same as user uploads | Introducing a cable-specific geometry or renderer path |
| `layer_drawer` | Toggling labels replaces the model's layers; `tick` never overwrites them afterwards | Re-installing the parsed snapshot while layers are present |

## Notes

- **Why bundled rather than fetched.** Cables change a handful of times a year.
  A runtime fetch bought freshness nobody needs at the cost of network access,
  a cache, and failure states. Refresh by running
  `python3 tools/fetch_submarine_cables.py` and committing the result.
- The earlier fetching version never displayed anything: its `tick` decided
  whether to start a worker *before* recording the finished result, so every
  result was discarded as stale and the status stayed on "loading…". Nothing
  here is scheduled any more, so that class of bug cannot recur.
  `fire_source::step` guards the same ordering for the live fire feed.
- The parse result lives in a process-wide `OnceLock` and is shared with the
  model by `Arc`, so it is parsed at most once per run.
- Built-in layers start with `show_labels: false`. The globe view has no
  viewport culling for labels, and ~2,600 names drawn at once is unreadable.
- `bundled_snapshot_parses` is the guard on every regeneration — the app has no
  fallback if the committed snapshot is bad.
- **Licence.** The snapshot is TeleGeography data under CC BY-NC-SA 3.0, which
  is separate from this repository's MIT/Apache code licence and does not
  permit commercial use. See `submarine_cables/LICENSE`.
