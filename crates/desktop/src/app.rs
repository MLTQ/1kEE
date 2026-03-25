use crate::camera_registry;
use crate::factal_stream;
use crate::model::AppModel;
use crate::panels;
use crate::panels::world_map::globe_pass;
use crate::panels::world_map::local_terrain_pass;
use crate::theme;
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
    model: AppModel,
    last_theme: theme::MapTheme,
}

impl DashboardApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);
        register_repaint_ctx(&cc.egui_ctx);

        // Initialise GPU pass resources for the globe and local terrain views.
        if let Some(wgpu_state) = cc.wgpu_render_state.as_ref() {
            let globe_res =
                globe_pass::GlobePassResources::new(&wgpu_state.device, wgpu_state.target_format);
            let terrain_res = local_terrain_pass::LocalTerrainPassResources::new(
                &wgpu_state.device,
                wgpu_state.target_format,
            );
            let mut renderer = wgpu_state.renderer.write();
            renderer.callback_resources.insert(globe_res);
            renderer.callback_resources.insert(terrain_res);
        }

        let model = AppModel::seed_demo();
        Self {
            last_theme: model.map_theme,
            model,
        }
    }
}

impl Drop for DashboardApp {
    fn drop(&mut self) {
        factal_stream::shutdown();
        camera_registry::shutdown();
        panels::world_map::srtm_focus_cache::terminate_active_gdal_jobs();
    }
}

impl eframe::App for DashboardApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        factal_stream::tick(&mut self.model);
        camera_registry::tick(&mut self.model);

        if self.model.map_theme != self.last_theme {
            theme::set_theme(ctx, self.model.map_theme);
            self.last_theme = self.model.map_theme;
        }
        if self.model.has_factal_api_key() || self.model.has_camera_source_keys() {
            ctx.request_repaint_after(Duration::from_secs(1));
        }

        if !self.model.cinematic_mode {
            panels::render_header(ctx, &mut self.model);
            panels::render_factal_brief(ctx, &mut self.model);
            panels::render_factal_settings(ctx, &mut self.model);
            panels::render_terrain_library(ctx, &mut self.model);
            panels::render_status_log(ctx, &mut self.model);
            panels::render_event_list(ctx, &mut self.model);
            panels::render_camera_list(ctx, &mut self.model);
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::canvas_background()))
            .show(ctx, |ui| {
                panels::render_world_map(ui, &mut self.model);
            });
    }
}
