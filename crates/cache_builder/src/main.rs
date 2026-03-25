mod admin;
mod app;
mod args;
mod geojson;
mod job;
mod node_store;
mod roads;
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
