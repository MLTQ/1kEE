//! Native map and analyst workflows shared with the OpenXR viewer.
use crate::{app::DashboardApp, model::GeoPoint, panels, theme};
use eframe::{egui_wgpu, wgpu};
use std::path::Path;

#[path = "vr_files.rs"]
mod files;

pub const HEIGHT_EDGE: usize = 65;
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Event(String),
    Camera(String),
}
#[derive(Clone, Debug)]
pub struct Spire {
    /// Latitude and longitude in degrees, in the same datum as the map.
    pub location: [f32; 2],
    pub color: [u8; 3],
    pub target: Option<Target>,
}
#[derive(Clone, Copy, Debug)]
pub struct GlobeCapture {
    pub lat: f32,
    pub lon: f32,
    /// Distance of the capture camera from the center, in globe radii.
    pub distance: f32,
}
#[derive(Default, Clone)]
pub struct MapFrame {
    pub local: bool,
    pub projection: [f32; 4],
    pub angles: [f32; 2],
    pub bounds: [f32; 4],
    pub heights: Vec<f32>,
    pub spires: Vec<Spire>,
    pub marker_scale: f32,
    pub globe_scale: f32,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub lat: f32,
    pub lon: f32,
    pub scale: f32,
    pub local: bool,
    pub local_zoom: f32,
}
#[derive(Clone, Copy, PartialEq)]
enum Page {
    View,
    Layers,
    Imagery,
    Events,
    Brief,
    Cameras,
    Settings,
}

