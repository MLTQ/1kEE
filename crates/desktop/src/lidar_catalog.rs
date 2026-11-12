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

impl Survey {
    fn dated_name(&self) -> String {
        let date = self
            .date
            .as_deref()
            .unwrap_or("")
            .split('T')
            .next()
            .unwrap_or("");
        format!("{date}  {}", self.name)
    }
}

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

struct Search {
    query: String,
    near: Option<GeoPoint>,
    offset: usize,
}

#[derive(Default)]
pub(crate) struct Browser {
    pub(crate) enabled: bool,
    pub(crate) tiles: Tiles,
    searched: bool,
    path: PathBuf,
    last_check: Option<Instant>,
    work: Option<ReadJob<String>>,
    search_job: Option<ReadJob<Vec<Survey>>>,
    next_search: Option<Search>,
    search_error: String,
    selection_job: Option<ReadJob<(Survey, GeoJsonLayer, GeoPoint)>>,
    query: String,
    near: bool,
    offset: usize,
    more: bool,
    results: Vec<Survey>,
    selected: Option<Survey>,
    picker_target: Option<GeoPoint>,
    picker_center: Option<GeoPoint>,
    picker_job: Option<ReadJob<Vec<Survey>>>,
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

fn search(path: &Path, request: Search) -> Result<Vec<Survey>, String> {
    let db = connection(path)?;
    let mut sql = String::from("SELECT id,name,product,date,page,asset FROM surveys WHERE 1");
    if !request.query.trim().is_empty() {
        sql.push_str(" AND instr(lower(name||' '||product||' '||id||' '||source),lower(?1))>0");
    }
    let center = request.near.unwrap_or(GeoPoint { lat: 0., lon: 0. });
    let half_lat = 50. / 110.;
    let half_lon = (half_lat / f64::from(center.lat).to_radians().cos().abs().max(0.001)).min(180.);
    if request.near.is_some() {
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
                request.query.trim(),
                lat - half_lat,
                lat + half_lat,
                lon - half_lon,
                lon + half_lon,
                PAGE_SIZE + 1,
                request.offset
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
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())
}

impl Browser {
    fn request_search(&mut self, ctx: &egui::Context, center: GeoPoint) {
        self.searched = true;
        self.next_search = Some(Search {
            query: self.query.clone(),
            near: self.near.then_some(center),
            offset: self.offset,
        });
        self.search_error.clear();
        self.poll_search(ctx);
    }

