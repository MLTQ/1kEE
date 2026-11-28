//! Fused highest-density LiDAR point cloud sidebar layer.
//!
//! Replaces modal Target mode with an always-available sidebar layer that
//! progressively streams coverage-first merged USGS EPT + OpenTopography bulk
//! LAS/LAZ surveys directly onto the unified continuous 3D map (default point
//! size `1` px).

use super::point_cloud_pass::{PointCloudCallback, RtcPointBatch, RtcPointVertex};
use super::projection::{MapProjection, ecef_to_tile_local, geo_to_ecef};
use crate::model::{ActiveBody, AppModel};
use crate::target_mode::{heatmap::HeightRange, project};
use lidar_pipeline::{
    ept::point_cloud::{self, Event},
    region::Region,
};
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
    mpsc,
};

/// Default point size requested by operator: crisp 1-pixel points.
pub const DEFAULT_POINT_SIZE_PX: f32 = 1.0;

/// Maximum viewport half-height (metres) at which automatic point-cloud streaming activates.
const MAX_STREAM_HALF_H_M: f64 = 15_000.0;

#[derive(Clone, Debug)]
pub struct PointCloudLayerState {
    pub enabled: bool,
    pub point_size_px: f32,
    /// 0 = Class, 1 = Height, 2 = RGB (default, fallback Class), 3 = Intensity, 4 = Elevation Heatmap
    pub color_mode: u32,
    pub class_mask: u32,
    pub budget_millions: usize,
}

impl Default for PointCloudLayerState {
    fn default() -> Self {
        Self {
            enabled: false,
            point_size_px: DEFAULT_POINT_SIZE_PX,
            color_mode: 2,
            class_mask: !0u32,
            budget_millions: 8,
        }
    }
}

impl PointCloudLayerState {
    pub fn status_summary(&self) -> String {
        if !self.enabled {
            return "Off".into();
        }
        runtime()
            .lock()
            .map(|rt| {
                if rt.status.is_empty() {
                    if rt.point_count > 0 {
                        format!("{:.1}M pts", rt.point_count as f64 / 1e6)
                    } else {
                        "Zoom < 15 km".into()
                    }
                } else {
                    rt.status.clone()
                }
            })
            .unwrap_or_else(|_| "Ready".into())
    }

    pub fn loaded_point_count(&self) -> usize {
        runtime().lock().map(|rt| rt.point_count).unwrap_or(0)
    }
}

enum WorkerMsg {
    Progress(String),
    Batch(RtcPointBatch, HeightRange),
    Done(Result<String, String>),
}

struct StreamRuntime {
    loaded_region: Option<Region>,
    loaded_budget: usize,
    loaded_root: Option<PathBuf>,
    batches: Vec<RtcPointBatch>,
    point_count: usize,
    elev_range: HeightRange,
    status: String,
    rx: Option<mpsc::Receiver<WorkerMsg>>,
    _cancel: Option<tempfile::NamedTempFile>,
    replacing: bool,
    generation: u64,
}

impl Default for StreamRuntime {
    fn default() -> Self {
        Self {
            loaded_region: None,
            loaded_budget: 0,
            loaded_root: None,
            batches: Vec::new(),
            point_count: 0,
            elev_range: HeightRange::EMPTY,
            status: String::new(),
            rx: None,
            _cancel: None,
            replacing: false,
            generation: 0,
        }
    }
}

fn runtime() -> &'static Mutex<StreamRuntime> {
    static RT: OnceLock<Mutex<StreamRuntime>> = OnceLock::new();
    RT.get_or_init(|| Mutex::new(StreamRuntime::default()))
}

fn next_batch_id() -> u64 {
    static ID: AtomicU64 = AtomicU64::new(1);
    ID.fetch_add(1, Ordering::Relaxed)
}

pub fn clear() {
    if let Ok(mut rt) = runtime().lock() {
        rt.loaded_region = None;
        rt.batches.clear();
        rt.point_count = 0;
        rt.elev_range = HeightRange::EMPTY;
        rt.status.clear();
        rt.rx = None;
        rt._cancel = None;
        rt.replacing = false;
        rt.generation = rt.generation.wrapping_add(1);
    }
}

