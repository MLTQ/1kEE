use std::path::PathBuf;

#[derive(Debug)]
pub enum Command {
    RoadsBbox(RoadsBboxCommand),
}

#[derive(Debug, Clone)]
pub struct RoadsBboxCommand {
    pub planet_path: PathBuf,
    pub cache_dir: PathBuf,
    pub min_lat: f32,
    pub max_lat: f32,
    pub min_lon: f32,
    pub max_lon: f32,
    pub margin_degrees: f32,
}

pub fn parse<I>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let Some(command) = args.next() else {
        return Err(usage());
    };

    match command.as_str() {
        "roads-bbox" => parse_roads_bbox(args).map(Command::RoadsBbox),
        "--help" | "-h" | "help" => Err(usage()),
        other => Err(format!("Unknown command '{other}'.\n\n{}", usage())),
    }
}

fn parse_roads_bbox<I>(args: I) -> Result<RoadsBboxCommand, String>
where
    I: IntoIterator<Item = String>,
{
    let mut planet_path = None;
    let mut cache_dir = None;
    let mut min_lat = None;
    let mut max_lat = None;
    let mut min_lon = None;
    let mut max_lon = None;
    let mut margin_degrees = 0.08_f32;

    let mut iter = args.into_iter();
    while let Some(flag) = iter.next() {
        let value = iter
            .next()
            .ok_or_else(|| format!("Missing value for '{flag}'.\n\n{}", usage()))?;
        match flag.as_str() {
            "--planet" => planet_path = Some(PathBuf::from(value)),
            "--cache-dir" => cache_dir = Some(PathBuf::from(value)),
            "--min-lat" => min_lat = Some(parse_f32("--min-lat", &value)?),
            "--max-lat" => max_lat = Some(parse_f32("--max-lat", &value)?),
            "--min-lon" => min_lon = Some(parse_f32("--min-lon", &value)?),
            "--max-lon" => max_lon = Some(parse_f32("--max-lon", &value)?),
            "--margin-deg" => margin_degrees = parse_f32("--margin-deg", &value)?,
            other => return Err(format!("Unknown flag '{other}'.\n\n{}", usage())),
        }
    }

    let command = RoadsBboxCommand {
        planet_path: planet_path.ok_or_else(|| format!("Missing --planet.\n\n{}", usage()))?,
        cache_dir: cache_dir.ok_or_else(|| format!("Missing --cache-dir.\n\n{}", usage()))?,
        min_lat: min_lat.ok_or_else(|| format!("Missing --min-lat.\n\n{}", usage()))?,
        max_lat: max_lat.ok_or_else(|| format!("Missing --max-lat.\n\n{}", usage()))?,
        min_lon: min_lon.ok_or_else(|| format!("Missing --min-lon.\n\n{}", usage()))?,
        max_lon: max_lon.ok_or_else(|| format!("Missing --max-lon.\n\n{}", usage()))?,
        margin_degrees,
    };

    if command.min_lat >= command.max_lat || command.min_lon >= command.max_lon {
        return Err("Invalid bbox: min values must be less than max values.".to_owned());
    }

    Ok(command)
}

fn parse_f32(flag: &str, value: &str) -> Result<f32, String> {
    value
        .parse::<f32>()
        .map_err(|_| format!("Invalid numeric value for {flag}: '{value}'"))
}

fn usage() -> String {
    "Usage:\n  one-thousand-electric-eye-cache-builder roads-bbox --planet <planet.osm.pbf> --cache-dir <Derived/osm/road_cells> --min-lat <f32> --max-lat <f32> --min-lon <f32> --max-lon <f32> [--margin-deg <f32>]".to_owned()
}
