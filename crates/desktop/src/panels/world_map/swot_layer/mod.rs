//! Offline SWOT marine gravity imagery and its source controls.
mod globe;
mod grid;
mod local;
mod viewport;

use super::layer_snapshot::Snapshot;
use crate::model::{ActiveBody, AppModel};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

pub(super) use globe::draw as draw_globe;
pub(super) use local::draw as draw_local;

#[derive(Clone, PartialEq, Eq)]
struct Request {
    root: Option<PathBuf>,
    region: Option<grid::Region>,
    max_edge: usize,
}

struct LoadedImage {
    texture: egui::TextureHandle,
    region: Option<grid::Region>,
}

#[derive(Clone, Copy)]
enum Scene {
    Overview,
    GlobeDetail,
    Local,
}

#[derive(Default)]
struct Cache {
    image: Snapshot<Request, Result<LoadedImage, String>>,
    status: String,
}

static GLOBE: OnceLock<Mutex<Cache>> = OnceLock::new();
static LOCAL: OnceLock<Mutex<Cache>> = OnceLock::new();
static DETAIL: OnceLock<Mutex<Cache>> = OnceLock::new();

fn cache(scene: Scene) -> &'static Mutex<Cache> {
    match scene {
        Scene::Overview => &GLOBE,
        Scene::GlobeDetail => &DETAIL,
        Scene::Local => &LOCAL,
    }
    .get_or_init(Default::default)
}

fn texture(
    ctx: &egui::Context,
    root: Option<&Path>,
    region: Option<grid::Region>,
    scene: Scene,
) -> Option<Arc<Result<LoadedImage, String>>> {
    let request = Request {
        root: root.map(Path::to_path_buf),
        region,
        max_edge: ctx.input(|i| i.max_texture_side).clamp(
            1,
            match scene {
                Scene::Local => 1024,
                _ => 4096,
            },
        ),
    };
    let wake = ctx.clone();
    let mut cache = cache(scene).lock().ok()?;
    let image = cache.image.get(
        request,
        "swot-vgg-image",
        // Keep a correctly georeferenced prior window while the next loads.
        // Never retain imagery from a different data root.
        |a, b| a.root == b.root,
        move |request| {
            let result = (|| {
                let derived = crate::terrain_assets::find_derived_root(request.root.as_deref())
                    .ok_or_else(|| {
                        "Prepare SWOT data in the selected Derived folder first.".to_string()
                    })?;
                let image =
                    grid::read_image(&derived.join("swot"), request.region, request.max_edge)?;
                Ok(LoadedImage {
                    texture: wake.load_texture("swot-vgg", image, egui::TextureOptions::LINEAR),
                    region: request.region,
                })
            })();
            // Cache failures until a root change or layer/cache reset;
            // never re-open a missing file each frame.
            Some(result)
        },
    );
    cache.status = match image.as_deref() {
        Some(Ok(_)) => "SWOT_06 · September 2026 · loaded".into(),
        Some(Err(error)) => error.clone(),
        None => "Loading SWOT gravity…".into(),
    };
    if image.is_none() {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
    image
}

pub(super) fn clear() {
    for state in [&GLOBE, &LOCAL, &DETAIL] {
        if let Some(cache) = state.get()
            && let Ok(mut cache) = cache.lock()
        {
            cache.image.clear();
            cache.status.clear();
        }
    }
    globe::clear();
}

pub(crate) fn controls(ui: &mut egui::Ui, model: &mut AppModel) {
    let response = ui.add_enabled(model.active_body == ActiveBody::Earth,
        egui::Checkbox::new(&mut model.show_swot_gravity, "SWOT marine gravity"))
        .on_hover_text("Satellite-derived vertical gravity gradient reveals seafloor structure. Values are gravity variation, measured in Eötvös.")
        .on_disabled_hover_text("Available on Earth");
    if response.changed() && !model.show_swot_gravity {
        clear();
    }
    if !model.show_swot_gravity {
        return;
    }
    ui.indent("swot-vgg-controls", |ui| {
        ui.small("Vertical gravity gradient (Eötvös)");
        ui.horizontal(|ui| {
            for (value, label) in [(-200, "−20"), (0, "0"), (200, "+20")] {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 8.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 0.0, grid::color(value));
                ui.small(label);
            }
        });
        ui.small("Color scale clips at ±20 E; not depth.");
        if let Ok(cache) = cache(if model.globe_view.local_mode {
            Scene::Local
        } else {
            Scene::Overview
        })
        .lock()
        {
            ui.small(if cache.status.is_empty() {
                "Enable to load prepared data."
            } else {
                &cache.status
            });
        }
    });
}
