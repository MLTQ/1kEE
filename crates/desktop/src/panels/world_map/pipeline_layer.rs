//! Indexed public pipeline reads and projected meshes stay off the UI thread.
use crate::model::GeoPoint;
use egui::{Color32, Pos2, Rect};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};
use tile_archive::pipelines::{self, Feature, Filter};

#[derive(Clone, PartialEq)]
pub(super) struct Request {
    pub(super) root: PathBuf,
    pub(super) bounds: Option<[f32; 4]>,
}
#[derive(Clone, PartialEq)]
struct MeshKey {
    request: Request,
    projection: Vec<u32>,
    filter: Filter,
    colors: [Color32; 3],
}
pub(super) struct Snapshot {
    pub(super) request: Request,
    pub(super) features: Vec<Feature>,
    pub(super) status: String,
}
struct Hit {
    a: Pos2,
    b: Pos2,
    feature: usize,
}
struct Prepared {
    key: MeshKey,
    data: Arc<Snapshot>,
    mesh: Arc<egui::Mesh>,
    hits: Vec<Hit>,
}
#[derive(Default)]
struct Store {
    data: Option<Arc<Snapshot>>,
    prepared: Option<Arc<Prepared>>,
    working: bool,
    generation: u64,
}

fn store(local: bool) -> &'static Mutex<Store> {
    static LOCAL: OnceLock<Mutex<Store>> = OnceLock::new();
    static GLOBE: OnceLock<Mutex<Store>> = OnceLock::new();
    if local { &LOCAL } else { &GLOBE }.get_or_init(|| Mutex::new(Store::default()))
}

pub(super) fn load(request: Request) -> Snapshot {
    let result = (|| {
        let derived = crate::terrain_assets::find_derived_root(Some(&request.root))
            .unwrap_or_else(|| request.root.clone());
        let paths = [
            derived.join(pipelines::FILE_NAME),
            derived.join(tile_archive::FILE_NAME),
        ];
        let mut error = "No pipeline archive found".to_owned();
        for path in paths {
            if !path.is_file() {
                continue;
            }
            match tile_archive::Reader::open(&path)
                .and_then(|reader| pipelines::load(&reader, request.bounds))
            {
                Ok(features) => return Ok((features, path)),
                Err(e) => error = e,
            }
        }
        Err(error)
    })();
    match result {
        Ok((features, path)) => {
            let status = format!(
                "{} route parts · {}",
                features.len(),
                path.file_name().unwrap_or_default().to_string_lossy()
            );
            eprintln!("[1kEE] Pipelines: {status}");
            Snapshot {
                request,
                features,
                status,
            }
        }
        Err(error) => Snapshot {
            request,
            features: Vec::new(),
            status: error,
        },
    }
}

pub(super) fn draw(
    painter: &egui::Painter,
    root: &Path,
    bounds: Option<[f32; 4]>,
    projection: Vec<u32>,
    filter: Filter,
    project: impl Fn(GeoPoint) -> Option<Pos2> + Send + 'static,
) {
    let local = bounds.is_some();
    let bounds = bounds.map(|b| {
        [
            ((b[0] * 4.0).floor() / 4.0 - 0.5).max(-90.0),
            ((b[1] * 4.0).ceil() / 4.0 + 0.5).min(90.0),
            ((b[2] * 4.0).floor() / 4.0 - 0.5).max(-180.0),
            ((b[3] * 4.0).ceil() / 4.0 + 0.5).min(180.0),
        ]
    });
    let colors = [
        crate::theme::pipeline_gas_color(),
        crate::theme::pipeline_oil_color(),
        crate::theme::pipeline_other_color(),
    ];
    let key = MeshKey {
        request: Request {
            root: root.to_owned(),
            bounds,
        },
        projection,
        filter,
        colors,
    };
    let Ok(mut state) = store(local).try_lock() else {
        painter.ctx().request_repaint();
        return;
    };
    if !state.prepared.as_ref().is_some_and(|p| p.key == key) && !state.working {
        let cached = state.data.clone().filter(|s| s.request == key.request);
        if let Some(data) = cached {
            // Small local snapshots reproject immediately during panning. File
            // reads/decoding still happen on workers; idle meshes are reused.
            let (mesh, hits) = prepare(
                &data.features,
                key.filter,
                key.colors,
                painter.clip_rect(),
                &project,
            );
            state.prepared = Some(Arc::new(Prepared {
                key: key.clone(),
                data,
                mesh: Arc::new(mesh),
                hits,
            }));
        } else {
            state.working = true;
            let generation = state.generation;
            let key = key.clone();
            let rect = painter.clip_rect();
            let ctx = painter.ctx().clone();
            std::thread::spawn(move || {
                let data = Arc::new(load(key.request.clone()));
                let (mesh, hits) = prepare(&data.features, key.filter, key.colors, rect, project);
                let mut retired = None;
                if let Ok(mut state) = store(local).lock() {
                    if state.generation == generation {
                        let previous_data = state.data.replace(data.clone());
                        let previous_mesh = state.prepared.replace(Arc::new(Prepared {
                            key,
                            data,
                            mesh: Arc::new(mesh),
                            hits,
                        }));
                        retired = Some((previous_data, previous_mesh));
                    }
                    state.working = false;
                }
                drop(retired); // Large snapshots are freed outside the UI-facing mutex.
                ctx.request_repaint();
            });
        }
    }
    let prepared = state.prepared.clone().filter(|p| p.key == key);
    drop(state);
    let Some(prepared) = prepared else {
        painter.text(
            painter.clip_rect().left_bottom() + egui::vec2(12.0, -30.0),
            egui::Align2::LEFT_BOTTOM,
            "Loading pipelines…",
            egui::FontId::proportional(11.0),
            crate::theme::text_muted(),
        );
        return;
    };
    painter.add(egui::Shape::Mesh(prepared.mesh.clone()));
    if let Some(pointer) = super::infrastructure_hover::pointer() {
        for hit in &prepared.hits {
            let distance = super::infrastructure_hover::line_distance(pointer, hit.a, hit.b);
            if distance <= super::infrastructure_hover::RADIUS {
                super::infrastructure_hover::pipeline(
                    &prepared.data.features[hit.feature].info,
                    distance,
                );
            }
        }
    }
}

