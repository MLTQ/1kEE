use crate::camera_feed_viewer::CameraFeedViewer;
use crate::camera_registry;
use crate::deflock_source;
use crate::factal_stream;
use crate::fire_source;
use crate::model::AppModel;
use crate::panels;
use crate::panels::world_map::{contour_pass, globe_pass};
use crate::submarine_cables;
use crate::theme;
use crate::usgs_stream;
use std::sync::OnceLock;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Global egui context — lets fire-and-forget background threads wake the
// event loop when they finish, even when the window is on a hidden macOS Space.
// ---------------------------------------------------------------------------
static REPAINT_CTX: OnceLock<egui::Context> = OnceLock::new();

/// Called once at startup from `DashboardApp::new`.
pub(crate) fn register_repaint_ctx(ctx: &egui::Context) {
    let _ = REPAINT_CTX.set(ctx.clone());
}

/// Call this from any background thread that has just produced new data the
/// UI should show.  Safe to call from any thread, any number of times.
pub fn request_repaint() {
    if let Some(ctx) = REPAINT_CTX.get() {
        ctx.request_repaint();
    }
}

pub struct DashboardApp {
    pub(crate) model: AppModel,
    camera_feed_viewer: CameraFeedViewer,
    event_camera_sidebar: panels::EventCameraSidebar,
    event_radio: panels::EventRadio,
    last_theme: theme::MapTheme,
    /// Gruve mesh bridge: serves the companion web view + announces to the local
    /// agent. `None` when no port was free; the app runs identically without it.
    gruve: Option<crate::gruve::GruveBridge>,
}

impl DashboardApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        if let Some(state) = &cc.wgpu_render_state {
            Self::register_passes(
                &state.device,
                state.target_format,
                &mut state.renderer.write(),
            );
        }
        Self::initialize(&cc.egui_ctx, true)
    }

    pub(crate) fn register_passes(
        device: &eframe::wgpu::Device,
        format: eframe::wgpu::TextureFormat,
        renderer: &mut eframe::egui_wgpu::Renderer,
    ) {
        renderer
            .callback_resources
            .insert(globe_pass::GlobePassResources::new(device, format));
        renderer
            .callback_resources
            .insert(contour_pass::ContourPassResources::new(device, format));
        renderer.callback_resources.insert(
            panels::world_map::local_contour_pass::LocalContourPassResources::new(device, format),
        );
    }

    pub(crate) fn initialize(ctx: &egui::Context, announce: bool) -> Self {
        theme::install(ctx);
        register_repaint_ctx(ctx);
        crate::event_store::open();
        let model = AppModel::new();
        let gruve = announce
            .then(|| crate::gruve::GruveBridge::start(&model))
            .flatten();
        Self {
            last_theme: model.map_theme,
            model,
            camera_feed_viewer: CameraFeedViewer::default(),
            event_camera_sidebar: panels::EventCameraSidebar::default(),
            event_radio: panels::EventRadio::default(),
            gruve,
        }
    }
}

impl DashboardApp {
    pub(crate) fn tick(&mut self, ctx: &egui::Context) {
        self.model.poll_asset_inventory(ctx);
        self.model.global_weather.tick(ctx, self.model.active_body);
        panels::terrain_library::tick(ctx, &mut self.model);
        factal_stream::tick(&mut self.model);
        factal_stream::history_tick(&mut self.model);
        usgs_stream::tick(&mut self.model);
        camera_registry::tick(&mut self.model);
        deflock_source::tick(&mut self.model);
        self.model.flock.tick(self.model.selected_root.as_deref());
        submarine_cables::tick(&mut self.model);
        fire_source::tick(&mut self.model);
        self.model.buildout.tick();
        self.model.tick_event_follow(ctx.input(|i| i.time));

        // Atlas sunlight shares the app clock even when star overlays are hidden.
        let atlas_sun = self.model.map_theme == theme::MapTheme::Atlas
            && self.model.active_body == crate::model::ActiveBody::Earth
            && self.model.atlas_style.light_mode == crate::atlas_style::LightMode::Sun;
        if self.model.show_stellar_correspondence || atlas_sun {
            if self.model.stellar_live {
                let now_unix = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs_f64();
                self.model.stellar_jd = crate::stellar_time::unix_to_jd(now_unix);
            } else if self.model.stellar_anim_speed != 0.0 {
                let dt = ctx.input(|i| i.stable_dt) as f64;
                self.model.stellar_jd += self.model.stellar_anim_speed * dt;
                ctx.request_repaint();
            }
        }

        // Advance the replay playhead and request a repaint while running.
        if let Some(state) = &mut self.model.replay_state {
            let needs_repaint = state.tick();
            if needs_repaint {
                ctx.request_repaint();
            }
        }

        if self.model.map_theme != self.last_theme {
            theme::set_theme(ctx, self.model.map_theme);
            self.last_theme = self.model.map_theme;
        }
        // The USGS quake feed polls with no key configured, so the frame loop
        // must keep ticking even when no API keys are present.
        ctx.request_repaint_after(Duration::from_secs(1));
    }
}

impl Drop for DashboardApp {
    fn drop(&mut self) {
        factal_stream::shutdown();
        usgs_stream::shutdown();
        camera_registry::shutdown();
        deflock_source::shutdown();
        fire_source::shutdown();
        panels::world_map::srtm_focus_cache::terminate_active_gdal_jobs();
    }
}

impl eframe::App for DashboardApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Handle escape before hidden panels can consume it. Presentation mode
        // changes only visibility, so returning restores the operator's layout.
        ctx.input_mut(|input| {
            if self.model.obs_view && input.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                self.model.obs_view = false;
            } else if input.consume_key(egui::Modifiers::NONE, egui::Key::F10) {
                self.model.obs_view = !self.model.obs_view;
            }
        });

        // Apply any commands web viewers sent (steer globe / select event) before
        // the panels read the model this frame.
        if let Some(bridge) = &self.gruve {
            bridge.drain_commands(&mut self.model);
        }

        panels::layer_file_import::poll(ctx, &mut self.model);
        self.tick(ctx);

        self.event_camera_sidebar.render(ctx, &mut self.model);

        // Preserve the panel flags while hiding operator chrome for capture.
        if !self.model.obs_view {
            panels::render_layer_drawer(ctx, &mut self.model);
            if self.model.show_event_list {
                panels::render_event_list(ctx, &mut self.model, &mut self.event_radio);
            }
        }

        panels::render_factal_brief(ctx, &mut self.model, &mut self.event_radio);

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::canvas_background()))
            .show(ctx, |ui| {
                panels::render_world_map(ui, &mut self.model);
            });

        if !self.model.obs_view {
            panels::render_settings(ctx, &mut self.model);
            panels::render_stellar_observatory(ctx, &mut self.model);
        }
        self.camera_feed_viewer.render(ctx, &mut self.model);
        self.event_radio.render(ctx, self.model.selected_event());

        // Refresh the snapshot the companion web view polls, now that this frame's
        // interaction has been folded into the model.
        if let Some(bridge) = &self.gruve {
            bridge.publish(&self.model);
        }
    }
}
