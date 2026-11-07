//! Shared cache construction and the companion application entry point.
mod admin;
mod app;
mod archive_pack;
mod archive_space;
mod args;
pub mod boundaries;
mod contours;
mod flat_node_store;
mod geojson;
mod job;
mod lunar;
mod marching_squares;
pub mod gpu_terrain;
mod mars;
mod node_store;
mod pbf_nodes;
mod pipelines;
mod planet_all;
mod planet_lookup;
mod platforms;
mod roads;
mod srtm;
mod util;

pub fn run() -> Result<(), String> {
    match args::parse(std::env::args().skip(1))? {
        args::Command::Gui => launch_gui(),
        args::Command::AdminLods(dir) => {
            let mut found = false;
            for level in 2..=11 {
                let path = dir.join(cell_format::admin_filename(level));
                if !path.try_exists().map_err(|e| e.to_string())? {
                    continue;
                }
                found = true;
                let archive = tile_archive::admin_lod::Archive::open(&path, level)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                println!(
                    "Level {level}, segment counts (world to original): {:?}",
                    archive.segment_counts().map_err(|e| e.to_string())?
                );
            }
            if !found {
                return Err(format!(
                    "No administrative boundary levels in {}",
                    dir.display()
                ));
            }
            Ok(())
        }
        args::Command::PackArchive(command) => archive_pack::run(command),
        args::Command::Pipelines(command) => pipelines::run(command),
        args::Command::Platforms(command) => platforms::run(command),
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
                args::ContourEngine::Gpu => gpu_terrain::build_bbox(&command, reporter),
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
