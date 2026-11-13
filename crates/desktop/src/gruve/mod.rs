//! The host serves the companion, a demand-driven window stream and map commands
//! on one loopback port. Gruve handles discovery, peer transport and port forwarding.
//! The native remote viewer runs separately from the host's model and data sources.

mod server;
mod snapshot;
mod surface;
pub(crate) mod viewer;

use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crossbeam_channel::Receiver;

use server::ServerShared;
use snapshot::{Command, Snapshot};

use crate::model::{AppModel, GeoPoint};

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
    capture: surface::Capture,
    map_rect: egui::Rect,
    // Heartbeats to the local agent from a background thread; withdraws on drop.
    _announce: gruve_sdk::AnnounceHandle,
}

impl GruveBridge {
    /// Bind, serve, and announce. Returns `None` if no port is free — in which case
    /// the host app simply runs without the companion (never an error to the user).
    pub fn start(model: &AppModel) -> Option<GruveBridge> {
        let (listener, port) = bind_in_range()?;

        let (tx, rx) = crossbeam_channel::bounded(64);
        let surface = Arc::new(surface::Surface::default());
        let capture = surface::Capture::new(surface.clone());
        let shared = Arc::new(ServerShared {
            surface,
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
            .shared_state()
            .hue(190)
            .upstream("api", port)
            .start();

        eprintln!(
            "gruve: companion web view on http://127.0.0.1:{port}/ — announcing as '{APP_ID}' \
             (open the Gruve lobby, or this URL standalone)"
        );

        Some(GruveBridge {
            shared,
            commands_rx: rx,
            port,
            capture,
            map_rect: egui::Rect::NOTHING,
            _announce: announce,
        })
    }

    /// Refresh the snapshot the web view polls. Cheap; call once per frame.
    pub fn publish(&self, model: &AppModel) {
        let mut next = Snapshot::from_model(model);
        if let Ok(mut slot) = self.shared.snapshot.lock() {
            // Bump each slice's generation counter only if that slice changed, so
            // pollers holding the current generation get 304s instead of bodies.
            next.carry_generations(&slot);
            *slot = next;
        }
    }

    /// Apply a bounded batch on the UI thread before the map renders.
    pub fn drain_commands(&self, model: &mut AppModel) {
        for cmd in self.commands_rx.try_iter().take(64) {
            match cmd {
                Command::Navigate { action, epoch } => {
                    if model.share_map
                        && !model.settings_open
                        && self.map_rect.is_positive()
                        && self.shared.surface.live_epoch() == Some(epoch)
                    {
                        model.stop_event_follow();
                        crate::panels::world_map::remote_navigation(
                            &mut model.globe_view,
                            self.map_rect,
                            action,
                        );
                    }
                }
                Command::Focus { point } => focus_map(model, point),
                Command::SelectEvent { id } => {
                    if let Some(event) = model.events.iter().find(|e| e.id == id) {
                        let point = event.location;
                        model.selected_event_id = Some(id);
                        focus_map(model, point);
                    }
                }
            }
        }
    }

    pub fn update_surface(&mut self, ctx: &egui::Context, model: &AppModel, rect: egui::Rect) {
        self.map_rect = rect;
        let mode = if !model.share_map {
            surface::Mode::Off
        } else if model.settings_open {
            surface::Mode::Paused
        } else {
            surface::Mode::Live
        };
        self.capture.update(ctx, mode);
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for GruveBridge {
    fn drop(&mut self) {
        // Stop the accept loop; the announce handle withdraws on its own drop.
        self.shared.stop.store(true, Ordering::Release);
        let _ = std::net::TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn focus_map(model: &mut AppModel, point: GeoPoint) {
    model.stop_event_follow();
    model.globe_view.stop_motion();
    model.globe_view.focus_on(point);
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

#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Navigation {
    /// Fraction of the map width and height dragged since the previous command.
    Pan {
        dx: f32,
        dy: f32,
    },
    Zoom {
        factor: f32,
    },
}

impl Navigation {
    fn valid(self) -> bool {
        match self {
            Self::Pan { dx, dy } => dx.abs() <= 1.0 && dy.abs() <= 1.0,
            Self::Zoom { factor } => (0.25..=4.0).contains(&factor),
        }
    }
}
