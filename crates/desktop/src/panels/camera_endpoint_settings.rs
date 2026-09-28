//! Nonblocking controls for the persisted directory endpoint cache.
use crate::camera_directory_pipeline::endpoints::{self, Action, Snapshot, Store};
use crate::{camera_registry, model::AppModel};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone)]
enum Command {
    Load,
    Discover,
    Check,
    Forget(String),
    RemoveFailed,
}

struct Reply {
    snapshot: Arc<Snapshot>,
    message: Option<String>,
    removed: Vec<String>,
}

#[derive(Clone, Default)]
struct State {
    snapshot: Option<Arc<Snapshot>>,
    worker: Option<crossbeam_channel::Receiver<Result<Reply, String>>>,
    refreshed: Option<Instant>,
    message: Option<String>,
    error: Option<String>,
    filter: String,
}

pub(super) fn render(ui: &mut egui::Ui, model: &mut AppModel) {
    let country =
        crate::settings_store::normalize_eyes_on_country_code(&model.eyes_on_country_code);
    let scope = if country.is_empty() {
        "global".to_owned()
    } else {
        country
    };
    let id = ui.id().with(("saved-camera-endpoints", &scope));
    let mut state = ui.data(|d| d.get_temp::<State>(id).unwrap_or_default());
    if let Some(receiver) = &state.worker {
        match receiver.try_recv() {
            Ok(result) => {
                state.worker = None;
                state.refreshed = Some(Instant::now());
                match result {
                    Ok(reply) => {
                        if !reply.removed.is_empty() {
                            let cameras = model
                                .cameras
                                .iter()
                                .filter(|camera| {
                                    !camera.id.starts_with("eyes-on-")
                                        || !reply.removed.contains(&camera.stream_url)
                                })
                                .cloned()
                                .collect();
                            model.replace_camera_registry(cameras, "saved camera cache");
                        }
                        state.snapshot = Some(reply.snapshot);
                        if let Some(message) = reply.message {
                            model.push_log(message.clone());
                            state.message = Some(message);
                        }
                        state.error = None;
                    }
                    Err(error) => {
                        state.error = Some(error);
                    }
                }
            }
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                state.worker = None;
                state.refreshed = Some(Instant::now());
                state.error = Some("The camera cache worker stopped; try again.".into());
            }
            Err(crossbeam_channel::TryRecvError::Empty) => {}
        }
    }
    ui.add_space(8.0);
    ui.label(format!("Saved endpoints · {scope}"));
    ui.small("Working endpoints are saved between launches. Search again to discover cameras; check saved endpoints to test their current availability.");
    let mut command = None;
    ui.add_enabled_ui(state.worker.is_none(), |ui| {
        command = controls(
            ui,
            state.snapshot.as_deref(),
            &mut state.filter,
            model.eyes_on_enabled,
        );
    });
    if let Some(error) = &state.error {
        ui.colored_label(egui::Color32::LIGHT_RED, format!("Camera cache: {error}"));
    }
    if let Some(message) = &state.message {
        ui.small(message);
    }
    if state.worker.is_some() {
        ui.spinner();
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    } else if command.is_none()
        && state
            .refreshed
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(2))
    {
        command = Some(Command::Load);
    }
    if let Some(command) = command {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let ctx = ui.ctx().clone();
        match std::thread::Builder::new()
            .name("camera-endpoint-settings".into())
            .spawn(move || {
                let result = run(command, &scope);
                let _ = tx.send(result);
                ctx.request_repaint();
            }) {
            Ok(_) => state.worker = Some(rx),
            Err(error) => {
                state.error = Some(error.to_string());
                state.refreshed = Some(Instant::now());
            }
        }
    }
    // Keep counts/check outcomes fresh while a directory worker is active.
    ui.ctx().request_repaint_after(Duration::from_secs(2));
    ui.data_mut(|d| d.insert_temp(id, state));
}

