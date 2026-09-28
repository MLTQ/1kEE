//! Asynchronous static inventory reads and cached, batched screen markers.
use crate::model::GeoPoint;
use egui::{Color32, Pos2, Rect};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, mpsc},
};
use tile_archive::platforms::{self, Filter, Inventory};

struct Snapshot {
    inventory: Option<Inventory>,
    status: String,
}
#[derive(Default)]
struct Source {
    root: Option<PathBuf>,
    pending: Option<mpsc::Receiver<Snapshot>>,
    snapshot: Option<Arc<Snapshot>>,
    revision: u64,
}
fn source() -> &'static Mutex<Source> {
    static SOURCE: OnceLock<Mutex<Source>> = OnceLock::new();
    SOURCE.get_or_init(|| Mutex::new(Source::default()))
}
fn load(root: &Path) -> Snapshot {
    let derived =
        crate::terrain_assets::find_derived_root(Some(root)).unwrap_or_else(|| root.to_owned());
    let mut error = "No platforms.1ka or platform inventory in world.1ka".to_owned();
    for path in [
        derived.join(platforms::FILE_NAME),
        derived.join(tile_archive::FILE_NAME),
    ] {
        if !path.is_file() {
            continue;
        }
        match tile_archive::Reader::open(&path).and_then(|r| platforms::load(&r)) {
            Ok(inventory) => {
                let date = inventory.manifest["retrieved_utc"]
                    .as_str()
                    .unwrap_or("unknown date")
                    .split('T')
                    .next()
                    .unwrap_or_default();
                let status = format!(
                    "{} installations · downloaded {date}",
                    inventory.platforms.len()
                );
                eprintln!("[1kEE] Offshore platforms: {status}");
                return Snapshot {
                    inventory: Some(inventory),
                    status,
                };
            }
            Err(e) => error = format!("{}: {e}", path.display()),
        }
    }
    Snapshot {
        inventory: None,
        status: error,
    }
}

fn snapshot(root: &Path, ctx: &egui::Context) -> Option<(u64, Arc<Snapshot>)> {
    let Ok(mut state) = source().try_lock() else {
        ctx.request_repaint();
        return None;
    };
    if state.root.as_deref() != Some(root) {
        state.root = Some(root.to_owned());
        state.pending = None;
        state.snapshot = None;
        state.revision += 1;
    }
    if let Some(receiver) = &state.pending {
        match receiver.try_recv() {
            Ok(data) => {
                state.snapshot = Some(Arc::new(data));
                state.pending = None;
                state.revision += 1;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                state.snapshot = Some(Arc::new(Snapshot {
                    inventory: None,
                    status: "Platform reader stopped; use Reload to retry".into(),
                }));
                state.pending = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }
    if state.snapshot.is_none() && state.pending.is_none() {
        let (tx, rx) = mpsc::channel();
        state.pending = Some(rx);
        let root = root.to_owned();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(load(&root));
            ctx.request_repaint();
        });
    }
    state.snapshot.clone().map(|s| (state.revision, s))
}

#[derive(PartialEq)]
struct MeshKey {
    revision: u64,
    filter: Filter,
    projection: Vec<u32>,
    color: Color32,
    clip: Rect,
}
struct Prepared {
    key: MeshKey,
    mesh: Arc<egui::Mesh>,
    hits: Vec<(Pos2, usize)>,
}
thread_local! { static MESHES: RefCell<[Option<Prepared>; 2]> = const { RefCell::new([None, None]) }; }

fn prepare(data: &Inventory, key: MeshKey, project: impl Fn(GeoPoint) -> Option<Pos2>) -> Prepared {
    let mut mesh = egui::Mesh::default();
    let mut hits = Vec::new();
    for (index, p) in data.platforms.iter().enumerate() {
        if !key.filter.accepts(p) {
            continue;
        }
        let Some(pos) = project(GeoPoint {
            lat: p.lat,
            lon: p.lon,
        })
        .filter(|p| p.x.is_finite() && p.y.is_finite() && key.clip.expand(4.0).contains(*p)) else {
            continue;
        };
        let color = if p.historical || p.planned || p.support {
            key.color.gamma_multiply(0.5)
        } else {
            key.color
        };
        let base = mesh.vertices.len() as u32;
        for offset in [
            egui::vec2(0.0, -3.5),
            egui::vec2(3.5, 0.0),
            egui::vec2(0.0, 3.5),
            egui::vec2(-3.5, 0.0),
        ] {
            mesh.colored_vertex(pos + offset, color);
        }
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
        hits.push((pos, index));
    }
    Prepared {
        key,
        mesh: Arc::new(mesh),
        hits,
    }
}

