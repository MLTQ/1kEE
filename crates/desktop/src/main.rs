mod app;
mod arcgis_source;
mod camera_directory_pipeline;
mod camera_feed_viewer;
mod camera_registry;
mod camera_scrape_catalog;
mod camera_source_catalog;
mod city_catalog;
mod deflock_source;
mod event_store;
mod factal_stream;
mod flight_tracks;
mod gruve;
mod model;
mod moving_tracks;
mod osm_ingest;
mod panels;
mod planet_ephemeris;
mod settings_store;
mod stellar_catalog;
mod stellar_time;
mod terrain_assets;
mod threedep;
mod terrain_precompute;
mod theme;
mod usgs_stream;

use app::DashboardApp;

/// Ask the adapter for the largest buffer it will give us.
///
/// wgpu's default `max_buffer_size` is 256 MiB, which a dense contour layer can
/// exceed in one allocation. The passes split across buffers regardless, so
/// this is not what keeps them safe — it just means far fewer, larger buffers
/// and correspondingly fewer draw calls on hardware that allows it.
fn contour_buffer_wgpu_setup() -> eframe::egui_wgpu::WgpuSetup {
    let mut setup = match eframe::egui_wgpu::WgpuConfiguration::default().wgpu_setup {
        eframe::egui_wgpu::WgpuSetup::CreateNew(create_new) => create_new,
        // An externally supplied device is not ours to reconfigure.
        existing => return existing,
    };
    let base = setup.device_descriptor;
    setup.device_descriptor = std::sync::Arc::new(move |adapter| {
        let mut descriptor = base(adapter);
        let adapter_limits = adapter.limits();
        descriptor.required_limits.max_buffer_size = adapter_limits.max_buffer_size;
        descriptor
    });
    eframe::egui_wgpu::WgpuSetup::CreateNew(setup)
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1520.0, 920.0])
            .with_min_inner_size([1100.0, 720.0])
            .with_title("1kEE | One Thousand Electric Eye"),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            wgpu_setup: contour_buffer_wgpu_setup(),
            ..Default::default()
        },
        ..Default::default()
    };

    eframe::run_native(
        "1kEE",
        options,
        Box::new(|cc| Ok(Box::new(DashboardApp::new(cc)))),
    )
}
