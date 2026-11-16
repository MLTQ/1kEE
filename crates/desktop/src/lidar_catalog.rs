//! One sidebar layer: worldwide survey coverage on Globe, fused measurements in Local.
use crate::model::{ActiveBody, AppModel};
use crate::{
    lidar_tiles::{Tiles, View},
    settings_store,
};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    time::{Duration, Instant},
};
const HARVESTER: &str = include_str!("../../../scripts/lidar_catalog.py");

pub(super) type ReadJob<T> = mpsc::Receiver<Result<T, String>>;

pub(super) fn read_in_background<T: Send + 'static>(
    ctx: &egui::Context,
    read: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> ReadJob<T> {
    let (tx, rx) = mpsc::channel();
    let ctx = ctx.clone();
    let _ = std::thread::Builder::new()
        .name("lidar-catalog".into())
        .spawn(move || {
            let _ = tx.send(read());
            ctx.request_repaint();
        });
    rx
}

pub(super) fn completed<T>(job: &mut Option<ReadJob<T>>) -> Option<Result<T, String>> {
    let result = match job.as_ref()?.try_recv() {
        Ok(result) => result,
        Err(mpsc::TryRecvError::Empty) => return None,
        Err(mpsc::TryRecvError::Disconnected) => Err("Lidar catalog worker stopped".into()),
    };
    *job = None;
    Some(result)
}

#[derive(Default)]
pub(crate) struct Layer {
    pub(crate) enabled: bool,
    pub(crate) tiles: Tiles,
    path: PathBuf,
    last_check: Option<Instant>,
    work: Option<ReadJob<String>>,
    status: String,
}
impl Layer {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            ..Default::default()
        }
    }
    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.tiles.stop();
        }
    }
    pub(crate) fn set_path(&mut self, path: PathBuf) {
        if self.path != path {
            *self = Self {
                enabled: self.enabled,
                ..Self::new(path)
            };
        }
    }
}

pub(crate) fn db_path(data_root: Option<&str>, asset_root: Option<&Path>) -> PathBuf {
    data_root
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| asset_root.unwrap_or(Path::new(".")).join("Data"))
        .join("Lidar/catalog.sqlite")
}

fn python(db: &Path) -> Command {
    let mut cmd = Command::new(if cfg!(windows) { "python" } else { "python3" });
    cmd.arg("-c").arg(HARVESTER).arg("--db").arg(db);
    gdal_path(&mut cmd);
    cmd
}

pub(crate) fn gdal_path(cmd: &mut Command) -> &mut Command {
    if let Some(folder) = settings_store::resolve_gdal_tool("gdalwarp")
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        let paths = std::env::var_os("PATH").unwrap_or_default();
        if let Ok(path) = std::env::join_paths(
            std::iter::once(folder.to_owned()).chain(std::env::split_paths(&paths)),
        ) {
            cmd.env("PATH", path);
        }
    }
    cmd
}

fn run(cmd: &mut Command) -> Result<String, String> {
    let o = cmd
        .output()
        .map_err(|e| format!("Cannot run lidar harvester: {e}"))?;
    let text = [o.stdout, o.stderr]
        .map(|s| String::from_utf8_lossy(&s).trim().to_owned())
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if o.status.success() {
        Ok(text)
    } else {
        Err(text)
    }
}

fn check(ctx: &egui::Context, browser: &mut Layer) {
    browser.last_check = Some(Instant::now());
    browser.status = "Checking survey coverage…".into();
    let db = browser.path.clone();
    browser.work = Some(read_in_background(ctx, move || {
        run(python(&db).arg("refresh"))
    }));
}

pub(crate) fn tick(ctx: &egui::Context, model: &mut AppModel) {
    let b = &mut model.lidar;
    let refreshed = completed(&mut b.work);
    let force = refreshed.is_some();
    if let Some(result) = refreshed {
        b.status = result.unwrap_or_else(|error| error);
    }
    if !b.enabled || model.active_body != ActiveBody::Earth {
        b.tiles.stop();
        return;
    }
    if b.work.is_none()
        && b.last_check
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(86400))
    {
        check(ctx, b);
    }
    if b.tiles.poll_catalog(ctx, &b.path, force) {
        let view = if model.globe_view.local_mode {
            View::Fused
        } else {
            View::Coverage
        };
        b.tiles.tick(ctx, &b.path, view);
    }
}

pub(crate) fn toggle(ui: &mut egui::Ui, model: &mut AppModel) {
    if model.active_body != ActiveBody::Earth {
        return;
    }
    let mut enabled = model.lidar.enabled;
    if ui
        .checkbox(&mut enabled, "Lidar")
        .on_hover_text("Globe: all survey boundaries. Local: fused lidar data. USGS coverage is prepared in the background when you visit.")
        .changed()
    {
        model.lidar.set_enabled(enabled);
    }
    if model.lidar.enabled {
        let status = if !model.lidar.tiles.status.is_empty() {
            &model.lidar.tiles.status
        } else {
            &model.lidar.status
        };
        if !status.is_empty() {
            ui.small(status).on_hover_text(&model.lidar.tiles.sources);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_sidebar_toggle_survives_view_changes_without_moving_the_camera() {
        let ctx = egui::Context::default();
        let mut model = AppModel::new();
        let center = model.globe_view.local_center;
        let frame = |model: &mut AppModel, events| {
            ctx.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| toggle(ui, model));
                },
            )
        };
        for enabled in [true, false] {
            let output = frame(&mut model, Vec::new());
            let labels = output
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) => Some((t.galley.text(), t.pos + t.galley.size() / 2.)),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                labels.iter().filter(|(text, _)| *text == "Lidar").count(),
                1
            );
            assert!(
                !labels
                    .iter()
                    .any(|(text, _)| text.contains("Search") || text.contains("Browse"))
            );
            let pos = labels.iter().find(|(text, _)| *text == "Lidar").unwrap().1;
            for pressed in [true, false] {
                let _ = frame(
                    &mut model,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            pressed,
                            button: egui::PointerButton::Primary,
                            modifiers: Default::default(),
                        },
                    ],
                );
            }
            assert_eq!(model.lidar.enabled, enabled);
            assert_eq!(model.globe_view.local_center, center);
            assert!(!model.show_camera_markers);
            for local in [true, false] {
                model.globe_view.local_mode = local;
                let _ = frame(&mut model, Vec::new());
                assert_eq!(model.lidar.enabled, enabled);
            }
        }
        model.active_body = ActiveBody::Moon;
        let output = frame(&mut model, Vec::new());
        assert!(
            !output
                .shapes
                .iter()
                .any(|s| matches!(&s.shape,egui::Shape::Text(t) if t.galley.text()=="Lidar"))
        );
    }
}
