//! Survey hit testing for the map canvas. A short screen-space radius makes thin
//! flight strips pickable from either side, including on the oblique local map.
use super::*;
use egui::{Pos2, Vec2};
use std::collections::HashMap;

const RADIUS: f32 = 5.;
const HOVER_DELAY: Duration = Duration::from_millis(240);

fn longitude_delta(d: f32) -> f32 {
    if d.abs() <= 180. {
        d
    } else {
        (d + 180.).rem_euclid(360.) - 180.
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct PickRegion {
    pub(crate) at: GeoPoint,
    dx: Vec2,
    dy: Vec2,
}

impl PickRegion {
    pub(super) fn point(at: GeoPoint) -> Self {
        Self {
            at,
            dx: Vec2::ZERO,
            dy: Vec2::ZERO,
        }
    }

    pub(crate) fn from_screen(
        pos: Pos2,
        unproject: impl Fn(Pos2) -> Option<GeoPoint>,
    ) -> Option<Self> {
        let at = unproject(pos)?;
        let step = |v: Vec2| {
            let (geo, sign) = unproject(pos + v)
                .map(|p| (p, 1.))
                .or_else(|| unproject(pos - v).map(|p| (p, -1.)))?;
            Some(egui::vec2(longitude_delta(geo.lon - at.lon), geo.lat - at.lat) * sign)
        };
        Some(Self {
            at,
            dx: step(Vec2::X * RADIUS).unwrap_or_default(),
            dy: step(Vec2::Y * RADIUS).unwrap_or_default(),
        })
    }

    fn offset(&self, p: GeoPoint) -> Vec2 {
        egui::vec2(longitude_delta(p.lon - self.at.lon), p.lat - self.at.lat)
    }

    fn nearby(&self, a: GeoPoint, b: GeoPoint) -> Option<(f32, GeoPoint)> {
        let det = self.dx.x * self.dy.y - self.dy.x * self.dx.y;
        if det == 0. {
            return (self.at == a).then_some((0., a));
        }
        let units = |d: Vec2| {
            egui::vec2(
                d.x * self.dy.y - d.y * self.dy.x,
                self.dx.x * d.y - self.dx.y * d.x,
            ) / det
        };
        let start = units(self.offset(a));
        let step = egui::vec2(longitude_delta(b.lon - a.lon), b.lat - a.lat);
        let edge = units(step);
        let t = (-start.dot(edge) / edge.length_sq().max(f32::MIN_POSITIVE)).clamp(0., 1.);
        let distance = (start + edge * t).length_sq();
        (distance <= 1.).then_some((
            distance,
            GeoPoint {
                lon: longitude_delta(a.lon + step.x * t),
                lat: a.lat + step.y * t,
            },
        ))
    }

    fn hit(&self, geometry: &GeoJsonGeometry) -> Option<(f32, GeoPoint)> {
        let ring_hit = |rings: &[Vec<GeoPoint>]| {
            if covers(rings, self.at) {
                return Some((0., self.at));
            }
            rings
                .iter()
                .flat_map(|r| r.windows(2))
                .filter_map(|e| self.nearby(e[0], e[1]))
                .min_by(|a, b| a.0.total_cmp(&b.0))
        };
        match geometry {
            GeoJsonGeometry::Point(p) => self.nearby(*p, *p),
            GeoJsonGeometry::Polygon(rings) => ring_hit(rings),
            GeoJsonGeometry::MultiPolygon(parts) => parts
                .iter()
                .filter_map(|p| ring_hit(p))
                .min_by(|a, b| a.0.total_cmp(&b.0)),
            _ => None,
        }
    }
}

pub(super) struct Hit {
    pub survey: Survey,
    label: String,
    focus: GeoPoint,
    available: bool,
}

pub(super) fn lookup(path: &Path, region: PickRegion, archives: bool) -> Result<Vec<Hit>, String> {
    let db = connection(path)?;
    let bulk = connection(&path.with_file_name("bulk.sqlite")).ok();
    let predicate = directory::MAPPABLE;
    let mut sql = format!(
        "SELECT id,name,product,date,page,asset,source,geometry,coalesce({predicate},0)
          FROM surveys WHERE west<=?1 AND east>=?2 AND south<=?3 AND north>=?4"
    );
    if !archives {
        sql.push_str(" AND ");
        sql.push_str(predicate);
    }
    let mut query = db.prepare(&sql).map_err(|e| e.to_string())?;
    let rx = region.dx.x.hypot(region.dy.x).min(180.);
    let ry = region.dx.y.hypot(region.dy.y);
    let (w, e) = (region.at.lon - rx, region.at.lon + rx);
    let mut ranges = vec![(w.max(-180.), e.min(180.))];
    if w <= -180. {
        ranges.push((w + 360., 180.));
    }
    if e >= 180. {
        ranges.push((-180., e - 360.));
    }
    let mut surveys = HashMap::<String, (f32, Hit)>::new();
    for (west, east) in ranges {
        let mut rows = query
            .query(params![east, west, region.at.lat + ry, region.at.lat - ry])
            .map_err(|e| e.to_string())?;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let geom: String = row.get(7).map_err(|e| e.to_string())?;
            let shape = GeoJsonLayer::parse(String::new(), &geom)?;
            let Some((distance, focus)) = shape
                .features
                .iter()
                .filter_map(|f| region.hit(&f.geometry))
                .min_by(|a, b| a.0.total_cmp(&b.0))
            else {
                continue;
            };
            let mut survey = directory::survey_row(row).map_err(|e| e.to_string())?;
            let mut available: bool = row.get(8).map_err(|e| e.to_string())?;
            if survey.id.starts_with("EBA:") {
                available = bulk.as_ref().is_some_and(|db| {
                    db.query_row(
                        "SELECT 1 FROM transects WHERE survey=? LIMIT 1",
                        [&survey.id],
                        |_| Ok(()),
                    )
                    .is_ok()
                });
            }
            if !archives && !available {
                continue;
            }
            if let Some((id, name)) = directory::collection(&survey.id, &survey.name) {
                survey.id = id;
                survey.name = name;
            }
            // Adjacent LINZ tiles represent one acquisition. Focus its closest actual
            // footprint even if an outer tile edge was the first database match.
            if surveys
                .get(&survey.id)
                .is_some_and(|(best, _)| *best <= distance)
            {
                continue;
            }
            let label = directory::label(&survey.name, survey.date.as_deref());
            let id = survey.id.clone();
            surveys.insert(
                id,
                (
                    distance,
                    Hit {
                        survey,
                        label,
                        focus,
                        available,
                    },
                ),
            );
        }
    }
    let mut hits: Vec<_> = surveys.into_values().map(|(_, hit)| hit).collect();
    hits.sort_by_cached_key(|h| (!h.available, h.label.to_lowercase(), h.survey.id.clone()));
    Ok(hits)
}

#[derive(Default)]
pub(super) struct MapPick {
    hover: Option<(PickRegion, Instant)>,
    reading: Option<PickRegion>,
    job: Option<ReadJob<Vec<Hit>>>,
    cached: Option<(PickRegion, Result<Vec<Hit>, String>)>,
    click: Option<(PickRegion, Pos2)>,
}

impl MapPick {
    pub(super) fn invalidate(&mut self) {
        self.cached = None;
        self.reading = None;
        self.job = None;
    }

    fn update(&mut self, ctx: &egui::Context, path: &Path, pointer: Option<PickRegion>) {
        if self.hover.map(|h| h.0) != pointer {
            self.hover = pointer.map(|p| (p, Instant::now()));
        }
        if let Some(done) = completed(&mut self.job) {
            self.cached = self.reading.take().map(|p| (p, done));
        }
        let target = self.click.map(|c| c.0).or_else(|| {
            let (p, since) = self.hover?;
            if let Some(delay) = HOVER_DELAY.checked_sub(since.elapsed()) {
                ctx.request_repaint_after(delay);
                None
            } else {
                Some(p)
            }
        });
        if let Some(target) = target
            && self.job.is_none()
            && !self.cached.as_ref().is_some_and(|(p, _)| *p == target)
        {
            let path = path.to_owned();
            self.reading = Some(target);
            self.job = Some(read_in_background(ctx, move || lookup(&path, target, true)));
        }
    }

    fn result(&self, region: PickRegion) -> Option<&Result<Vec<Hit>, String>> {
        self.cached
            .as_ref()
            .filter(|(p, _)| *p == region)
            .map(|r| &r.1)
    }
}

pub(crate) fn map_interaction(
    ui: &egui::Ui,
    response: &egui::Response,
    model: &mut AppModel,
    pointer: Option<PickRegion>,
) {
    let ctx = ui.ctx();
    let clicked = response.secondary_clicked();
    if clicked && let (Some(region), Some(pos)) = (pointer, response.interact_pointer_pos()) {
        model.stop_event_follow();
        model.globe_view.stop_motion();
        model.lidar.selection_job = None;
        let pick = &mut model.lidar.map_pick;
        if pick.result(region).is_some_and(Result::is_err) {
            pick.cached = None;
        }
        pick.click = Some((region, pos));
    }
    let b = &mut model.lidar;
    let pick = &mut b.map_pick;
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) || response.dragged() {
        pick.click = None;
    }
    pick.update(ctx, &b.path, pointer);
    let Some((target, pos)) = pick.click else {
        if let Some(Ok(hits)) = pointer.and_then(|p| pick.result(p)) {
            let count = hits.iter().take_while(|h| h.available).count();
            if count > 0 {
                response
                    .clone()
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text_at_pointer(if hits.len() == 1 {
                        format!("{}\nRight-click to load lidar", hits[0].label)
                    } else {
                        format!("{} overlapping surveys · right-click to choose", hits.len())
                    });
            } else if !hits.is_empty() {
                response.clone().on_hover_text_at_pointer(
                    "Archive coverage · right-click for the survey record",
                );
            }
        }
        return;
    };
    let result = pick.result(target);
    let hits = result
        .and_then(|r| r.as_ref().ok())
        .map(Vec::as_slice)
        .unwrap_or_default();
    let count = hits.iter().take_while(|h| h.available).count();
    let mut choice = None;
    if hits.len() == 1 && count == 1 {
        choice = Some(&hits[0]);
    } else {
        let area = egui::Area::new(egui::Id::new("lidar-map-choice"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .movable(false)
            .constrain(true)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| match result {
                    None => {
                        ui.label("Finding surveys…");
                    }
                    Some(Err(e)) => {
                        ui.label(e);
                    }
                    Some(Ok(_)) if hits.is_empty() => {
                        ui.label("No survey at this point");
                    }
                    Some(Ok(_)) => {
                        ui.set_width((ctx.screen_rect().width() - 24.).clamp(1., 380.));
                        ui.strong(if count == 0 {
                            "Archive coverage"
                        } else {
                            "Choose a lidar survey"
                        });
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);

                        let height = ui.spacing().interact_size.y;
                        egui::ScrollArea::vertical().max_height(260.).show_rows(
                            ui,
                            height,
                            hits.len(),
                            |ui, range| {
                                for h in &hits[range] {
                                    // Keep acquisition dates legible even when long flight names truncate.
                                    let title = match h.label.rsplit_once(" · ") {
                                        Some((name, date))
                                            if date.starts_with(|c: char| c.is_ascii_digit()) =>
                                        {
                                            format!("{date}   {name}")
                                        }
                                        _ => h.label.clone(),
                                    };
                                    if h.available {
                                        if ui
                                            .selectable_label(
                                                b.selected
                                                    .as_ref()
                                                    .is_some_and(|s| s.id == h.survey.id),
                                                &title,
                                            )
                                            .on_hover_text(format!("{}\n{}", h.label, h.survey.id))
                                            .clicked()
                                        {
                                            choice = Some(h);
                                        }
                                    } else {
                                        ui.hyperlink_to(title, &h.survey.page).on_hover_text(
                                            format!("{}\nOpen archive record", h.label),
                                        );
                                    }
                                }
                            },
                        );
                    }
                });
            });
        if !clicked
            && ctx.input(|i| {
                i.pointer.any_pressed()
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|p| !area.response.rect.contains(p))
            })
        {
            pick.click = None;
            return;
        }
    }
    if let Some(hit) = choice {
        let at = hit.focus;
        let survey = hit.survey.clone();
        b.pin(survey);
        if b.tiles.at_overview() {
            model.globe_view.focus_on(at);
            model.globe_view.local_mode = true;
            model.globe_view.local_zoom = model.globe_view.local_zoom.max(20.);
        }
    }
}

#[cfg(test)]
mod tests;