    fn poll_search(&mut self, ctx: &egui::Context) {
        if let Some(done) = completed(&mut self.search_job)
            && self.next_search.is_none()
        {
            match done {
                Ok(mut rows) => {
                    self.more = rows.len() > PAGE_SIZE;
                    rows.truncate(PAGE_SIZE);
                    self.results = rows;
                }
                Err(error) => {
                    self.search_error = error;
                    self.results.clear();
                    self.more = false;
                }
            }
        }
        if self.search_job.is_none()
            && let Some(request) = self.next_search.take()
        {
            let path = self.path.clone();
            self.search_job = Some(read_in_background(ctx, move || search(&path, request)));
        }
    }
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

fn select(path: &Path, survey: Survey) -> Result<(Survey, GeoJsonLayer, GeoPoint), String> {
    let db = connection(path)?;
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
    Ok((survey, layer, focus))
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

fn check(ctx: &egui::Context, browser: &mut Browser) {
    browser.last_check = Some(Instant::now());
    browser.status = "Checking survey catalogs…".into();
    let db = browser.path.clone();
    browser.work = Some(read_in_background(ctx, move || {
        run(python(&db).arg("refresh"))
    }));
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
        let mut menu_visible = false;
        ui.menu_button("Surveys at map center ▾", |ui| {
            menu_visible = true;
            // Keep the point the user opened the menu on while the globe is moving.
            let target = *b.picker_target.get_or_insert(at);
            if let Some(found) = completed(&mut b.picker_job) {
                match found {
                    Ok(rows) => b.picker_results = rows,
                    Err(e) => b.picker_error = e,
                }
            }
            if b.picker_job.is_none() && b.picker_center != Some(target) {
                let path = b.path.clone();
                b.picker_job = Some(read_in_background(ui.ctx(), move || {
                    overlapping(&path, target)
                }));
                b.picker_center = Some(target);
                b.picker_results.clear();
                b.picker_error.clear();
            }
            if ui
                .selectable_label(b.selected.is_none(), "Automatic")
                .clicked()
            {
                b.selection_job = None;
                b.selected = None;
                b.footprint = None;
                ui.close_menu();
            }
            if b.picker_job.is_some() {
                ui.small("Finding surveys…");
            } else if !b.picker_error.is_empty() {
                ui.small(&b.picker_error);
            } else if b.picker_results.is_empty() {
                ui.small("No surveys cover the map center");
            } else {
                let width = (ui.ctx().screen_rect().width() - 24.).clamp(1., 360.);
                ui.set_width(width);
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                let height = ui.spacing().interact_size.y.max(
                    ui.text_style_height(&egui::TextStyle::Button)
                        + 2. * ui.spacing().button_padding.y,
                );
                egui::ScrollArea::vertical().max_height(280.).show_rows(
                    ui,
                    height,
                    b.picker_results.len(),
                    |ui, visible| {
                        for s in &b.picker_results[visible] {
                            if ui
                                .selectable_label(
                                    b.selected.as_ref().is_some_and(|v| v.id == s.id),
                                    s.dated_name(),
                                )
                                .on_hover_text(&s.name)
                                .clicked()
                            {
                                b.selection_job = None;
                                b.selected = Some(s.clone());
                                b.footprint = None;
                                ui.close_menu();
                            }
                        }
                    },
                );
            }
        });
        if !menu_visible {
            b.picker_target = None;
            if !b.picker_error.is_empty() {
                b.picker_center = None;
            }
        }
        if let Some(s) = &b.selected {
            ui.add(egui::Label::new(&s.name).truncate())
                .on_hover_text(&s.name);
        }
    });
}

pub(crate) fn tick(ctx: &egui::Context, model: &mut AppModel) {
    let earth = model.active_body == ActiveBody::Earth;
    let focus = center(model);
    let b = &mut model.lidar;
    b.poll_search(ctx);
    if let Some(done) = completed(&mut b.selection_job)
        && b.enabled
        && earth
    {
        match done {
            Ok((survey, layer, point)) => {
                b.selected = Some(survey);
                b.footprint = Some(layer);
                model.globe_view.stop_motion();
                model.globe_view.focus_on(point);
                model.globe_view.local_mode = true;
                model.globe_view.local_zoom = model.globe_view.local_zoom.max(20.);
            }
            Err(e) => b.search_error = e,
        }
    }
    let refreshed = completed(&mut b.work);
    let force_check = refreshed.is_some();
    if let Some(result) = refreshed {
        b.picker_center = None;
        b.status = result.unwrap_or_else(|error| error);
        if b.searched {
            b.request_search(ctx, focus);
        }
    }
    if !b.enabled || !earth {
        b.selection_job = None;
        b.picker_target = None;
        b.tiles.stop();
        return;
    }
    if b.work.is_none()
        && b.last_check
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(86400))
    {
        check(ctx, b);
    }
    if b.tiles.poll_catalog(ctx, &b.path, force_check) {
        let preferred = b.selected.as_ref().map_or("", |survey| survey.id.as_str());
        b.tiles.tick(ctx, &b.path, preferred);
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
            check(ui.ctx(), b);
        }
        ui.small(&b.status);
        let edit = ui
            .text_edit_singleline(&mut b.query)
            .on_hover_text("Survey, file, or provider name");
        let submitted = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let changed = ui
            .checkbox(&mut b.near, "100 km box at map center")
            .changed();
        let searching = b.search_job.is_some();
        let mut query = !b.searched;
        if ui.small_button("Search surveys").clicked() || changed || submitted {
            b.offset = 0;
            query = true;
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!searching && b.offset > 0, egui::Button::new("Previous"))
                .clicked()
            {
                b.offset = b.offset.saturating_sub(PAGE_SIZE);
                query = true;
            }
            if ui
                .add_enabled(!searching && b.more, egui::Button::new("Next"))
                .clicked()
            {
                b.offset += PAGE_SIZE;
                query = true;
            }
        });
        if query {
            b.request_search(ui.ctx(), focus);
        }
        if b.search_job.is_some() {
            ui.small("Searching surveys…");
        } else if !b.search_error.is_empty() {
            ui.small(&b.search_error);
        } else if b.results.is_empty() {
            ui.small("No matching surveys");
        } else {
            ui.add_enabled_ui(b.selection_job.is_none(), |ui| {
                for row in &b.results {
                    if ui
                        .selectable_label(
                            b.selected.as_ref().is_some_and(|s| s.id == row.id),
                            &row.name,
                        )
                        .clicked()
                    {
                        let path = b.path.clone();
                        let survey = row.clone();
                        b.enabled = true;
                        b.selection_job =
                            Some(read_in_background(ui.ctx(), move || select(&path, survey)));
                    }
                }
            });
        }
        if b.selection_job.is_some() {
            ui.small("Opening survey…");
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
                b.selection_job = None;
                b.selected = None;
                b.footprint = None;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(
        ctx: &egui::Context,
        model: &mut AppModel,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ctx| {
                tick(ctx, model);
                egui::CentralPanel::default().show(ctx, |ui| {
                    picker(ui, model);
                    controls(ui, model);
                });
            },
        )
    }

    fn label(frame: &egui::FullOutput, text: &str) -> Option<egui::Pos2> {
        frame.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(t) if t.galley.text() == text => Some(t.pos + t.galley.size() / 2.),
            _ => None,
        })
    }

    fn click(ctx: &egui::Context, model: &mut AppModel, text: &str) -> egui::FullOutput {
        let at = label(&frame(ctx, model, vec![]), text).expect(text);
        for pressed in [true, false] {
            frame(
                ctx,
                model,
                vec![
                    egui::Event::PointerMoved(at),
                    egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        frame(ctx, model, vec![])
    }

    fn wait_for(ctx: &egui::Context, model: &mut AppModel, done: impl Fn(&Browser) -> bool) {
        let start = Instant::now();
        while !done(&model.lidar) {
            assert!(
                start.elapsed() < Duration::from_secs(60),
                "catalog read did not complete"
            );
            std::thread::sleep(Duration::from_millis(2));
            frame(ctx, model, vec![]);
        }
    }

    #[test]
    fn survey_menus_remain_interactive_while_catalog_is_locked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.sqlite");
        let db = Connection::open(&path).unwrap();
        db.execute_batch(r#"CREATE TABLE surveys(id,source,name,product,date,page,asset,stac,geometry,west,south,east,north);
            INSERT INTO surveys VALUES('NOAAID:1','noaa','Coastal flight','Lidar','1998','','',NULL,
                '{"type":"Point","coordinates":[-75,38]}',-75,38,-75,38);
            INSERT INTO surveys VALUES('b','demo','Inland flight','Lidar',NULL,'','',NULL,
                '{"type":"Point","coordinates":[10,20]}',10,20,10,20);
            BEGIN EXCLUSIVE;"#).unwrap();

        let ctx = egui::Context::default();
        ctx.style_mut(|s| s.animation_time = 0.);
        let mut model = AppModel::new();
        model.lidar = Browser::new(path);
        model.globe_view.local_mode = true;
        let coast = GeoPoint {
            lat: 38.,
            lon: -75.,
        };
        model.globe_view.local_center = coast;

        let open = click(&ctx, &mut model, "Lidar surveys");
        assert!(label(&open, "Searching surveys…").is_some());
        let closed = click(&ctx, &mut model, "Lidar surveys");
        assert!(label(&closed, "Search surveys").is_none());
        assert!(model.lidar.search_job.is_some());
        model.lidar.query = "Inland".into();
        model.lidar.request_search(&ctx, coast);

        model.lidar.enabled = true;
        model.lidar.last_check = Some(Instant::now());
        let menu = click(&ctx, &mut model, "Surveys at map center ▾");
        assert!(label(&menu, "Finding surveys…").is_some());
        model.globe_view.local_center = GeoPoint { lat: 20., lon: 10. };
        frame(&ctx, &mut model, vec![]);
        assert_eq!(model.lidar.picker_target, Some(coast));

        db.execute_batch("COMMIT").unwrap();
        wait_for(&ctx, &mut model, |b| {
            b.search_job.is_none() && b.picker_job.is_none()
        });
        assert_eq!(model.lidar.results.len(), 1);
        assert_eq!(model.lidar.results[0].name, "Inland flight");
        assert_eq!(model.lidar.picker_results.len(), 1);
        click(&ctx, &mut model, "1998  Coastal flight");
        assert_eq!(model.lidar.selected.unwrap().id, "NOAAID:1");
        assert_eq!(model.globe_view.local_center.lon, 10.);
    }

    #[test]
    #[ignore = "requires the Ocean City surveys in ONEKEE_TEST_LIDAR_DB"]
    fn open_both_survey_menus_on_live_catalog() {
        let ctx = egui::Context::default();
        ctx.style_mut(|s| s.animation_time = 0.);
        let mut model = AppModel::new();
        model.lidar = Browser::new(std::env::var_os("ONEKEE_TEST_LIDAR_DB").unwrap().into());
        model.globe_view.local_mode = true;
        model.globe_view.local_center = GeoPoint {
            lat: 38.33,
            lon: -75.0883,
        };

        click(&ctx, &mut model, "Lidar surveys");
        wait_for(&ctx, &mut model, |b| b.search_job.is_none());
        assert_eq!(model.lidar.results.len(), PAGE_SIZE);
        click(&ctx, &mut model, "Lidar surveys");
        model.lidar.enabled = true;
        model.lidar.last_check = Some(Instant::now());
        click(&ctx, &mut model, "Surveys at map center ▾");
        wait_for(&ctx, &mut model, |b| b.picker_job.is_none());
        assert!(
            model
                .lidar
                .picker_results
                .iter()
                .any(|s| s.id == "NOAAID:38")
        );
        let newest = &model.lidar.picker_results[0];
        let id = newest.id.clone();
        let title = newest.dated_name();
        click(&ctx, &mut model, &title);
        assert_eq!(model.lidar.selected.unwrap().id, id);
    }

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
        let at = GeoPoint {
            lat: -3.5,
            lon: -54.5,
        };
        let request = |near| Search {
            query: "Amazon".into(),
            near,
            offset: 0,
        };
        let all = search(&path, request(None)).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(select(&path, all[0].clone()).unwrap().2.lon, -60.);
        let nearby = search(&path, request(Some(at))).unwrap();
        assert_eq!(nearby.len(), 1);
        let (_, footprint, focus) = select(&path, nearby[0].clone()).unwrap();
        assert_eq!(focus.lon, -54.25);
        assert!(footprint.visible);
        db.execute(
            "UPDATE surveys SET id='NOAAID:1',date='1998' WHERE id='a'",
            [],
        )
        .unwrap();
        db.execute("INSERT INTO surveys SELECT 'NOAAID:2',source,name,product,'2018',page,asset,stac,geometry,west,south,east,north FROM surveys WHERE id='NOAAID:1'", []).unwrap();
        let choices = overlapping(
            &path,
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
                &path,
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