/// Sidebar layer toggle and compact controls for `layer_drawer.rs`.
pub(crate) fn controls(ui: &mut egui::Ui, model: &mut AppModel) {
    let available = model.active_body == ActiveBody::Earth;
    let resp = ui.add_enabled(
        available,
        egui::Checkbox::new(&mut model.point_cloud.enabled, "LiDAR Point Cloud (Fused)"),
    );
    resp.on_hover_text(
        "Streams fused highest-density USGS EPT & OpenTopography point clouds directly on the 3D map (default 1 px points).",
    );
    if !model.point_cloud.enabled || !available {
        return;
    }

    ui.indent("lidar_point_cloud_controls", |ui| {
        ui.horizontal(|ui| {
            ui.label("Point size");
            ui.add(
                egui::Slider::new(&mut model.point_cloud.point_size_px, 1.0..=6.0)
                    .step_by(0.5)
                    .suffix(" px"),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Color");
            egui::ComboBox::from_id_salt("pc_color_mode")
                .selected_text(match model.point_cloud.color_mode {
                    0 => "Class",
                    1 => "Height",
                    2 => "RGB / Class",
                    3 => "Intensity",
                    _ => "Heatmap",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut model.point_cloud.color_mode, 2, "RGB / Class");
                    ui.selectable_value(&mut model.point_cloud.color_mode, 4, "Heatmap");
                    ui.selectable_value(&mut model.point_cloud.color_mode, 0, "Class");
                    ui.selectable_value(&mut model.point_cloud.color_mode, 1, "Height");
                    ui.selectable_value(&mut model.point_cloud.color_mode, 3, "Intensity");
                });
        });
        ui.horizontal(|ui| {
            ui.label("Budget");
            ui.add(
                egui::Slider::new(&mut model.point_cloud.budget_millions, 1..=16)
                    .suffix("M pts"),
            );
        });
        ui.horizontal_wrapped(|ui| {
            for (bit, label) in [(2u32, "Ground"), (3, "Veg"), (6, "Bldg"), (9, "Water")] {
                let mask = if bit == 3 {
                    (1 << 3) | (1 << 4) | (1 << 5)
                } else {
                    1 << bit
                };
                let mut on = (model.point_cloud.class_mask & mask) != 0;
                if ui.checkbox(&mut on, label).changed() {
                    if on {
                        model.point_cloud.class_mask |= mask;
                    } else {
                        model.point_cloud.class_mask &= !mask;
                    }
                }
            }
        });
        if let Ok(rt) = runtime().try_lock() {
            if !rt.status.is_empty() {
                ui.small(&rt.status);
            }
            if rt.point_count > 0 && ui.small_button("Reload area").clicked() {
                drop(rt);
                clear();
            }
        }
    });
}

fn region_covers_view(loaded: Region, target_lat: f64, target_lon: f64, want_radius_m: f64) -> bool {
    let [dx, dy] = loaded.offset(target_lon, target_lat);
    let dist_m = dx.hypot(dy);
    // Keep current point cloud while camera stays comfortably inside it and resolution hasn't zoomed >2.5x closer.
    dist_m + want_radius_m * 0.45 <= loaded.radius_m
        && loaded.radius_m <= want_radius_m * 2.5
}

fn point_to_rtc(p: &point_cloud::Point, origin_ecef: glam::DVec3, radius_m: f64) -> RtcPointVertex {
    let sea_ecef = geo_to_ecef(p.lon_lat[1], p.lon_lat[0], 0.0, radius_m);
    let local = ecef_to_tile_local(sea_ecef, origin_ecef);
    let mut color = match p.classification {
        2 => [180, 154, 114, 255],
        3..=5 => [76, 181, 133, 255],
        6 => [220, 168, 105, 255],
        9 => [75, 163, 219, 255],
        _ => [111, 198, 213, 255],
    };
    if let Some(rgb) = p.rgb {
        color = [
            (rgb[0] >> 8) as u8,
            (rgb[1] >> 8) as u8,
            (rgb[2] >> 8) as u8,
            255,
        ];
    }
    let packed = u32::from(p.classification)
        | (u32::from(p.intensity) << 8)
        | (u32::from(p.return_number.min(15)) << 24)
        | if p.rgb.is_some() { 1 << 31 } else { 0 };
    RtcPointVertex {
        local_elev: [local.x, local.y, local.z, p.source_xyz[2] as f32],
        color,
        packed,
    }
}

