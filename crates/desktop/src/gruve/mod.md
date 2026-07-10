# gruve/

## Purpose
Puts 1kEE on a [Gruve](../../../../gruve-kit/README.md) mesh so friends can open it
from their lobby and collaborate. 1kEE is a native egui/wgpu app and can't be served
over the mesh as-is, so this module is a **companion web view** rather than a port:
a small embedded HTTP server publishes the app's live situational picture and serves
a thin web UI that mirrors the host's globe and can steer it. The native console
stays the source of truth.

Conformance: Gruve adapter protocol **L1 (announce) + L2 (dispatch) + L3 (session)**.

## Components

### `GruveBridge` (`mod.rs`)
- **Does**: Owns the bridge lifecycle — binds a localhost port, starts the HTTP
  server, announces the app to the local agent, and shuttles data both ways.
- **Interacts with**: `DashboardApp` (`app.rs`) calls `start` once, then
  `drain_commands` at the top of each frame and `publish` at the end; `gruve-sdk`
  (vendored Rust crate) for the announce heartbeat; `server` + `snapshot` siblings.
- **Rationale**: One owner so the announce withdraws and the accept loop stops on
  drop. Everything degrades silently — no agent, no free port, or no viewers leaves
  the native app behaving exactly as before (contract resilience rule 1.2).

### `snapshot.rs`
- **Does**: Defines `Snapshot`, a serializable projection of `AppModel` built once
  per frame on the UI thread, plus the per-endpoint JSON serializers and the
  `Command` enum (Focus / SelectEvent) that web viewers send back. Also carries
  `Generations` — per-slice change counters (state/events/cameras/tracks/flights)
  that `publish` bumps only when a slice actually changed (`Arc::ptr_eq` for the
  big track/flight vecs, equality for the small DTO vecs and view scalars), seeded
  from wall-clock millis so a host restart can't alias a previous run's counters.
- **Interacts with**: `AppModel` (read-only) to build DTOs; `server` reads the
  snapshot under a short mutex and serializes on demand.
- **Rationale**: `AppModel` is not `Send`; rather than share it we copy a cheap
  snapshot across the thread boundary. Per-frame cost is bounded (scalars + Arc
  bumps + a few event/camera DTOs); the expensive part (thousands of tracks) is
  serialized only when a viewer actually polls. `stream_url` is deliberately not
  published — feed connection stays on the host.

### `server.rs`
- **Does**: A std-only HTTP/1.1 server. Serves the UI (`/`, `/app.js`, `/app.css`,
  `/gruve-sdk.js`) and the `api` upstream (`/state`, `/events`, `/cameras`,
  `/tracks`, `/flights`, `POST /command`) on one port. GET API routes stamp the
  slice's generation in an `X-Gen` header and honour `?gen=<last-seen>`: a match
  answers `304 Not Modified` with no body (serialization skipped); no `gen` param
  always returns a full 200, so older clients keep working.
- **Interacts with**: `ServerShared` (snapshot mutex + command sender + stop flag);
  the Gruve agent, which strips the `/apps/1kee/` and `/__gruve/api/` prefixes before
  proxying, so routes look identical standalone and over the mesh.
- **Rationale**: Zero extra dependencies — like the agent, it's all localhost
  traffic. Commands are validated (finite, clamped coords; non-empty ids) before
  reaching the UI thread.

### `web/`
- **Does**: The companion frontend. `app.js` draws an orthographic globe centred on
  the host's view, plots events/cameras/vessels/flights, polls the `api` upstream,
  POSTs steer/select commands, and shares a pin over the L3 session. Recolours to
  match the host's active theme. Polls conditionally: remembers each path's `X-Gen`
  and sends `?gen=`; on 304 it keeps existing state and skips the parse/re-render.
- **Interacts with**: `gruve-sdk.js` (vendored JS SDK) for `apiBase` / `joinSession`
  / `isServedByGruve`. All asset paths are relative so the app works under
  `/apps/1kee/` on a friend's machine.
- **Rationale**: This directory is also the `gruve doctor` target (`gruve doctor
  crates/desktop/src/gruve/web` → "Contract holds"). The files are embedded into the
  binary via `include_str!`, so the shipped app needs no asset directory.

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `app.rs` | `start` → `Option<GruveBridge>`; `publish`/`drain_commands` are cheap, non-blocking, frame-safe | Making them block or panic; calling on a non-UI thread |
| Web view (L2) | UI + `api` upstream share one port; routes resolve under `/apps/1kee/__gruve/api/…` | Splitting UI/API ports without re-declaring upstreams in the announce |
| Mesh viewers | `/command` only accepts validated Focus/SelectEvent; appliers drive the store, never replay input (rule 6) | Adding commands that simulate events or trust unbounded input |
| Web assets | `web/` passes `gruve doctor` (no hardcoded localhost, relative assets) | Hardcoding agent/localhost URLs outside SDK fallbacks; absolute asset paths |