pub(super) fn draw(
    painter: &egui::Painter,
    root: &Path,
    local: bool,
    projection: Vec<u32>,
    filter: Filter,
    project: impl Fn(GeoPoint) -> Option<Pos2>,
) {
    let Some((revision, snapshot)) = snapshot(root, painter.ctx()) else {
        painter.text(
            painter.clip_rect().left_bottom() + egui::vec2(12.0, -46.0),
            egui::Align2::LEFT_BOTTOM,
            "Loading offshore platforms…",
            egui::FontId::proportional(11.0),
            crate::theme::text_muted(),
        );
        return;
    };
    let Some(inventory) = &snapshot.inventory else {
        return;
    };
    let key = MeshKey {
        revision,
        filter,
        projection,
        color: crate::theme::pipeline_oil_color(),
        clip: painter.clip_rect(),
    };
    let pointer = super::infrastructure_hover::pointer();
    MESHES.with(|cache| {
        let mut cache = cache.borrow_mut();
        let slot = &mut cache[usize::from(local)];
        if slot.as_ref().is_none_or(|p| p.key != key) {
            *slot = Some(prepare(inventory, key, project));
        }
        let prepared = slot.as_ref().unwrap();
        painter.add(egui::Shape::Mesh(prepared.mesh.clone()));
        if let Some(pointer) = pointer {
            for &(pos, index) in &prepared.hits {
                let distance = pointer.distance(pos);
                if distance <= super::infrastructure_hover::RADIUS {
                    super::infrastructure_hover::platform(&inventory.platforms[index], distance);
                }
            }
        }
    });
}

pub(crate) fn controls(ui: &mut egui::Ui, filter: &mut Filter) {
    ui.horizontal(|ui| {
        ui.checkbox(&mut filter.bsee, "BSEE · Gulf");
        ui.checkbox(&mut filter.emodnet, "EMODnet · Europe");
    });
    ui.checkbox(&mut filter.historical, "Include removed / inactive");
    ui.checkbox(&mut filter.planned, "Include planned / construction");
    ui.checkbox(&mut filter.support, "Include subsea / buoys / terminals");
    if let Ok(state) = source().try_lock()
        && let Some(snapshot) = &state.snapshot
    {
        ui.small(&snapshot.status);
    }
    ui.small("Static locations. No recorded removal does not confirm operation. Hover a diamond for details.");
    if ui.small_button("Reload platform archive").clicked()
        && let Ok(mut state) = source().try_lock()
    {
        state.root = None;
        state.snapshot = None;
        state.pending = None;
        state.revision += 1;
    }
    ui.horizontal_wrapped(|ui| {
        ui.hyperlink_to("BSEE", "https://www.data.bsee.gov/Main/Mapping.aspx");
        ui.hyperlink_to("EMODnet / Cogea · CC BY 4.0", "https://emodnet.ec.europa.eu/geonetwork/srv/eng/catalog.search#/metadata/ddbe3597-4e3f-4e74-8d31-947c4efef2e9");
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires installed platform archive"]
    fn real_platform_mesh_has_no_count_budget() {
        let root = std::env::var_os("ONEKEE_PLATFORM_ARCHIVE").expect("archive path");
        let inventory =
            platforms::load(&tile_archive::Reader::open(Path::new(&root)).unwrap()).unwrap();
        let filter = Filter::default();
        let expected = inventory
            .platforms
            .iter()
            .filter(|p| filter.accepts(p))
            .count();
        let key = MeshKey {
            revision: 1,
            filter,
            projection: vec![],
            color: Color32::WHITE,
            clip: Rect::from_min_max(egui::pos2(-200.0, -100.0), egui::pos2(200.0, 100.0)),
        };
        let started = std::time::Instant::now();
        let mesh = prepare(&inventory, key, |p| Some(egui::pos2(p.lon, p.lat)));
        assert_eq!(mesh.hits.len(), expected);
        assert_eq!(mesh.mesh.vertices.len(), expected * 4);
        eprintln!(
            "Prepared {expected} platform diamonds in {:?}",
            started.elapsed()
        );
    }
}
