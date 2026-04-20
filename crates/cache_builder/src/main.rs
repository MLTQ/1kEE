mod admin;
mod app;
mod args;
mod contours;
mod flat_node_store;
mod geojson;
mod job;
mod lunar;
mod marching_squares;
mod mars;
mod node_store;
mod planet_all;
mod roads;
mod srtm;
mod util;

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    match args::parse(std::env::args().skip(1))? {
        args::Command::Gui => launch_gui(),
        args::Command::RoadsBbox(command) => roads::build_bbox_cache(command),
        args::Command::PlanetAll(command) => planet_all::build_planet_cache(command),
        args::Command::ContoursBbox(command) => {
            let bounds = contours::GeoBounds {
                min_lat: command.min_lat,
                max_lat: command.max_lat,
                min_lon: command.min_lon,
                max_lon: command.max_lon,
            };
            let reporter = &mut |p: contours::ContourBuildProgress| {
                println!("[{:.0}%] {}: {}", p.fraction * 100.0, p.stage, p.message);
            };
            match command.engine {
                args::ContourEngine::Native => contours::build_contour_tiles_native(
                    &command.srtm_root,
                    &command.cache_db_path,
                    bounds,
                    &command.zoom_buckets,
                    reporter,
                ),
                args::ContourEngine::Gdal => {
                    let tmp_dir = command.tmp_dir.clone().unwrap_or_else(|| {
                        command
                            .cache_db_path
                            .parent()
                            .unwrap_or(std::path::Path::new("."))
                            .join("srtm_focus_tmp")
                    });
                    contours::build_contour_tiles(
                        &command.srtm_root,
                        &command.cache_db_path,
                        &tmp_dir,
                        bounds,
                        &command.zoom_buckets,
                        &command.gdal_bin_dir,
                        reporter,
                    )
                }
            }
            .map(|summary| println!("{summary}"))
            .map_err(|e| e)
        }
    }
}

fn launch_gui() -> Result<(), String> {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "1kEE Cache Builder",
        options,
        Box::new(|_cc| Ok(Box::new(app::BuilderApp::new()))),
    )
    .map_err(|error| error.to_string())
}
