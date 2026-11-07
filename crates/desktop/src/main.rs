mod app;
mod buildout;
mod arcgis_source;
mod camera_directory_pipeline;
mod camera_feed_player;
mod camera_feed_viewer;
mod camera_registry;
mod camera_scrape_catalog;
mod camera_source_catalog;
mod city_catalog;
mod deflock_source;
mod event_store;
mod elevation_sources;
mod feature_heights;
mod fire_source;
mod flock_source;
mod factal_stream;
mod flight_tracks;
mod gruve;
mod gpu_config;
mod model;
mod moving_tracks;
mod osm_ingest;
mod panels;
mod planet_ephemeris;
mod radio;
mod settings_store;
mod submarine_cables;
mod stellar_catalog;
mod stellar_time;
mod terrain_assets;
mod threedep;
mod hillshade_source;
mod terrain_precompute;
mod theme;
mod usgs_stream;
mod world_archive;

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
    if let Some(name) = gpu_config::adapter_name() {
        setup.native_adapter_selector = Some(std::sync::Arc::new(move |adapters, surface| {
            gpu_config::select_adapter(adapters, surface, &name)
        }));
    }
    let base = setup.device_descriptor;
    setup.device_descriptor = std::sync::Arc::new(move |adapter| {
        let info = adapter.get_info();
        eprintln!(
            "[1kEE GPU] {} ({:?}, {:?}) driver={} {}",
            info.name, info.backend, info.device_type, info.driver, info.driver_info
        );
        gpu_config::log_settings();
        let mut descriptor = base(adapter);
        let adapter_limits = adapter.limits();
        descriptor.required_limits.max_buffer_size = adapter_limits.max_buffer_size;
        descriptor
    });
    eframe::egui_wgpu::WgpuSetup::CreateNew(setup)
}

fn main() -> eframe::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|arg| arg == "--repair-native-tile") {
        let result = (|| -> Result<(), String> {
            if args.len() != 7 {
                return Err("Usage: --repair-native-tile <srtm_native_v1.sqlite> <SRTM directory> <bucket 0..6> <lat bucket> <lon bucket>".into());
            }
            let integer = |i: usize| args[i].parse::<i32>().map_err(|e| e.to_string());
            let tile = panels::world_map::srtm_focus_cache::TileKey {
                zoom_bucket: integer(4)?, lat_bucket: integer(5)?, lon_bucket: integer(6)?,
            };
            let path = panels::world_map::srtm_focus_cache::recovery::repair_native_tile(
                std::path::Path::new(&args[2]), std::path::Path::new(&args[3]), tile)?;
            println!("Verified terrain tile {}/{}/{}: {}", tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket, path.display());
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("[1kEE] {error}");
            std::process::exit(1);
        }
        return Ok(());
    }
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
