//! Browse the local survey index and render a selected bare-earth window.
use crate::model::{ActiveBody, AppModel, GeoJsonFeature, GeoJsonGeometry, GeoJsonLayer, GeoPoint};
use crate::panels::world_map::satellite_layer::SatelliteLayer;
use crate::{satellite_image, settings_store};
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
    has_raster_collection: bool,
}

enum Completed {
    Refresh(String),
    Terrain(String, satellite_image::SatelliteImage),
}

#[derive(Default)]
pub(crate) struct Browser {
    path: PathBuf,
    last_check: Option<Instant>,
    work: Option<mpsc::Receiver<Result<Completed, String>>>,
    query: String,
    near: bool,
    offset: usize,
    more: bool,
    results: Vec<Survey>,
    selected: Option<Survey>,
    pub(crate) footprint: Option<GeoJsonLayer>,
    status: String,
}

fn db_path(model: &AppModel) -> PathBuf {
    settings_store::configured_data_root()
        .unwrap_or_else(|| {
            model
                .selected_root
                .as_deref()
                .unwrap_or(Path::new("."))
                .join("Data")
        })
        .join("Lidar/catalog.sqlite")
}

fn connection(path: &Path) -> Result<Connection, String> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| e.to_string())
}

fn search(browser: &mut Browser, center: GeoPoint) -> Result<(), String> {
    let db = connection(&browser.path)?;
    let mut sql = String::from(
        "SELECT id,name,product,date,page,asset,stac IS NOT NULL FROM surveys
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
                    has_raster_collection: row.get(6)?,
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
    Ok(())
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
    // A measured boundary point also works for disjoint flight strips.
    let focus = match geometry {
        GeoJsonGeometry::Point(point) => Some(*point),
        GeoJsonGeometry::Polygon(rings) => rings.first().and_then(|ring| ring.first()).copied(),
        GeoJsonGeometry::MultiPolygon(parts) => parts
            .first()
            .and_then(|rings| rings.first())
            .and_then(|ring| ring.first())
            .copied(),
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
    job: impl FnOnce(PathBuf) -> Result<Completed, String> + Send + 'static,
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
    start(browser, |db| {
        run(python(&db).arg("refresh")).map(Completed::Refresh)
    });
}

fn center(model: &AppModel) -> GeoPoint {
    if model.globe_view.local_mode {
        model.globe_view.local_center
    } else {
        model.globe_view.globe_center_latlon()
    }
}

pub(crate) fn tick(ctx: &egui::Context, model: &mut AppModel) {
    let Some(rx) = model.lidar.work.as_ref() else {
        return;
    };
    let result = match rx.try_recv() {
        Ok(r) => r,
        Err(mpsc::TryRecvError::Empty) => return,
        Err(mpsc::TryRecvError::Disconnected) => Err("Lidar worker stopped".into()),
    };
    model.lidar.work = None;
    if db_path(model) != model.lidar.path {
        return;
    }
    match result {
        Ok(Completed::Terrain(name, image)) => {
            model
                .satellite_layers
                .push(SatelliteLayer::new(ctx, name, image));
            model.lidar.status = "Bare-earth hillshade loaded.".into();
        }
        result => {
            model.lidar.status = match result {
                Ok(Completed::Refresh(msg)) | Err(msg) => msg,
                _ => unreachable!(),
            };
            let focus = center(model);
            if model.lidar.path.is_file()
                && let Err(e) = search(&mut model.lidar, focus)
            {
                model.lidar.status.push_str(&format!("\n{e}"));
            }
        }
    }
}

pub(crate) fn controls(ui: &mut egui::Ui, model: &mut AppModel) {
    if model.active_body != ActiveBody::Earth {
        return;
    }
    ui.collapsing("Lidar surveys", |ui| {
        let path = db_path(model);
        let focus = center(model);
        let b = &mut model.lidar;
        let first = b.path != path;
        if first {
            *b = Browser {
                path,
                ..Default::default()
            };
        }
        let due = b
            .last_check
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(86400));
        let refresh = ui
            .add_enabled(b.work.is_none(), egui::Button::new("Refresh catalogs"))
            .clicked();
        if b.work.is_none() && (refresh || due) {
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
        let mut query = first;
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
                Ok(point) => model.globe_view.focus_on(point),
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
            if let Some(layer) = &mut b.footprint {
                ui.checkbox(&mut layer.visible, "Show survey footprint");
            }
            if s.has_raster_collection
                && ui
                    .add_enabled(
                        b.work.is_none(),
                        egui::Button::new("Stream terrain at map center"),
                    )
                    .on_hover_text("Load 2 km of published bare-earth relief from this survey")
                    .clicked()
            {
                let (id, name) = (s.id.clone(), s.name.clone());
                let max_side = ui.ctx().input(|i| i.max_texture_side);
                // The selected survey may have moved the map this frame.
                let at = if model.globe_view.local_mode {
                    model.globe_view.local_center
                } else {
                    model.globe_view.globe_center_latlon()
                };
                b.status = "Streaming a 2 km terrain window…".into();
                start(b, move |db| {
                    let tmp = tempfile::tempdir().map_err(|e| e.to_string())?;
                    let out = tmp.path().join("relief.tif");
                    run(python(&db)
                        .arg("terrain")
                        .arg("--id")
                        .arg(id)
                        .arg("--lat")
                        .arg(at.lat.to_string())
                        .arg("--lon")
                        .arg(at.lon.to_string())
                        .arg("--output")
                        .arg(&out))?;
                    Ok(Completed::Terrain(
                        format!("{name} hillshade"),
                        satellite_image::read(&out, max_side)?,
                    ))
                });
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
        assert_eq!(select(&mut browser, strip).unwrap().lon, -55.);
        assert!(browser.footprint.unwrap().visible);
    }
}
