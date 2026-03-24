mod args;
mod geojson;
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
        args::Command::RoadsBbox(command) => roads::build_bbox_cache(command),
    }
}