fn controls(
    ui: &mut egui::Ui,
    snapshot: Option<&Snapshot>,
    filter: &mut String,
    enabled: bool,
) -> Option<Command> {
    let mut command = None;
    let count = snapshot.map_or(0, |s| s.endpoints.len());
    let failed = snapshot.map_or(0, |s| s.endpoints.iter().filter(|e| !e.reachable).count());
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(enabled, egui::Button::new("Search again"))
            .clicked()
        {
            command = Some(Command::Discover);
        }
        if ui
            .add_enabled(
                enabled && count > 0,
                egui::Button::new("Check saved endpoints"),
            )
            .clicked()
        {
            command = Some(Command::Check);
        }
        if ui
            .add_enabled(failed > 0, egui::Button::new("Remove failed"))
            .clicked()
        {
            command = Some(Command::RemoveFailed);
        }
    });
    let Some(snapshot) = snapshot else {
        ui.small("Loading saved cameras…");
        return command;
    };
    ui.small(format!("{count} saved · {failed} failed at last check"));
    if let Some(at) = snapshot.scanned_at {
        ui.small(format!(
            "Last directory search: {} ago",
            endpoints::age_label(at)
        ));
    }
    if let Some(pending) = snapshot.pending {
        ui.small(match pending {
            Action::Discover => "Directory search requested.",
            _ => "Saved endpoint check requested.",
        });
    }
    if !snapshot.available {
        ui.small("The first search will save working cameras here.");
    }
    if count > 0 {
        ui.add(egui::TextEdit::singleline(filter).hint_text("Filter by camera name or address"));
        let needle = filter.to_ascii_lowercase();
        let entries: Vec<_> = snapshot
            .endpoints
            .iter()
            .filter(|e| {
                e.camera.label.to_ascii_lowercase().contains(&needle)
                    || e.camera.stream_url.to_ascii_lowercase().contains(&needle)
            })
            .collect();
        egui::ScrollArea::vertical().id_salt("saved-camera-list").max_height(200.0)
            .show_rows(ui, 68.0, entries.len(), |ui, range| {
                for index in range {
                    let entry = entries[index];
                    ui.push_id(&entry.camera.stream_url, |ui| {
                        ui.horizontal(|ui| {
                            if ui.small_button("Forget").on_hover_text("Remove only this saved endpoint. A future directory search can discover it again.").clicked() {
                                command = Some(Command::Forget(entry.camera.stream_url.clone()));
                            }
                            ui.label(&entry.camera.label);
                        });
                        ui.small(&entry.camera.stream_url);
                        ui.small(format!("{} · checked {} ago · verified {} ago",
                            if entry.reachable { "Previously working" } else { "Failed last check" },
                            endpoints::age_label(entry.checked_at), endpoints::age_label(entry.verified_at)));
                    });
                }
            });
    }
    command
}

fn run(command: Command, scope: &str) -> Result<Reply, String> {
    let store = Store::open_default()?;
    let mut removed = Vec::new();
    let message = match command {
        Command::Load => None,
        Command::Discover | Command::Check => {
            let action = if matches!(command, Command::Discover) {
                Action::Discover
            } else {
                Action::Check
            };
            store.request(scope, action).map_err(|e| e.to_string())?;
            camera_registry::invalidate();
            Some(
                if action == Action::Discover {
                    "Camera directory search requested."
                } else {
                    "Saved camera endpoint check requested."
                }
                .into(),
            )
        }
        Command::Forget(ref url) => {
            removed = store.remove(scope, Some(url)).map_err(|e| e.to_string())?;
            camera_registry::invalidate();
            Some("Saved camera endpoint removed.".into())
        }
        Command::RemoveFailed => {
            removed = store.remove(scope, None).map_err(|e| e.to_string())?;
            let count = removed.len();
            camera_registry::invalidate();
            Some(format!("Removed {count} failed saved camera endpoint(s)."))
        }
    };
    Ok(Reply {
        snapshot: Arc::new(store.snapshot(scope).map_err(|e| e.to_string())?),
        message,
        removed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_exposes_refresh_check_and_selective_removal() {
        let snapshot = Snapshot {
            available: true,
            endpoints: vec![endpoints::Endpoint {
                camera: crate::model::CameraFeed {
                    id: "eyes-on-1".into(),
                    label: "Example camera".into(),
                    provider: "test".into(),
                    kind: "snapshot".into(),
                    location: crate::model::GeoPoint { lat: 1.0, lon: 2.0 },
                    stream_url: "http://8.8.8.8/camera.jpg".into(),
                    last_seen: "saved".into(),
                    status: crate::model::CameraConnectionState::Unreachable,
                },
                verified_at: 1,
                checked_at: 1,
                reachable: false,
            }],
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    controls(ui, Some(&snapshot), &mut String::new(), true);
                });
            },
        );
        let labels: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text()),
                _ => None,
            })
            .collect();
        for expected in [
            "Search again",
            "Check saved endpoints",
            "Remove failed",
            "Forget",
            "Example camera",
        ] {
            assert!(labels.contains(&expected), "missing {expected}: {labels:?}");
        }
    }
}
