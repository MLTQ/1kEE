//! Worldwide lidar layer and optional survey search.
use crate::model::{ActiveBody, AppModel, GeoJsonFeature, GeoJsonGeometry, GeoJsonLayer, GeoPoint};
use crate::{lidar_tiles::Tiles, settings_store};
use rusqlite::{Connection, OpenFlags, params};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    time::{Duration, Instant},
};

const HARVESTER: &str = include_str!("../../../scripts/lidar_catalog.py");
const PAGE_SIZE: usize = 12;

#[derive(Clone)]
struct Survey {
    id: String,
    name: String,
    product: String,
    date: Option<String>,
    page: String,
    asset: Option<String>,
}

#[derive(Default)]
pub(crate) struct Browser {
    pub(crate) enabled: bool,
    pub(crate) tiles: Tiles,
    searched: bool,
    path: PathBuf,
    last_check: Option<Instant>,
    last_generation_check: Option<Instant>,
    work: Option<mpsc::Receiver<Result<String, String>>>,
    query: String,
    near: bool,
    offset: usize,
    more: bool,
    results: Vec<Survey>,
    selected: Option<Survey>,
    picker_center: Option<GeoPoint>,
    picker_job: Option<mpsc::Receiver<Result<Vec<Survey>, String>>>,
    picker_results: Vec<Survey>,
    picker_error: String,
    pub(crate) footprint: Option<GeoJsonLayer>,
    status: String,
}

impl Browser {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            ..Default::default()
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

fn connection(path: &Path) -> Result<Connection, String> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| e.to_string())
}

fn search(browser: &mut Browser, center: GeoPoint) -> Result<(), String> {
    let db = connection(&browser.path)?;
    let mut sql = String::from(
        "SELECT id,name,product,date,page,asset FROM surveys
         WHERE instr(lower(name||' '||product||' '||id||' '||source),lower(?1))>0",
    );
    let half_lat = 50. / 110.;
    let half_lon = (half_lat / f64::from(center.lat).to_radians().cos().abs().max(0.001)).min(180.);
    if browser.near {
        sql.push_str(
            " AND north >= ?2 AND south <= ?3 AND
            ((east >= ?4 AND west <= ?5) OR (east >= ?4+360 AND west <= ?5+360)
              OR (east >= ?4-360 AND west <= ?5-360))",
        );
    }
    sql.push_str(" ORDER BY name,id LIMIT ?6 OFFSET ?7");
    let mut stmt = db.prepare(&sql).map_err(|e| e.to_string())?;
    let lon = f64::from(center.lon);
    let lat = f64::from(center.lat);
    let rows = stmt
        .query_map(
            params![
                browser.query.trim(),
                lat - half_lat,
                lat + half_lat,
                lon - half_lon,
                lon + half_lon,
                PAGE_SIZE + 1,
                browser.offset
            ],
            |row| {
                Ok(Survey {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    product: row.get(2)?,
                    date: row.get(3)?,
                    page: row.get(4)?,
                    asset: row.get(5)?,
                })
            },
        )
        .map_err(|e| e.to_string())?;
    let mut results = rows
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    browser.more = results.len() > PAGE_SIZE;
    results.truncate(PAGE_SIZE);
    browser.results = results;
    browser.searched = true;
    Ok(())
}

fn interior(rings: &[Vec<GeoPoint>]) -> Option<GeoPoint> {
    let outer = rings.first()?;
    let first = *outer.first()?;
    let (south, north) = outer.iter().fold((first.lat, first.lat), |(s, n), p| {
        (s.min(p.lat), n.max(p.lat))
    });
    let lat = (south + north) * 0.5;
    let unwrap = |lon: f32| first.lon + (lon - first.lon + 180.).rem_euclid(360.) - 180.;
    let mut crossings = Vec::new();
    for ring in rings {
        for edge in ring.windows(2) {
            let (a, b) = (edge[0], edge[1]);
            if (a.lat > lat) != (b.lat > lat) {
                crossings.push(
                    unwrap(a.lon)
                        + (lat - a.lat) / (b.lat - a.lat) * (unwrap(b.lon) - unwrap(a.lon)),
                );
            }
        }
    }
    crossings.sort_by(f32::total_cmp);
    crossings
        .chunks_exact(2)
        .max_by(|a, b| (a[1] - a[0]).total_cmp(&(b[1] - b[0])))
        .map(|pair| GeoPoint {
            lat,
            lon: ((pair[0] + pair[1]) * 0.5 + 180.).rem_euclid(360.) - 180.,
        })
        .or(Some(first))
}

