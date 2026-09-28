use super::{
    contour_pass::{ContourCallback, ContourLayer, SegmentInstance},
    globe_scene::GlobeLayout,
    pipeline_layer::{Request, Snapshot, load},
};
use crate::model::{GeoPoint, GlobeViewState};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};
use tile_archive::pipelines::Filter;

#[derive(Clone, PartialEq)]
struct Key {
    root: PathBuf,
    filter: Filter,
    colors: [egui::Color32; 3],
}
struct Batch {
    key: Key,
    version: u64,
    instances: Arc<Vec<SegmentInstance>>,
}
#[derive(Default)]
struct Store {
    data: Option<Arc<Snapshot>>,
    batch: Option<Arc<Batch>>,
    working: bool,
    generation: u64,
}
fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(Store::default()))
}

pub(super) fn status() -> Option<String> {
    store()
        .try_lock()
        .ok()?
        .data
        .as_ref()
        .map(|d| d.status.clone())
}
pub(super) fn reload() {
    if let Ok(mut state) = store().try_lock() {
        state.generation += 1;
        let retired = (state.data.take(), state.batch.take());
        std::thread::spawn(move || drop(retired));
    }
}

pub(super) fn draw(
    painter: &egui::Painter,
    root: &Path,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    filter: Filter,
) {
    let key = Key {
        root: root.to_owned(),
        filter,
        colors: [
            crate::theme::pipeline_gas_color(),
            crate::theme::pipeline_oil_color(),
            crate::theme::pipeline_other_color(),
        ],
    };
    let Ok(mut state) = store().try_lock() else {
        painter.ctx().request_repaint();
        return;
    };
    if !state.batch.as_ref().is_some_and(|b| b.key == key) && !state.working {
        state.working = true;
        let cached = state.data.clone().filter(|d| d.request.root == key.root);
        let generation = state.generation;
        let key = key.clone();
        let ctx = painter.ctx().clone();
        std::thread::spawn(move || {
            static VERSION: AtomicU64 = AtomicU64::new(1);
            let data = cached.unwrap_or_else(|| {
                Arc::new(load(Request {
                    root: key.root.clone(),
                    bounds: None,
                }))
            });
            let instances = prepare(&data.features, key.filter, key.colors);
            let batch = Arc::new(Batch {
                key,
                version: VERSION.fetch_add(1, Ordering::Relaxed),
                instances: Arc::new(instances),
            });
            let mut retired = None;
            if let Ok(mut state) = store().lock() {
                if state.generation == generation {
                    retired = Some((state.data.replace(data), state.batch.replace(batch)));
                }
                state.working = false;
            }
            drop(retired);
            ctx.request_repaint();
        });
    }
    let batch = state.batch.clone().filter(|b| b.key == key);
    drop(state);
    if let Some(batch) = batch {
        painter.add(
            ContourCallback::new(
                ContourLayer::Pipelines,
                batch.version,
                batch.instances.clone(),
                layout,
                view,
                0.0,
                1.0,
                1.5,
                painter.ctx().pixels_per_point(),
            )
            .into_paint_callback(painter.clip_rect()),
        );
    } else {
        painter.text(
            painter.clip_rect().left_bottom() + egui::vec2(12.0, -30.0),
            egui::Align2::LEFT_BOTTOM,
            "Loading pipelines…",
            egui::FontId::proportional(11.0),
            crate::theme::text_muted(),
        );
    }
}

fn prepare(
    features: &[tile_archive::pipelines::Feature],
    filter: Filter,
    colors: [egui::Color32; 3],
) -> Vec<SegmentInstance> {
    let mut instances = Vec::new();
    for f in features.iter().filter(|f| filter.accepts(&f.info)) {
        let mut color = colors[match f.info.product.as_str() {
            "gas" => 0,
            "oil" => 1,
            _ => 2,
        }];
        if f.info.historical {
            color = color.gamma_multiply(0.35);
        } else if f.info.planned {
            color = color.gamma_multiply(0.60);
        }
        for pair in f.points.windows(2) {
            if pair[0] != pair[1] {
                instances.push(SegmentInstance::line(
                    GeoPoint {
                        lat: pair[0].lat,
                        lon: pair[0].lon,
                    },
                    GeoPoint {
                        lat: pair[1].lat,
                        lon: pair[1].lon,
                    },
                    color,
                ));
            }
        }
    }
    instances
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "read-only real archive GPU batch check: set ONEKEE_PIPELINE_ARCHIVE"]
    fn real_pipeline_globe_batch() {
        let path = std::env::var("ONEKEE_PIPELINE_ARCHIVE").unwrap();
        let reader = tile_archive::Reader::open(Path::new(&path)).unwrap();
        let features = tile_archive::pipelines::load(&reader, None).unwrap();
        let start = std::time::Instant::now();
        let instances = prepare(&features, Filter::default(), [egui::Color32::WHITE; 3]);
        let expected: usize = features
            .iter()
            .filter(|f| Filter::default().accepts(&f.info))
            .map(|f| f.points.windows(2).filter(|p| p[0] != p[1]).count())
            .sum();
        assert_eq!(instances.len(), expected);
        assert!(expected > 0);
        println!(
            "Pipeline globe: {expected} GPU segments, {} bytes, {:?} one-time preparation",
            instances.len() * std::mem::size_of::<SegmentInstance>(),
            start.elapsed()
        );
    }
}
