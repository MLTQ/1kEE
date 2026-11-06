//! Optional NCEI free-air, Bouguer and isostatic anomaly maps.
mod controls;
mod grid;
mod render;
pub(crate) use controls::controls;
pub(super) use render::{draw_globe, draw_local};

use super::layer_snapshot::Snapshot;
use crate::model::gravity::gravity_maps;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone, PartialEq, Eq)]
struct Request {
    root: Option<PathBuf>,
    id: String,
    limit: usize,
}
struct Loaded {
    texture: egui::TextureHandle,
    bounds: [f32; 4],
}
#[derive(Default)]
struct Cache {
    image: Snapshot<Request, Result<Loaded, String>>,
    status: String,
}
static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

fn texture(ctx: &egui::Context, root: Option<&Path>, id: &str) -> Option<Arc<Result<Loaded, String>>> {
    let request = Request {
        root: root.map(Path::to_path_buf),
        id: id.into(),
        limit: ctx.input(|i| i.max_texture_side).clamp(1, 4096),
    };
    let wake = ctx.clone();
    let mut cache = CACHE.get_or_init(Default::default).lock().ok()?;
    let result = cache.image.get(
        request,
        "gravity-anomaly",
        |a, b| a == b,
        move |request| {
            Some((|| {
                let root = crate::terrain_assets::find_derived_root(request.root.as_deref())
                    .ok_or_else(|| "Mount the gravity data drive first.".to_string())?;
                let kind = gravity_maps()
                    .iter()
                    .find(|m| m.id == request.id)
                    .ok_or_else(|| "Unknown gravity product".to_string())?
                    .kind;
                let image = grid::read(&root, &request.id, request.limit, kind.color_limit())?;
                Ok(Loaded {
                    texture: wake.load_texture(
                        "gravity-anomaly",
                        image.image,
                        egui::TextureOptions::LINEAR,
                    ),
                    bounds: image.bounds,
                })
            })())
        },
    );
    cache.status = match result.as_deref() {
        Some(Ok(_)) => "Loaded · outside coverage stays clear".into(),
        Some(Err(error)) => error.clone(),
        None => "Loading gravity map…".into(),
    };
    if result.is_none() {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    result
}

pub(super) fn clear() {
    if let Some(cache) = CACHE.get()
        && let Ok(mut cache) = cache.lock()
    {
        cache.image.clear();
        cache.status.clear();
    }
    render::clear();
}