fn select(browser: &mut Browser, survey: Survey) -> Result<GeoPoint, String> {
    let db = connection(&browser.path)?;
    let geometry: String = db
        .query_row(
            "SELECT geometry FROM surveys WHERE id=?",
            [&survey.id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    let mut layer = GeoJsonLayer::parse(survey.name.clone(), &geometry)?;
    let geometry = &layer
        .features
        .first()
        .ok_or("Missing survey footprint")?
        .geometry;
    let focus = match geometry {
        GeoJsonGeometry::Point(point) => Some(*point),
        GeoJsonGeometry::Polygon(rings) => interior(rings),
        GeoJsonGeometry::MultiPolygon(parts) => parts
            .iter()
            .max_by_key(|rings| rings.first().map_or(0, Vec::len))
            .and_then(|rings| interior(rings)),
        _ => None,
    }
    .ok_or("Missing survey location")?;
    layer.color = [57, 219, 202, 255];
    layer.features.push(GeoJsonFeature {
        geometry: GeoJsonGeometry::Point(focus),
        label: Some(survey.name.clone()),
        color: None,
    });
    browser.footprint = Some(layer);
    browser.selected = Some(survey);
    Ok(focus)
}

fn start(
    browser: &mut Browser,
    job: impl FnOnce(PathBuf) -> Result<String, String> + Send + 'static,
) {
    let db = browser.path.clone();
    let (tx, rx) = mpsc::channel();
    match std::thread::Builder::new()
        .name("lidar-surveys".into())
        .spawn(move || {
            let _ = tx.send(job(db));
            crate::app::request_repaint();
        }) {
        Ok(_) => browser.work = Some(rx),
        Err(e) => browser.status = e.to_string(),
    }
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

fn check(browser: &mut Browser) {
    browser.last_check = Some(Instant::now());
    browser.status = "Checking survey catalogs…".into();
    start(browser, |db| run(python(&db).arg("refresh")));
}

fn center(model: &AppModel) -> GeoPoint {
    if model.globe_view.local_mode {
        model.globe_view.local_center
    } else {
        model.globe_view.globe_center_latlon()
    }
}

fn covers(rings: &[Vec<GeoPoint>], point: GeoPoint) -> bool {
    let Some(first) = rings.first().and_then(|r| r.first()) else {
        return false;
    };
    let unwrap = |lon: f32| first.lon + (lon - first.lon + 180.).rem_euclid(360.) - 180.;
    let x = unwrap(point.lon);
    let mut inside = false;
    for edge in rings.iter().flat_map(|ring| ring.windows(2)) {
        let (a, b) = (edge[0], edge[1]);
        if (a.lat > point.lat) != (b.lat > point.lat)
            && x < unwrap(a.lon)
                + (point.lat - a.lat) / (b.lat - a.lat) * (unwrap(b.lon) - unwrap(a.lon))
        {
            inside = !inside;
        }
    }
    inside
}

fn overlapping(path: &Path, at: GeoPoint) -> Result<Vec<Survey>, String> {
    let db = connection(path)?;
    let bulk = connection(&path.with_file_name("bulk.sqlite")).ok();
    let mut q = db
        .prepare(
            "SELECT id,name,product,date,page,asset,geometry FROM surveys
        WHERE west<=?1 AND east>=?1 AND south<=?2 AND north>=?2
        AND (id LIKE 'NOAAID:%' OR id LIKE 'USGS_3DEP_ID:%' OR stac IS NOT NULL
             OR source IN ('linz-coastal','eba') OR id LIKE 'INFOMAR:%:SR_URL'
             OR (source='swisssurface3d' AND asset LIKE '%.copc.laz') OR product='Lidar raster')
             ORDER BY date DESC,name,id",
        )
        .map_err(|e| e.to_string())?;
    let mut rows = q
        .query(params![at.lon, at.lat])
        .map_err(|e| e.to_string())?;
    let mut surveys = Vec::new();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let geometry: String = row.get(6).map_err(|e| e.to_string())?;
        let shape = GeoJsonLayer::parse(String::new(), &geometry)?;
        let hit = shape.features.iter().any(|f| match &f.geometry {
            GeoJsonGeometry::Polygon(rings) => covers(rings, at),
            GeoJsonGeometry::MultiPolygon(parts) => parts.iter().any(|rings| covers(rings, at)),
            GeoJsonGeometry::Point(point) => *point == at,
            _ => false,
        });
        if hit {
            let read = |i| row.get::<_, String>(i).map_err(|e| e.to_string());
            let mut survey = Survey {
                id: read(0)?,
                name: read(1)?,
                product: read(2)?,
                date: row.get(3).map_err(|e| e.to_string())?,
                page: read(4)?,
                asset: row.get(5).map_err(|e| e.to_string())?,
            };
            if survey.id.starts_with("EBA:")
                && !bulk.as_ref().is_some_and(|db| {
                    db.query_row(
                        "SELECT 1 FROM transects WHERE survey=? LIMIT 1",
                        [&survey.id],
                        |_| Ok(()),
                    )
                    .is_ok()
                })
            {
                continue;
            }
            if survey.id.starts_with("LINZ:")
                && let Some(slash) = survey.id.rfind('/')
            {
                survey.id.truncate(slash + 1);
                if let Some((collection, _)) = survey.name.split_once(" · ") {
                    survey.name = collection.to_owned();
                }
            }
            if !surveys.iter().any(|s: &Survey| s.id == survey.id) {
                surveys.push(survey);
            }
        }
    }
    Ok(surveys)
}

pub(crate) fn picker(ui: &mut egui::Ui, model: &mut AppModel) {
    if !model.lidar.enabled || model.active_body != ActiveBody::Earth {
        return;
    }
    let at = center(model);
    let b = &mut model.lidar;
    ui.horizontal(|ui| {
        ui.menu_button("Surveys at map center ▾", |ui| {
            if b.picker_job.is_none() && b.picker_center != Some(at) {
                let path = b.path.clone();
                let (tx, rx) = mpsc::channel();
                let ctx = ui.ctx().clone();
                std::thread::spawn(move || {
                    let _ = tx.send(overlapping(&path, at));
                    ctx.request_repaint();
                });
                b.picker_center = Some(at);
                b.picker_job = Some(rx);
                b.picker_results.clear();
                b.picker_error.clear();
            }
            if let Some(rx) = &b.picker_job
                && let Ok(found) = rx.try_recv()
            {
                match found {
                    Ok(rows) => b.picker_results = rows,
                    Err(e) => b.picker_error = e,
                }
                b.picker_job = None;
            }
            if ui
                .selectable_label(b.selected.is_none(), "Automatic")
                .clicked()
            {
                b.selected = None;
                b.footprint = None;
                ui.close_menu();
            }
            egui::ScrollArea::vertical()
                .max_height(280.)
                .show(ui, |ui| {
                    for s in &b.picker_results {
                        let date = s
                            .date
                            .as_deref()
                            .unwrap_or("")
                            .split('T')
                            .next()
                            .unwrap_or("");
                        if ui
                            .selectable_label(
                                b.selected.as_ref().is_some_and(|v| v.id == s.id),
                                format!("{date}  {}", s.name),
                            )
                            .clicked()
                        {
                            b.selected = Some(s.clone());
                            b.footprint = None;
                            ui.close_menu();
                        }
                    }
                    if b.picker_job.is_some() {
                        ui.small("Finding surveys…");
                    } else if !b.picker_error.is_empty() {
                        ui.small(&b.picker_error);
                    } else if b.picker_results.is_empty() {
                        ui.small("No surveys cover the map center");
                    }
                });
        });
        if let Some(s) = &b.selected {
            ui.add(egui::Label::new(&s.name).truncate())
                .on_hover_text(&s.name);
        }
    });
}

pub(crate) fn tick(ctx: &egui::Context, model: &mut AppModel) {
    if !model.lidar.enabled || model.active_body != ActiveBody::Earth {
        model.lidar.tiles.stop();
        if model.lidar.work.is_none() {
            return;
        }
    }
    let focus = center(model);
    let earth = model.active_body == ActiveBody::Earth;
    let b = &mut model.lidar;
    if let Some(rx) = &b.work {
        let result = match rx.try_recv() {
            Ok(r) => Some(r),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err("Lidar catalog worker stopped".into()))
            }
        };
        if let Some(result) = result {
            b.work = None;
            b.picker_center = None;
            b.status = result.unwrap_or_else(|error| error);
            b.tiles.catalog_changed(&b.path);
            if b.searched
                && let Err(e) = search(b, focus)
            {
                b.status.push_str(&format!("\n{e}"));
            }
        }
    }
    if b.enabled && earth {
        if b.work.is_none()
            && b.last_check
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(86400))
        {
            check(b);
        }
        if b.path.is_file() {
            if b.last_generation_check
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(10))
            {
                b.tiles.catalog_changed(&b.path);
                b.last_generation_check = Some(Instant::now());
            }
            let preferred = b.selected.as_ref().map_or("", |survey| survey.id.as_str());
            b.tiles.tick(ctx, &b.path, preferred);
        }
    }
}

