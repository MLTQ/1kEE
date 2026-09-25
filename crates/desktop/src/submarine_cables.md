# submarine_cables.rs

## Purpose

Supplies the built-in submarine cable layer — cable routes and landing points —
from a TeleGeography snapshot compiled into the binary, plus the catalogue of
detail behind the landing-point tooltip and panel. It owns no network access, no
cache, and no rendering: the shared `GeoJsonLayer` pipeline draws it.

## Components

### Bundled snapshot (`submarine_cables/`)
- **Does**: Holds `cables.geojson` (route, name, colour, length, owners,
  ready-for-service date, planned flag, URL) and `landings.geojson` (name,
  country, ids of the cables landing there), coordinates rounded to 4 dp
  (~11 m). Pulled in with `include_str!`.
- **Interacts with**: `tools/fetch_submarine_cables.py`, which regenerates it.

### `tick`
- **Does**: On the first frame the layer is enabled, parses the snapshot on a
  background thread; once parsed, installs the layers into the model and wakes
  the UI. Returns immediately while the layer is off or already installed.
- **Interacts with**: `app.rs`, `AppModel`, `crate::app::request_repaint`.

### `CableCatalog` / `CableInfo` / `LandingPoint` / `catalog`
- **Does**: One entry per cable system (route segments collapsed) and per
  landing station, with each station's cables as indices sorted by name.
  `catalog()` exposes it once parsed.
- **Interacts with**: `map_tooltips::draw_landing_hover_tooltip`,
  `map_detail_panels::draw_landing_detail_panel`, both scenes' hit-testing.

### `landing_points_visible`
- **Does**: Single answer to "are landing dots on screen": cables enabled, on
  Earth, and the landing layer not hidden. Hit-testing, tooltips and the panel
  all key off it, so a hidden dot can never be hovered or clicked.
- **Interacts with**: globe/local scenes, `map_detail_panels`.

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
| `layer_drawer` | Toggling labels or landing points edits the installed layers; `tick` never overwrites them afterwards | Re-installing the parsed snapshot while layers are present |
| Map hit-testing | Feature `i` of the landing layer is `catalog().landings[i]` | Building the landing layer from anything but the catalogue |

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
- **Landing detail comes from inverting the cable records.** Each cable's
  detail lists its landing points, so ~710 requests (one per cable system) yield every station's
  country and cable list; fetching per station would take ~1,900. All 1,925
  stations link to at least one cable in the current snapshot.
- The landing layer is **built from the catalogue**, not parsed separately, so
  map indices and catalogue indices cannot drift apart
  (`landing_layer_and_catalogue_are_index_aligned`).
- Cables are Earth-only: both scenes skip them on the Moon and Mars.
- The parse result lives in a process-wide `OnceLock` and is shared with the
  model by `Arc`, so it is parsed at most once per run.
- Built-in layers start with `show_labels: false`. The globe view has no
  viewport culling for labels, and ~2,600 names drawn at once is unreadable.
- `bundled_snapshot_parses` is the guard on every regeneration — the app has no
  fallback if the committed snapshot is bad.
- **Licence.** The snapshot is TeleGeography data under CC BY-NC-SA 3.0, which
  is separate from this repository's MIT/Apache code licence and does not
  permit commercial use. See `submarine_cables/LICENSE`.