pub struct Operator {
    app: DashboardApp,
    page: Page,
    files: files::Browser,
    scale: f32,
    pub recenter: u64,
    pub quit: bool,
    camera: crate::camera_feed_player::CameraFeedPlayer,
    playing: Option<String>,
    automatic_camera: bool,
    room: bool,
    archive: Option<std::path::PathBuf>,
    hydrated: Option<String>,
}
impl Operator {
    pub fn new(
        ctx: &egui::Context,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        renderer: &mut egui_wgpu::Renderer,
        focus: [f32; 2],
        asset_root: Option<&Path>,
        events: Option<&Path>,
    ) -> Result<Self, String> {
        DashboardApp::register_passes(device, format, renderer);
        let mut app = DashboardApp::initialize(ctx, false);
        let model = &mut app.model;
        model.map_theme = theme::MapTheme::Atlas;
        theme::set_theme(ctx, model.map_theme);
        let archived = if let Some(path) = events {
            crate::event_store::read_recent_from(path)?
        } else {
            let last = crate::event_store::newest_event_unix().unwrap_or(0);
            crate::event_store::load_events_in_range(last - 86400, last)
        };
        model.replace_factal_events(archived.into_iter().rev().map(|(_, e)| e).collect());
        model.globe_view.focus_on(GeoPoint {
            lat: focus[0],
            lon: focus[1],
        });
        model.globe_view.zoom = 1.45;
        let mut operator = Self {
            app,
            page: Page::View,
            files: Default::default(),
            scale: 1.,
            recenter: 0,
            quit: false,
            camera: Default::default(),
            playing: None,
            automatic_camera: true,
            room: false,
            archive: events.map(Path::to_owned),
            hydrated: None,
        };
        if let Some(path) = asset_root {
            operator.use_asset_root(path);
        }
        Ok(operator)
    }
    /// Change this session’s source drive without rewriting the desktop settings.
    pub fn use_asset_root(&mut self, root: &Path) {
        let model = &mut self.app.model;
        model.selected_root = Some(root.to_owned());
        model.request_asset_inventory();
        let data = crate::settings_store::configured_data_root();
        model
            .global_weather
            .set_cache_path(panels::world_map::global_weather::cache_path(
                data.as_ref().and_then(|p| p.to_str()),
                Some(root),
            ));
    }
    pub fn view(&self) -> View {
        let v = &self.app.model.globe_view;
        let p = if v.local_mode {
            v.local_center
        } else {
            v.globe_center_latlon()
        };
        View {
            lat: p.lat,
            lon: p.lon,
            scale: self.scale,
            local: v.local_mode,
            local_zoom: v.local_zoom,
        }
    }
    pub fn navigate(&mut self, v: View) {
        let m = &mut self.app.model;
        m.stop_event_follow();
        m.globe_view.stop_motion();
        let p = GeoPoint {
            lat: v.lat.clamp(-89.9, 89.9),
            lon: (v.lon + 180.).rem_euclid(360.) - 180.,
        };
        m.globe_view.yaw = p.lon.to_radians() - std::f32::consts::FRAC_PI_2;
        m.globe_view.pitch = p.lat.to_radians();
        m.globe_view.local_center = p;
        m.globe_view.local_mode = v.local;
        m.globe_view.local_zoom = v.local_zoom.clamp(1., 60.);
        self.scale = v.scale.clamp(1., 32.);
        m.globe_view.zoom = (1.45 * self.scale).clamp(0.6, 50.);
    }
    pub fn pick(&mut self, target: Target) {
        self.room = true;
        match target {
            Target::Event(id) => {
                self.app.model.select_event(&id);
                if let Some(event) = self.app.model.selected_event() {
                    let focus = event.location;
                    self.app.model.globe_view.focus_on(focus);
                }
                self.page = Page::Brief;
            }
            Target::Camera(id) => {
                self.app.model.select_camera(&id);
                self.playing = Some(id);
                self.automatic_camera = false;
                self.page = Page::Cameras;
            }
        }
    }
    pub fn room_visible(&self) -> bool {
        self.room
    }
    pub fn paint_room(&self, ctx: &egui::Context) {
        let m = &self.app.model;
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.columns(2, |cols| {
                let ui = &mut cols[0];
                if let Some(recap) = &m.follow_recaps.active {
                    ui.heading("Regional recap");
                    ui.label(format!("{:?}", recap.region));
                } else if let Some(e) = m.selected_event() {
                    ui.colored_label(
                        e.severity.color(),
                        format!("{} · {}", e.severity.label(), e.source),
                    );
                    ui.heading(&e.title);
                    ui.label(&e.location_name);
                    ui.label(&e.summary);
                } else {
                    ui.label("Select an event to open its brief.");
                }
                let ui = &mut cols[1];
                if let Some(c) = self
                    .playing
                    .as_ref()
                    .and_then(|id| m.cameras.iter().find(|c| &c.id == id))
                {
                    ui.strong(&c.label);
                    if let Some(t) = &self.camera.texture {
                        ui.add(egui::Image::new(t).max_size(ui.available_size()));
                    } else {
                        ui.label(self.camera.error.as_ref().unwrap_or(&self.camera.status));
                    }
                } else {
                    ui.label("No webcam selected");
                }
            });
        });
    }
    pub fn import(&mut self, path: std::path::PathBuf) {
        panels::layer_file_import::queue(path, &mut self.app.model, 4096);
    }
    pub fn tick(&mut self, ctx: &egui::Context, wrist_visible: bool) {
        self.app.tick(ctx);
        if self.app.model.event_follow.enabled() {
            self.scale = (self.app.model.globe_view.zoom / 1.45).clamp(1., 32.);
        }
        if (self.room_visible() || self.page == Page::Brief)
            && let Some(id) = self.app.model.selected_event_id.as_deref()
            && self.hydrated.as_deref() != Some(id)
        {
            self.hydrated = Some(id.to_owned());
            if let Some(factal_id) = id.strip_prefix("hist-")
                && let Some(raw) = crate::event_store::raw_brief(self.archive.as_deref(), factal_id)
                && let Some(e) = self.app.model.events.iter_mut().find(|e| e.id == id)
                && let Some(b) = &mut e.factal_brief
            {
                if let Some(brief) = crate::factal_stream::stored_brief(&raw) {
                    *b = brief;
                } else {
                    b.raw_json_pretty = raw;
                }
            }
        }
        if wrist_visible {
            self.wrist(ctx);
        }
        self.update_playback(ctx, wrist_visible);
    }
    fn update_playback(&mut self, ctx: &egui::Context, wrist_visible: bool) {
        let m = &self.app.model;
        if self.app.model.follow_recaps.active.is_some() {
            self.playing = None;
            self.camera.stop();
        } else if self.room_visible() || (wrist_visible && self.page == Page::Cameras) {
            if self.automatic_camera {
                self.playing = m
                    .nearby_camera_snapshot(2000.)
                    .iter()
                    .find(|c| panels::nearby_preview(c))
                    .map(|c| c.id.clone());
            }
            if let Some(c) = self
                .playing
                .as_ref()
                .and_then(|id| m.cameras.iter().find(|c| &c.id == id))
            {
                self.camera.sync(ctx, c);
            } else {
                self.camera.stop();
            }
        } else {
            self.camera.stop();
        }
    }
    pub fn paint_map(&mut self, ctx: &egui::Context, capture: Option<GlobeCapture>) -> MapFrame {
        panels::layer_file_import::poll(ctx, &mut self.app.model);
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                panels::world_map::paint_vr(ui, &mut self.app.model, capture, self.scale)
            })
            .inner
    }
    pub fn wrist(&mut self, ctx: &egui::Context) {
        use egui::{Color32, Frame, Margin, RichText, Stroke};
        let bar = Color32::from_rgb(23, 40, 47);
        let accent = Color32::from_rgb(151, 228, 208);
        let nav = Frame::new()
            .fill(bar)
            .inner_margin(Margin::symmetric(14, 10));
        egui::TopBottomPanel::top("wrist-navigation")
            .exact_height(68.)
            .frame(nav)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.x = 5.;
                ui.horizontal(|ui| {
                    let width = (ui.available_width() - 5. * ui.spacing().item_spacing.x) / 6.;
                    for (page, name) in [
                        (Page::View, "Navigate"),
                        (Page::Layers, "Layers"),
                        (Page::Imagery, "Satellite"),
                        (Page::Events, "Events"),
                        (Page::Brief, "Brief"),
                        (Page::Cameras, "Webcams"),
                    ] {
                        let active = self.page == page;
                        let text = RichText::new(name).size(19.).strong().color(if active {
                            bar
                        } else {
                            Color32::WHITE
                        });
                        let tab = egui::Button::new(text).fill(if active {
                            accent
                        } else {
                            Color32::from_rgb(46, 65, 73)
                        });
                        let response = ui.add_sized([width, 48.], tab);
                        if response.hovered() {
                            ui.painter().rect_stroke(
                                response.rect,
                                4.,
                                Stroke::new(2., Color32::WHITE),
                                egui::StrokeKind::Inside,
                            );
                        }
                        if response.clicked() {
                            self.page = page;
                        }
                    }
                });
            });
        // Fixed controls stay in reach while long layer lists and briefs scroll.
        let scroll_id = egui::Id::new(("wrist-scroll-range", self.page as u8));
        let (up, down) = ctx
            .data(|d| d.get_temp::<(bool, bool)>(scroll_id))
            .unwrap_or_default();
        let mut scroll = 0.;
        egui::TopBottomPanel::bottom("wrist-footer")
            .exact_height(50.)
            .frame(
                Frame::new()
                    .fill(bar)
                    .inner_margin(Margin::symmetric(16, 6)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Trigger to select  ·  Stick to scroll")
                            .size(16.)
                            .color(Color32::WHITE),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for (label, enabled, direction) in
                            [("Page down", down, -1.), ("Page up", up, 1.)]
                        {
                            let response = ui.add_enabled(
                                enabled,
                                egui::Button::new(RichText::new(label).size(16.).color(
                                    if enabled {
                                        Color32::WHITE
                                    } else {
                                        Color32::GRAY
                                    },
                                ))
                                .min_size(egui::vec2(104., 38.))
                                .fill(Color32::from_rgb(46, 65, 73)),
                            );
                            if response.clicked() {
                                scroll = direction;
                            }
                        }
                    });
                });
            });
        let panel = Frame::new()
            .fill(ctx.style().visuals.panel_fill)
            .inner_margin(Margin::same(16));
        egui::CentralPanel::default().frame(panel).show(ctx, |ui| {
            let style = ui.style_mut();
            style.spacing.interact_size.y = 42.;
            style.spacing.button_padding = egui::vec2(12., 8.);
            style.spacing.item_spacing = egui::vec2(8., 8.);
            style.spacing.slider_width = 220.;
            style.spacing.icon_width = 22.;
            if !style.visuals.dark_mode {
                style.visuals.widgets.inactive.bg_stroke =
                    Stroke::new(1., Color32::from_rgb(131, 137, 128));
                style.visuals.widgets.hovered.bg_stroke =
                    Stroke::new(2., Color32::from_rgb(19, 96, 98));
            }
            for text in [egui::TextStyle::Body, egui::TextStyle::Button] {
                style.text_styles.get_mut(&text).unwrap().size = 19.;
            }
            style
                .text_styles
                .get_mut(&egui::TextStyle::Small)
                .unwrap()
                .size = 15.;
            let delta = scroll * ui.available_height() * 0.8;
            let area = egui::ScrollArea::vertical()
                .id_salt(("wrist-content", self.page as u8))
                .auto_shrink([false, false])
                .drag_to_scroll(false)
                .show(ui, |ui| {
                    if delta != 0. {
                        ui.scroll_with_delta(egui::vec2(0., delta));
                    }
                    self.page_ui(ui);
                });
            let y = area.state.offset.y;
            ctx.data_mut(|d| {
                d.insert_temp(
                    scroll_id,
                    (
                        y > 1.,
                        y + area.inner_rect.height() < area.content_size.y - 1.,
                    ),
                )
            });
        });
    }
    fn page_ui(&mut self, ui: &mut egui::Ui) {
        let m = &mut self.app.model;
        if self.page == Page::Imagery {
            let selection = ui
                .add_enabled_ui(!panels::layer_file_import::busy(), |ui| {
                    self.files.ui(ui, m.selected_root.as_deref())
                })
                .inner;
            if let Some(path) = selection {
                panels::layer_file_import::queue(path, m, 4096);
            }
            ui.separator();
        }
        match self.page {
            Page::View => {
                let mut view = self.view();
                let previous = view;
                ui.heading("Map view");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut view.local, false, "Globe");
                    ui.selectable_value(&mut view.local, true, "Local terrain");
                });
                ui.horizontal(|ui| {
                    let step = if view.local {
                        local_extent(view.local_zoom) * 0.5
                    } else {
                        10. / view.scale
                    };
                    for (name, lat, lon) in [
                        ("West", 0., -1.),
                        ("North", 1., 0.),
                        ("South", -1., 0.),
                        ("East", 0., 1.),
                    ] {
                        if ui.add_sized([95., 42.], egui::Button::new(name)).clicked() {
                            view.lat += lat * step;
                            view.lon += lon * step;
                        }
                    }
                });
                if view.local {
                    ui.add(
                        egui::Slider::new(&mut view.local_zoom, 1.0..=60.)
                            .logarithmic(true)
                            .text("Terrain zoom"),
                    );
                } else {
                    ui.add(
                        egui::Slider::new(&mut view.scale, 1.0..=32.)
                            .logarithmic(true)
                            .text("Globe zoom"),
                    );
                }
                if view != previous {
                    self.navigate(view);
                }
                let m = &mut self.app.model;
                ui.horizontal(|ui| {
                    if ui.button("Recenter table").clicked() {
                        self.recenter += 1;
                    }
                    if ui
                        .add_enabled(
                            m.selected_event().is_some(),
                            egui::Button::new("Focus event"),
                        )
                        .clicked()
                        && let Some(event) = m.selected_event()
                    {
                        let p = event.location;
                        m.globe_view.focus_on(p);
                    }
                    if ui.button("Settings").clicked() {
                        self.page = Page::Settings;
                    }
                    if ui.button("Exit VR").clicked() {
                        self.quit = true;
                    }
                });
                ui.label(format!(
                    "{:.3}° latitude    {:.3}° longitude",
                    view.lat, view.lon
                ));
                ui.separator();
                ui.collapsing(format!("Map theme · {}", m.map_theme.label()), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for &t in theme::MapTheme::ALL {
                            ui.selectable_value(&mut m.map_theme, t, t.label());
                        }
                    });
                });
                ui.add_enabled(
                    view.local,
                    egui::Slider::new(&mut m.globe_view.local_layer_spread, 0.0..=3.)
                        .text("Local relief"),
                );
                ui.collapsing("Atlas lighting", |ui| {
                    panels::settings::vr_atlas_controls(ui, &mut m.atlas_style)
                });
                ui.collapsing("Replay", |ui| {
                    if ui
                        .button(if m.replay_mode {
                            "End replay"
                        } else {
                            "Start replay"
                        })
                        .clicked()
                    {
                        m.toggle_replay();
                    }
                    panels::render_replay_controls(ui, m);
                });
                follow(ui, m, &mut self.room);
                if let Some(message) = ui
                    .ctx()
                    .data(|d| d.get_temp::<String>(egui::Id::new("vr-terrain-status")))
                {
                    ui.label(message);
                }
            }
            Page::Settings => panels::settings::wrist(ui, m),
            Page::Imagery => {
                panels::world_map::global_weather::controls(ui, m);
                ui.separator();
                panels::layer_drawer::imported_controls(ui, m);
            }
            Page::Layers => {
                panels::layer_drawer::map_controls(ui, m);
                ui.separator();
                if ui.button("Import layers…").clicked() {
                    self.page = Page::Imagery;
                }
            }
            Page::Events => {
                follow(ui, m, &mut self.room);
                ui.label(format!(
                    "{} events · {}",
                    m.events.len(),
                    m.factal_stream_status
                ));
                let mut pick = None;
                let page = egui::Id::new("wrist-event-list-page");
                let mut index = ui.ctx().data(|d| d.get_temp::<usize>(page)).unwrap_or(0);
                let count = m.events.len().div_ceil(10).max(1);
                index = index.min(count - 1);
                if count > 1 {
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(index > 0, egui::Button::new("Previous"))
                            .clicked()
                        {
                            index -= 1;
                        }
                        ui.label(format!("{} / {count}", index + 1));
                        if ui
                            .add_enabled(index + 1 < count, egui::Button::new("Next"))
                            .clicked()
                        {
                            index += 1;
                        }
                    });
                }
                ui.ctx().data_mut(|d| d.insert_temp(page, index));
                for e in m.events.iter().skip(index * 10).take(10) {
                    if ui
                        .add_sized(
                            [ui.available_width(), 56.],
                            egui::Button::new(format!(
                                "{} · {}\n{}",
                                e.severity.label(),
                                e.location_name,
                                e.title
                            ))
                            .truncate(),
                        )
                        .clicked()
                    {
                        pick = Some(e.id.clone());
                    }
                }
                if let Some(id) = pick {
                    self.pick(Target::Event(id));
                }
            }
            Page::Brief => {
                ui.checkbox(&mut self.room, "Pin brief and webcam above the map");
                follow(ui, m, &mut self.room);
                if let Some(e) = m.selected_event() {
                    ui.heading(&e.title);
                    ui.colored_label(e.severity.color(), e.severity.label());
                    ui.label(format!(
                        "{} · {} · {}",
                        e.location_name, e.occurred_at, e.source
                    ));
                    ui.label(&e.summary);
                    if let Some(b) = &e.factal_brief {
                        if let Some(content) = &b.content {
                            ui.label(content);
                        }
                        if !b.topics.is_empty() {
                            ui.label(b.topics.join(" · "));
                        }
                        if !b.raw_json_pretty.is_empty() {
                            ui.collapsing("Source record", |ui| {
                                ui.monospace(&b.raw_json_pretty);
                            });
                        }
                    }
                    if ui.button("Nearby webcams").clicked() {
                        self.automatic_camera = true;
                        self.page = Page::Cameras;
                    }
                } else {
                    ui.label("Select an event from the globe or event list.");
                }
            }
            Page::Cameras => {
                ui.checkbox(&mut self.room, "Pin brief and webcam above the map");
                ui.checkbox(
                    &mut self.automatic_camera,
                    "Play nearest webcam to selected event",
                );
                let near = m.nearby_camera_snapshot(2000.);
                if let Some(c) = self
                    .playing
                    .as_ref()
                    .and_then(|id| m.cameras.iter().find(|c| &c.id == id))
                {
                    ui.heading(&c.label);

                    if let Some(texture) = &self.camera.texture {
                        ui.add(
                            egui::Image::new(texture)
                                .max_size(egui::vec2(ui.available_width(), 220.)),
                        );
                    }
                    ui.label(self.camera.error.as_ref().unwrap_or(&self.camera.status));
                    if ui.button("Stop feed").clicked() {
                        self.automatic_camera = false;
                        self.playing = None;
                        self.camera.stop();
                    }
                } else {
                    self.camera.stop();
                    ui.label(&m.camera_registry_status);
                }
                ui.separator();
                for c in near.iter().filter(|c| panels::nearby_preview(c)).take(24) {
                    if ui
                        .button(format!("{} · {:.0} km", c.label, c.distance_km))
                        .clicked()
                    {
                        self.automatic_camera = false;
                        self.playing = Some(c.id.clone());
                    }
                }
                ui.collapsing(format!("Camera registry ({})", m.cameras.len()), |ui| {
                    egui::ScrollArea::vertical().max_height(240.).show_rows(
                        ui,
                        42.,
                        m.cameras.len(),
                        |ui, range| {
                            for c in &m.cameras[range] {
                                if ui
                                    .add_sized(
                                        [ui.available_width(), 42.],
                                        egui::Button::new(&c.label).truncate(),
                                    )
                                    .clicked()
                                {
                                    self.automatic_camera = false;
                                    self.playing = Some(c.id.clone());
                                }
                            }
                        },
                    );
                });
            }
        }
    }
}