/// Poll active fused LiDAR point-cloud worker, trigger viewport-driven loads when
/// zoomed in, and submit `PointCloudCallback` into `UnifiedSceneTarget`.
pub(super) fn draw_unified(
    painter: &egui::Painter,
    proj: &MapProjection,
    model: &AppModel,
) {
    if !model.point_cloud.enabled || model.active_body != ActiveBody::Earth {
        return;
    }

    let Ok(mut rt) = runtime().lock() else {
        return;
    };

    // 1. Drain incoming batches from background worker
    for _ in 0..6 {
        let msg = rt.rx.as_ref().map(|rx| rx.try_recv());
        match msg {
            Some(Ok(WorkerMsg::Progress(text))) => rt.status = text,
            Some(Ok(WorkerMsg::Batch(batch, range))) => {
                if rt.replacing {
                    rt.batches.clear();
                    rt.point_count = 0;
                    rt.elev_range = HeightRange::EMPTY;
                    rt.replacing = false;
                }
                rt.point_count += batch.points.len();
                rt.elev_range = rt.elev_range.union(range);
                rt.batches.push(batch);
                rt.status = format!("Streaming · {} points", rt.point_count);
            }
            Some(Ok(WorkerMsg::Done(res))) => {
                rt.status = match res {
                    Ok(summary) => summary,
                    Err(err) => err,
                };
                rt.rx = None;
                rt._cancel = None;
                break;
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                rt.rx = None;
                rt._cancel = None;
                break;
            }
            _ => break,
        }
    }

    // 2. Decide whether to stream a new viewport region
    let root = model.selected_root.clone();
    if proj.half_h_m <= MAX_STREAM_HALF_H_M {
        let want_radius = (proj.half_h_m.hypot(proj.half_w_m) * 1.15).clamp(120.0, 20_000.0);
        let budget_pts = model.point_cloud.budget_millions.clamp(1, 16) * 1_000_000;
        let needs_load = rt.loaded_root != root
            || rt.loaded_budget != budget_pts
            || rt.loaded_region.is_none_or(|r| {
                !region_covers_view(r, proj.target_geo.lat, proj.target_geo.lon, want_radius)
            });

        if needs_load {
            let region = Region {
                lon: proj.target_geo.lon,
                lat: proj.target_geo.lat,
                radius_m: want_radius,
            };
            let catalog = crate::lidar_catalog::db_path(
                crate::settings_store::configured_data_root()
                    .as_ref()
                    .and_then(|p| p.to_str()),
                root.as_deref(),
            );
            rt.loaded_region = Some(region);
            rt.loaded_budget = budget_pts;
            rt.loaded_root = root;
            rt.replacing = true;
            rt.status = "Discovering highest-density LiDAR surveys…".into();
            rt.rx = None;
            rt._cancel = None;

            if let Ok(cancel) = tempfile::NamedTempFile::new() {
                let marker = cancel.path().to_owned();
                let ctx = painter.ctx().clone();
                let (tx, rx) = mpsc::sync_channel(4);
                let radius_m = ActiveBody::Earth.radius_m();
                let origin_ecef = geo_to_ecef(region.lat, region.lon, 0.0, radius_m);

                if std::thread::Builder::new()
                    .name("lidar-point-cloud-layer".into())
                    .spawn(move || {
                        let result = point_cloud::merge::load(
                            &catalog,
                            region,
                            None, // None = coverage-first multi-survey merge (highest density prioritized)
                            Some(&marker),
                            budget_pts,
                            |crs, pts| project::xy(crs, pts, &marker),
                            |event| {
                                use point_cloud::merge::MergeEvent;
                                match event {
                                    MergeEvent::Sources(sources) => {
                                        let _ = tx.send(WorkerMsg::Progress(format!(
                                            "Found {} LiDAR survey(s)…",
                                            sources.len()
                                        )));
                                    }
                                    MergeEvent::Reading { source, refining } => {
                                        let _ = tx.send(WorkerMsg::Progress(format!(
                                            "{} · {}",
                                            if refining { "Refining" } else { "Loading" },
                                            source.name
                                        )));
                                    }
                                    MergeEvent::Data(Event::Header(_)) => {}
                                    MergeEvent::Data(Event::Batch { points, .. }) => {
                                        let mut range = HeightRange::EMPTY;
                                        let mut verts = Vec::with_capacity(points.len());
                                        for p in &points {
                                            range.include(p.source_xyz[2] as f32);
                                            verts.push(point_to_rtc(p, origin_ecef, radius_m));
                                        }
                                        let batch = RtcPointBatch {
                                            id: next_batch_id(),
                                            origin_ecef,
                                            points: Arc::new(verts),
                                        };
                                        tx.send(WorkerMsg::Batch(batch, range))
                                            .map_err(|_| "Layer closed".to_string())?;
                                    }
                                }
                                ctx.request_repaint();
                                Ok(())
                            },
                        );
                        let done = match result {
                            Ok(report) => Ok(format!(
                                "{} pts fused",
                                report.total.points,
                            )),
                            Err(err) => Err(err),
                        };
                        let _ = tx.send(WorkerMsg::Done(done));
                        ctx.request_repaint();
                    })
                    .is_ok()
                {
                    rt._cancel = Some(cancel);
                    rt.rx = Some(rx);
                }
            }
        }
    } else if rt.batches.is_empty() && rt.rx.is_none() {
        rt.status = "Zoom in below ~30 km to stream fused LiDAR points".into();
    }

    if rt.rx.is_some() {
        painter
            .ctx()
            .request_repaint_after(std::time::Duration::from_millis(80));
    }

    // 3. Submit resident batches into shared Reversed-Z scene target
    if !rt.batches.is_empty() {
        let elev_range = if rt.elev_range.low <= rt.elev_range.high {
            [rt.elev_range.low, rt.elev_range.high]
        } else {
            [0.0, 100.0]
        };
        painter.add(
            PointCloudCallback {
                proj: *proj,
                batches: rt.batches.clone(),
                point_size_px: model.point_cloud.point_size_px,
                class_mask: model.point_cloud.class_mask,
                color_mode: model.point_cloud.color_mode,
                elev_range,
            }
            .into_paint_callback(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_point_cloud_layer_uses_one_pixel_points_and_sidebar_toggle() {
        let state = PointCloudLayerState::default();
        assert!(!state.enabled);
        assert_eq!(state.point_size_px, 1.0);
        assert_eq!(state.color_mode, 2);
        assert_eq!(state.class_mask, !0u32);
        assert_eq!(state.status_summary(), "Off");
    }
}
