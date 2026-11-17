//! Local catalog footprints, independent of prepared scans and raster workers.
use super::local_terrain_scene::projection::LocalProjectionParams;
use crate::{
    lidar_catalog::{ReadJob, completed, read_in_background},
    model::GeoPoint,
};
use rusqlite::{Connection, OpenFlags, params};
use std::{
    cell::Cell,
    path::Path,
    time::{Duration, Instant, SystemTime},
};

const COLOR: egui::Color32 = egui::Color32::from_rgb(41, 210, 214);
const POINT_LIMIT: usize = 250_000;
#[derive(Default)]
pub(crate) struct Coverage {
    requested: Cell<Option<[f32; 4]>>,
    snapshot: Option<Snapshot>,
    job: Option<ReadJob<Snapshot>>,
    checked: Option<Instant>,
    failed: bool,
    pub status: String,
}
struct Snapshot {
    bounds: [f32; 4],
    modified: Option<SystemTime>,
    paths: Vec<Vec<GeoPoint>>,
    surveys: usize,
    limited: bool,
}
impl Coverage {
    pub fn suspend(&self) {
        self.requested.set(None);
    }
    pub fn tick(&mut self, ctx: &egui::Context, path: &Path, local: bool) {
        if let Some(result) = completed(&mut self.job) {
            match result {
                Ok(snapshot) => {
                    self.failed = false;
                    self.status = format!(
                        "{} catalog survey footprints{}",
                        snapshot.surveys,
                        if snapshot.limited {
                            " · outline display limit reached"
                        } else {
                            ""
                        }
                    );
                    self.snapshot = Some(snapshot);
                }
                Err(e) => {
                    self.status = e;
                    self.failed = true;
                }
            }
            self.checked = Some(Instant::now());
        }
        if !local {
            self.suspend();
            return;
        }
        let Some(bounds) = self.requested.get() else {
            return;
        };
        if self.job.is_some() {
            return;
        }
        if self.failed
            && self
                .checked
                .is_some_and(|t| t.elapsed() < Duration::from_secs(5))
        {
            return;
        }
        let modified = path.metadata().and_then(|m| m.modified()).ok();
        let fresh = self
            .snapshot
            .as_ref()
            .is_some_and(|s| contains(s.bounds, bounds) && s.modified == modified);
        if fresh
            && self
                .checked
                .is_some_and(|t| t.elapsed() < Duration::from_secs(30))
        {
            return;
        }
        if !fresh
            && self.snapshot.is_none()
            && self
                .checked
                .is_some_and(|t| t.elapsed() < Duration::from_secs(5))
        {
            return;
        }
        let bounds = expanded(bounds);
        let path = path.to_owned();
        if self.snapshot.is_none() {
            self.status = "Loading survey outlines…".into();
        }
        self.job = Some(read_in_background(ctx, move || read(&path, bounds)));
    }
    pub fn draw(
        &self,
        painter: &egui::Painter,
        projection: &LocalProjectionParams,
        bounds: Option<[f32; 4]>,
        root: Option<&Path>,
        fine: bool,
    ) {
        if self.requested.replace(bounds) != bounds {
            painter
                .ctx()
                .request_repaint_after(Duration::from_millis(50));
        }
        let (Some(bounds), Some(snapshot)) = (bounds, &self.snapshot) else {
            return;
        };
        let srtm = crate::terrain_assets::find_srtm_root(root);
        let project = |point: GeoPoint| {
            let height = if fine {
                crate::threedep::cached_elevation_m(point)
            } else {
                None
            }
            .or_else(|| super::srtm_stream::cached_tile_elevation_m(srtm.as_deref(), point))
            .unwrap_or(0.);
            let (x, y, _) = projection.project_logical(point, height);
            egui::pos2(x, y)
        };
        for path in &snapshot.paths {
            if path.len() == 1
                && let Some(p) = path.first()
            {
                let p = GeoPoint {
                    lon: unwrap(p.lon, (bounds[0] + bounds[2]) * 0.5),
                    ..*p
                };
                if p.lon >= bounds[0]
                    && p.lon <= bounds[2]
                    && p.lat >= bounds[1]
                    && p.lat <= bounds[3]
                {
                    painter.circle_stroke(project(p), 3., egui::Stroke::new(1.3_f32, COLOR));
                }
            }
            for pair in path.windows(2) {
                if let Some([a, b]) = clip(pair[0], pair[1], bounds) {
                    let [a, b] = [project(a), project(b)];
                    if a.is_finite() && b.is_finite() {
                        painter.line_segment([a, b], egui::Stroke::new(1.3_f32, COLOR));
                    }
                }
            }
        }
    }
}
fn contains(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] <= b[0] && a[1] <= b[1] && a[2] >= b[2] && a[3] >= b[3]
}
fn expanded([w, s, e, n]: [f32; 4]) -> [f32; 4] {
    let dx = (e - w) * 0.25;
    let dy = (n - s) * 0.25;
    [w - dx, (s - dy).max(-90.), e + dx, (n + dy).min(90.)]
}
fn unwrap(lon: f32, center: f32) -> f32 {
    center + (lon - center + 180.).rem_euclid(360.) - 180.
}
fn clip(a: GeoPoint, b: GeoPoint, bounds: [f32; 4]) -> Option<[GeoPoint; 2]> {
    let [w, s, e, n] = bounds;
    let center = (w + e) * 0.5;
    let a = GeoPoint {
        lon: unwrap(a.lon, center),
        ..a
    };
    let b = GeoPoint {
        lon: unwrap(b.lon, center),
        ..b
    };
    let dx = b.lon - a.lon;
    let dy = b.lat - a.lat;
    let mut lo = 0f32;
    let mut hi = 1f32;
    for (p, q) in [
        (-dx, a.lon - w),
        (dx, e - a.lon),
        (-dy, a.lat - s),
        (dy, n - a.lat),
    ] {
        if p == 0. {
            if q < 0. {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0. {
                lo = lo.max(t);
            } else {
                hi = hi.min(t);
            }
        }
    }
    (lo <= hi).then_some([
        GeoPoint {
            lon: a.lon + lo * dx,
            lat: a.lat + lo * dy,
        },
        GeoPoint {
            lon: a.lon + hi * dx,
            lat: a.lat + hi * dy,
        },
    ])
}
fn read(path: &Path, bounds: [f32; 4]) -> Result<Snapshot, String> {
    let [w, s, e, n] = bounds;
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    let mut query=db.prepare("SELECT geometry,length(geometry) FROM surveys WHERE rowid IN
        (SELECT rowid FROM surveys WHERE south<?3 AND north>?4
        AND ((west<?1 AND east>?2) OR (west<?1-360 AND east>?2-360) OR (west<?1+360 AND east>?2+360)) LIMIT 2049)").map_err(|e|e.to_string())?;
    let mut rows = query
        .query(params![e, w, n, s])
        .map_err(|e| e.to_string())?;
    let mut snapshot = Snapshot {
        bounds,
        modified: path.metadata().and_then(|m| m.modified()).ok(),
        paths: Vec::new(),
        surveys: 0,
        limited: false,
    };
    let mut bytes = 0;
    let mut points = 0;
    let mut seen = 0;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        seen += 1;
        if seen > 2048 || points >= POINT_LIMIT {
            snapshot.limited = true;
            break;
        }
        let size = row.get::<_, usize>(1).map_err(|e| e.to_string())?;
        if size > 8 * 1024 * 1024 || bytes + size > 32 * 1024 * 1024 {
            snapshot.limited = true;
            continue;
        }
        bytes += size;
        let geometry = row.get::<_, String>(0).map_err(|e| e.to_string())?;
        if let Ok(value) = serde_json::from_str(&geometry) {
            append(
                &value,
                &mut snapshot.paths,
                &mut points,
                &mut snapshot.limited,
            );
            snapshot.surveys += 1;
        }
    }
    Ok(snapshot)
}
fn append(
    value: &serde_json::Value,
    paths: &mut Vec<Vec<GeoPoint>>,
    points: &mut usize,
    limited: &mut bool,
) {
    let Some(kind) = value["type"].as_str() else {
        return;
    };
    let line = |v: &serde_json::Value,
                paths: &mut Vec<Vec<GeoPoint>>,
                points: &mut usize,
                limited: &mut bool| {
        let Some(values) = v.as_array() else {
            return;
        };
        let mut line = Vec::new();
        for value in values {
            if *points >= POINT_LIMIT {
                *limited = true;
                break;
            }
            if let (Some(lon), Some(lat)) = (value[0].as_f64(), value[1].as_f64())
                && lon.is_finite()
                && lat.is_finite()
                && (-90. ..=90.).contains(&lat)
            {
                line.push(GeoPoint {
                    lon: lon as f32,
                    lat: lat as f32,
                });
                *points += 1;
            }
        }
        if !line.is_empty() {
            paths.push(line);
        }
    };
    match kind {
        "Point" => line(
            &serde_json::json!([value["coordinates"]]),
            paths,
            points,
            limited,
        ),
        "LineString" => line(&value["coordinates"], paths, points, limited),
        "Polygon" | "MultiLineString" => {
            if let Some(rings) = value["coordinates"].as_array() {
                for ring in rings {
                    line(ring, paths, points, limited);
                }
            }
        }
        "MultiPolygon" => {
            if let Some(polygons) = value["coordinates"].as_array() {
                for rings in polygons {
                    if let Some(rings) = rings.as_array() {
                        for ring in rings {
                            line(ring, paths, points, limited);
                        }
                    }
                }
            }
        }
        "GeometryCollection" => {
            if let Some(values) = value["geometries"].as_array() {
                for value in values {
                    append(value, paths, points, limited);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "reads the real ONEKEE_TARGET_TEST_CATALOG and projects Local outlines without scan preparation"]
    fn live_local_lidar_coverage() {
        let catalog = std::env::var_os("ONEKEE_TARGET_TEST_CATALOG")
            .map(std::path::PathBuf::from)
            .expect("catalog");
        let region = lidar_pipeline::region::Region {
            lon: -77.58,
            lat: 35.26,
            radius_m: 100_000.,
        };
        let bounds = region.bounds().map(|v| v as f32);
        let start = Instant::now();
        let snapshot = read(&catalog, bounds).unwrap();
        let read_ms = start.elapsed().as_secs_f64() * 1000.;
        assert!(snapshot.surveys > 0 && !snapshot.paths.is_empty());
        let survey_count = snapshot.surveys;
        let vertices = snapshot.paths.iter().map(Vec::len).sum::<usize>();
        let limited = snapshot.limited;
        let coverage = Coverage {
            snapshot: Some(snapshot),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280., 720.));
        let focus = GeoPoint {
            lon: -77.58,
            lat: 35.26,
        };
        let mut view = crate::model::GlobeViewState::from_focus(focus);
        view.local_pitch = 0.;
        let layout = super::super::local_terrain_scene::layout(rect);
        let projection = super::super::local_terrain_scene::projection::local_projection_params(
            &layout, &view, focus, 125., 125.,
        );
        let start = Instant::now();
        let out = ctx.run(
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ctx| {
                coverage.draw(
                    &ctx.layer_painter(egui::LayerId::background()),
                    &projection,
                    Some(bounds),
                    None,
                    false,
                );
            },
        );
        let draw_ms = start.elapsed().as_secs_f64() * 1000.;
        let segments = out
            .shapes
            .iter()
            .filter(
                |s| matches!(&s.shape,egui::Shape::LineSegment {stroke,..} if stroke.color==COLOR),
            )
            .count();
        assert!(segments > 10);
        std::fs::write("/tmp/1kee-local-coverage.json",serde_json::to_vec_pretty(&serde_json::json!({"surveys":survey_count,"vertices":vertices,
            "drawn_segments":segments,"read_ms":read_ms,"projection_and_shape_ms":draw_ms,"limited":limited})).unwrap()).unwrap();
        eprintln!(
            "LOCAL_COVERAGE surveys={survey_count} vertices={vertices} segments={segments} read_ms={read_ms:.2} draw_ms={draw_ms:.2} limited={limited}"
        );
    }
    #[test]
    fn local_coverage_reads_unprepared_catalog_surveys_and_polygon_holes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.sqlite");
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE surveys(geometry TEXT,west REAL,south REAL,east REAL,north REAL);",
        )
        .unwrap();
        let geom = serde_json::json!({"type":"Polygon","coordinates":[[[0,0],[2,0],[2,2],[0,0]],[[0.5,0.5],[1,0.5],[0.5,1],[0.5,0.5]]]});
        db.execute("INSERT INTO surveys VALUES(?1,0,0,2,2)", [geom.to_string()])
            .unwrap();
        let s = read(&path, [0., 0., 1., 1.]).unwrap();
        assert_eq!(s.surveys, 1);
        assert_eq!(s.paths.len(), 2);
        assert!(!s.limited);
        assert!(!dir.path().join("fused").exists());
    }
    #[test]
    fn coverage_segments_clip_to_local_view_and_wrap_the_date_line() {
        let p = |lon, lat| GeoPoint { lon, lat };
        assert_eq!(
            clip(p(-5., 0.), p(5., 0.), [-1., -1., 1., 1.]),
            Some([p(-1., 0.), p(1., 0.)])
        );
        assert!(clip(p(-5., 2.), p(5., 2.), [-1., -1., 1., 1.]).is_none());
        let [a, b] = clip(p(179., 0.), p(-179., 0.), [179.5, -1., 180.5, 1.]).unwrap();
        assert_eq!([a.lon, b.lon], [179.5, 180.5]);
    }
}