fn follow(ui: &mut egui::Ui, m: &mut crate::model::AppModel, room: &mut bool) {
    ui.horizontal(|ui| {
        let mut enabled = m.event_follow.enabled();
        if ui.checkbox(&mut enabled, "Follow Events").changed() {
            m.toggle_event_follow();
            *room = m.event_follow.enabled();
        }
        let mut seconds = m.event_follow.dwell_seconds();
        if ui
            .add(egui::Slider::new(&mut seconds, 5..=300).text("Dwell (s)"))
            .changed()
        {
            m.event_follow.set_dwell_seconds(seconds);
        }
    });
}

pub fn local_extent(zoom: f32) -> f32 {
    panels::world_map::vr_local_extent(zoom)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        time::{Duration, Instant},
    };
    #[test]
    #[ignore = "isolated native session and loopback webcam"]
    fn selected_webcam_decodes_and_paints_its_frame_in_the_room() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let url = format!("http://{}/snapshot.png", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut buf = [0; 2048];
            let _ = stream.read(&mut buf).unwrap();
            let image = image::RgbImage::from_pixel(24, 12, image::Rgb([20, 180, 240]));
            let mut bytes = std::io::Cursor::new(vec![]);
            image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
            let data = bytes.get_ref();
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",data.len()).unwrap();
            stream.write_all(data).unwrap();
        });
        let ctx = egui::Context::default();
        let mut app = DashboardApp::initialize(&ctx, false);
        app.model.cameras = vec![crate::model::CameraFeed {
            id: "preview".into(),
            label: "Preview".into(),
            provider: "Local fixture".into(),
            kind: "Snapshot".into(),
            location: GeoPoint {
                lat: 46.8,
                lon: -121.8,
            },
            stream_url: url,
            last_seen: String::new(),
            status: crate::model::CameraConnectionState::Reachable,
        }];
        let mut operator = Operator {
            app,
            page: Page::View,
            files: Default::default(),
            scale: 1.,
            recenter: 0,
            quit: false,
            camera: Default::default(),
            playing: None,
            automatic_camera: true,
            room: false,
            archive: None,
            hydrated: None,
        };
        operator.pick(Target::Camera("preview".into()));
        let start = Instant::now();
        while operator.camera.texture.is_none() {
            operator.update_playback(&ctx, false);
            assert!(
                start.elapsed() < Duration::from_secs(8),
                "webcam: {}",
                operator.camera.status
            );
            std::thread::sleep(Duration::from_millis(15));
        }
        server.join().unwrap();
        let t = operator.camera.texture.as_ref().unwrap();
        assert_eq!(t.size(), [24, 12]);
        let id = t.id();
        let out = ctx.run(Default::default(), |ctx| operator.paint_room(ctx));
        assert!(ctx.tessellate(out.shapes, out.pixels_per_point).iter().any(
            |p| matches!(&p.primitive,egui::epaint::Primitive::Mesh(mesh) if mesh.texture_id==id)
        ));
        operator.page = Page::View;
        operator.room = false;
        operator.update_playback(&ctx, false);
        assert!(operator.camera.texture.is_none());
    }
}
