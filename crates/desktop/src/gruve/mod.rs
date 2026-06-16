//! Gruve mesh integration — the companion-web-view bridge.
//!
//! 1kEE is a native egui/wgpu app, so it can't be served over the mesh directly.
//! This module gives it a path onto Gruve without a WASM port: a lightweight
//! embedded HTTP server publishes the app's live situational picture (the host's
//! globe centre, events, vessels, flights, cameras) and serves a companion web
//! view that mirrors it. Friends open the lobby tile and see what the analyst sees;
//! they can steer the host's globe ("look here") or select an event back on the
//! host. The native console stays the source of truth.
//!
//! Lifecycle, all driven from `DashboardApp`:
//!   * `GruveBridge::start(&model)` once — binds a port, serves, announces.
//!   * `bridge.drain_commands(&mut model)` at the top of each frame — apply remote
//!     viewer actions before the UI renders.
//!   * `bridge.publish(&model)` at the end of each frame — refresh the snapshot.
//!
//! Everything degrades silently: no agent running, port taken, no viewers — the
//! native app behaves exactly as it did before (contract resilience rule 1.2).

mod server;
mod snapshot;

use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crossbeam_channel::Receiver;

use server::ServerShared;
use snapshot::{Command, Snapshot};

use crate::model::{AppModel, GeoPoint, GlobeViewState};

/// Stable lobby slug — matches `^[a-z0-9][a-z0-9-]{0,31}$` and the web view's
/// announce/session id.
const APP_ID: &str = "1kee";
/// Preferred UI+API port; we probe a small range so two instances (or a leftover
/// socket) don't wedge startup.
const PORT_RANGE: std::ops::Range<u16> = 9766..9786;

pub struct GruveBridge {
    shared: Arc<ServerShared>,
    commands_rx: Receiver<Command>,
    port: u16,
    // Heartbeats to the local agent from a background thread; withdraws on drop.
    _announce: gruve_sdk::AnnounceHandle,
}

impl GruveBridge {
    /// Bind, serve, and announce. Returns `None` if no port is free — in which case
    /// the host app simply runs without the companion (never an error to the user).
    pub fn start(model: &AppModel) -> Option<GruveBridge> {
        let (listener, port) = bind_in_range()?;

        let (tx, rx) = crossbeam_channel::unbounded();
        let shared = Arc::new(ServerShared {
            snapshot: Mutex::new(Snapshot::from_model(model)),
            commands: tx,
            stop: AtomicBool::new(false),
        });
        server::serve(listener, shared.clone());

        // Listen-then-announce (contract 1.4): the socket is already bound above, so
        // the agent's port probe succeeds. UI and API share one port, so the `api`
        // upstream points back at us — `apiBase("api")` in the web view resolves to
        // `/apps/1kee/__gruve/api`.
        let announce = gruve_sdk::Announce::app(APP_ID, "1kEE", port)
            .blurb("live OSINT globe — events, vessels, flights, cameras")
            .hue(190)
            .upstream("api", port)
            .start();

        eprintln!(
            "gruve: companion web view on http://127.0.0.1:{port}/ — announced as '{APP_ID}' \
             (open the Gruve lobby, or this URL standalone)"
        );

        Some(GruveBridge {
            shared,
            commands_rx: rx,
            port,
            _announce: announce,
        })
    }

    /// Refresh the snapshot the web view polls. Cheap; call once per frame.
    pub fn publish(&self, model: &AppModel) {
        let next = Snapshot::from_model(model);
        if let Ok(mut slot) = self.shared.snapshot.lock() {
            *slot = next;
        }
    }

    /// Apply any commands web viewers have sent since the last frame. These are the
    /// only way the mesh writes into the host — explicit, one-shot, and idempotent
    /// (they drive the store; they never replay input events — contract rule 6).
    pub fn drain_commands(&self, model: &mut AppModel) {
        let mut applied = false;
        while let Ok(cmd) = self.commands_rx.try_recv() {
            apply_command(model, cmd);
            applied = true;
        }
        if applied {
            // Wake the event loop so a remote action shows up even when the host is idle.
            crate::app::request_repaint();
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for GruveBridge {
    fn drop(&mut self) {
        // Stop the accept loop; the announce handle withdraws on its own drop.
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

fn apply_command(model: &mut AppModel, cmd: Command) {
    match cmd {
        Command::Focus { point } => focus_globe(&mut model.globe_view, point),
        Command::SelectEvent { id } => {
            if let Some(ev) = model.events.iter().find(|e| e.id == id) {
                let point = ev.location;
                model.selected_event_id = Some(id);
                focus_globe(&mut model.globe_view, point);
            }
        }
    }
}

/// Point the host's globe at `point`, killing any drift so the camera actually
/// settles there (a remote "look here" shouldn't fight an active auto-spin).
fn focus_globe(view: &mut GlobeViewState, point: GeoPoint) {
    view.auto_spin = false;
    view.meander_mode = false;
    view.vel_yaw = 0.0;
    view.vel_pitch = 0.0;
    view.focus_on(point);
}

/// Bind the first free port in `PORT_RANGE` on localhost.
fn bind_in_range() -> Option<(TcpListener, u16)> {
    for port in PORT_RANGE {
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)) {
            return Some((listener, port));
        }
    }
    None
}