fn prepare(
    features: &[Feature],
    filter: Filter,
    colors: [Color32; 3],
    rect: Rect,
    project: impl Fn(GeoPoint) -> Option<Pos2>,
) -> (egui::Mesh, Vec<Hit>) {
    let mut mesh = egui::Mesh::default();
    let mut hits = Vec::new();
    for (index, f) in features
        .iter()
        .enumerate()
        .filter(|(_, f)| filter.accepts(&f.info))
    {
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
        let mut previous = None;
        for p in &f.points {
            let current = project(GeoPoint {
                lat: p.lat,
                lon: p.lon,
            })
            .filter(|p| p.x.is_finite() && p.y.is_finite());
            if let (Some(a), Some(b)) = (previous, current)
                && rect.intersects(Rect::from_two_pos(a, b))
            {
                let delta: egui::Vec2 = b - a;
                if delta.length_sq() > 0.001 {
                    let normal = egui::vec2(-delta.y, delta.x).normalized() * 0.75;
                    let base = mesh.vertices.len() as u32;
                    for pos in [a + normal, b + normal, b - normal, a - normal] {
                        mesh.colored_vertex(pos, color);
                    }
                    mesh.add_triangle(base, base + 1, base + 2);
                    mesh.add_triangle(base, base + 2, base + 3);
                    hits.push(Hit {
                        a,
                        b,
                        feature: index,
                    });
                }
            }
            // Never join across hidden/invalid globe points or disconnected parts.
            previous = current;
        }
    }
    (mesh, hits)
}

pub(crate) fn controls(ui: &mut egui::Ui, filter: &mut Filter) {
    ui.horizontal(|ui| {
        ui.checkbox(&mut filter.gas, "Gas");
        ui.checkbox(&mut filter.oil, "Oil");
        ui.checkbox(&mut filter.liquids, "Liquids / mixed");
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut filter.eia, "EIA");
        ui.checkbox(&mut filter.gem, "GEM global");
        ui.checkbox(&mut filter.bsee, "BSEE offshore");
    });
    ui.checkbox(&mut filter.planned, "Include planned / construction");
    ui.checkbox(&mut filter.historical, "Include inactive / historical");
    ui.small("Unspecified status remains visible. Sources can overlap; route accuracy varies. Hover a route in either view for details.");
    if let Some(status) = super::pipeline_globe::status() {
        ui.small(status);
    }
    for local in [true] {
        if let Ok(state) = store(local).lock()
            && let Some(data) = &state.data
        {
            ui.small(&data.status);
        }
    }
    if ui.small_button("Reload pipeline archive").clicked() {
        super::pipeline_globe::reload();
        for local in [false, true] {
            if let Ok(mut state) = store(local).lock() {
                state.generation += 1;
                state.data = None;
                state.prepared = None;
            }
        }
    }
    ui.horizontal_wrapped(|ui| {
        ui.hyperlink_to("EIA / DOE", "https://arcgis.netl.doe.gov/server/rest/services/Hosted/EIA_pipeline_data/FeatureServer");
        ui.hyperlink_to("BSEE", "https://www.data.bsee.gov/Main/Mapping.aspx");
        ui.hyperlink_to("Global Energy Monitor · CC BY 4.0", "https://globalenergymonitor.org/creative-commons-license");
    });
    ui.small(
        "GEM gas: Nov 2025 · oil: Jun 2026 public maps. Routes clipped; globe overview simplified.",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hidden_projection_breaks_lines_instead_of_connecting_across_globe() {
        let info = serde_json::from_value(serde_json::json!({"source":"eia-3","source_id":"1","name":"test","operator":"","owner":"","product":"gas","status":"unknown","accuracy":"unknown","historical":false,"planned":false})).unwrap();
        let features = vec![Feature {
            info,
            points: vec![
                cell_format::CellPoint { lon: 0.0, lat: 0.0 },
                cell_format::CellPoint { lon: 1.0, lat: 0.0 },
                cell_format::CellPoint { lon: 2.0, lat: 0.0 },
            ],
        }];
        let (mesh, _) = prepare(
            &features,
            Filter::default(),
            [Color32::WHITE; 3],
            Rect::EVERYTHING,
            |p| (p.lon != 1.0).then_some(Pos2::new(p.lon, 0.0)),
        );
        assert!(mesh.vertices.is_empty());
    }
}