pub(crate) fn toggle(ui: &mut egui::Ui, model: &mut AppModel) {
    if model.active_body != ActiveBody::Earth {
        return;
    }
    ui.checkbox(&mut model.lidar.enabled, "Lidar")
        .on_hover_text(
            "Zoom into teal survey paths to load and save lidar. Gray paths mark archive coverage.",
        );
    if model.lidar.enabled {
        if !model.lidar.tiles.status.is_empty() {
            ui.small(&model.lidar.tiles.status)
                .on_hover_text(&model.lidar.tiles.sources);
        } else if !model.lidar.status.is_empty() {
            ui.small(&model.lidar.status);
        }
    }
}

pub(crate) fn controls(ui: &mut egui::Ui, model: &mut AppModel) {
    if model.active_body != ActiveBody::Earth {
        return;
    }
    ui.collapsing("Lidar surveys", |ui| {
        let focus = center(model);
        let b = &mut model.lidar;
        if ui
            .add_enabled(b.work.is_none(), egui::Button::new("Refresh catalogs"))
            .clicked()
        {
            check(b);
        }
        ui.small(&b.status);
        let edit = ui
            .text_edit_singleline(&mut b.query)
            .on_hover_text("Survey, file, or provider name");
        let submitted = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let changed = ui
            .checkbox(&mut b.near, "100 km box at map center")
            .changed();
        let mut query = !b.searched;
        if ui.small_button("Search surveys").clicked() || changed || submitted {
            b.offset = 0;
            query = true;
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(b.offset > 0, egui::Button::new("Previous"))
                .clicked()
            {
                b.offset = b.offset.saturating_sub(PAGE_SIZE);
                query = true;
            }
            if ui.add_enabled(b.more, egui::Button::new("Next")).clicked() {
                b.offset += PAGE_SIZE;
                query = true;
            }
        });
        if query
            && b.path.is_file()
            && let Err(e) = search(b, focus)
        {
            b.status = e;
        }
        if b.results.is_empty() && b.work.is_none() {
            ui.small("No matching surveys");
        }
        let mut chosen = None;
        for row in &b.results {
            if ui
                .selectable_label(
                    b.selected.as_ref().is_some_and(|s| s.id == row.id),
                    &row.name,
                )
                .clicked()
            {
                chosen = Some(row.clone());
            }
        }
        if let Some(row) = chosen {
            match select(b, row) {
                Ok(point) => {
                    b.enabled = true;
                    model.globe_view.stop_motion();
                    model.globe_view.focus_on(point);
                    model.globe_view.local_mode = true;
                    model.globe_view.local_zoom = model.globe_view.local_zoom.max(20.);
                }
                Err(e) => b.status = e,
            }
        }
        if let Some(s) = &b.selected {
            ui.separator();
            ui.strong(&s.name);
            ui.small(&s.product);
            if let Some(date) = &s.date {
                ui.small(date);
            }
            ui.hyperlink_to("Survey record", &s.page);
            if let Some(asset) = &s.asset {
                ui.hyperlink_to(
                    if s.id.starts_with("EBA:") {
                        "Regional scan archives"
                    } else {
                        "Download scan file"
                    },
                    asset,
                );
            }
            if ui.small_button("Clear selection").clicked() {
                b.selected = None;
                b.footprint = None;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_and_select_polygon_or_point_without_network() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.sqlite");
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE surveys(id,source,name,product,date,page,asset,stac,geometry,west,south,east,north);
            INSERT INTO surveys VALUES('a','eba','Amazon strip','EBA',NULL,'https://example.org',NULL,NULL,
                '{\"type\":\"Polygon\",\"coordinates\":[[[-55,-4],[-54,-4],[-54,-3],[-55,-4]]]}',-55,-4,-54,-3);
            INSERT INTO surveys VALUES('b','nasa-brazil','Amazon point','NASA',NULL,'https://example.org','https://example.org/f.laz',NULL,
                '{\"type\":\"Point\",\"coordinates\":[-60,-6]}',-60,-6,-60,-6);").unwrap();
        let mut browser = Browser {
            path,
            query: "Amazon".into(),
            ..Default::default()
        };
        let at = GeoPoint {
            lat: -3.5,
            lon: -54.5,
        };
        search(&mut browser, at).unwrap();
        assert_eq!(browser.results.len(), 2);
        let point = browser.results[0].clone();
        assert_eq!(select(&mut browser, point).unwrap().lon, -60.);
        browser.near = true;
        search(&mut browser, at).unwrap();
        assert_eq!(browser.results.len(), 1);
        let strip = browser.results[0].clone();
        assert_eq!(select(&mut browser, strip).unwrap().lon, -54.25);
        assert!(browser.footprint.unwrap().visible);
        db.execute(
            "UPDATE surveys SET id='NOAAID:1',date='1998' WHERE id='a'",
            [],
        )
        .unwrap();
        db.execute("INSERT INTO surveys SELECT 'NOAAID:2',source,name,product,'2018',page,asset,stac,geometry,west,south,east,north FROM surveys WHERE id='NOAAID:1'", []).unwrap();
        let choices = overlapping(
            &browser.path,
            GeoPoint {
                lat: -3.6,
                lon: -54.2,
            },
        )
        .unwrap();
        assert_eq!(
            choices.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["NOAAID:2", "NOAAID:1"]
        );
        assert!(
            overlapping(
                &browser.path,
                GeoPoint {
                    lat: -3.1,
                    lon: -54.9
                }
            )
            .unwrap()
            .is_empty()
        );
    }
}
