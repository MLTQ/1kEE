use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum Command {
    Gui,
    RoadsBbox(BboxCommand),
    ContoursBbox(ContoursBboxCommand),
}

#[derive(Debug, Clone)]
pub struct ContoursBboxCommand {
    pub srtm_root: PathBuf,
    pub cache_db_path: PathBuf,  // path to srtm_focus_cache.sqlite
    pub tmp_dir: Option<PathBuf>, // default: cache_db parent / srtm_focus_tmp
    pub min_lat: f32,
    pub max_lat: f32,
    pub min_lon: f32,
    pub max_lon: f32,
    pub zoom_buckets: Vec<i32>,  // default: all 0–6
    pub gdal_bin_dir: PathBuf,   // "" = use $PATH
}

#[derive(Debug, Clone)]
pub struct BboxCommand {
    pub planet_path: PathBuf,
    pub cache_dir: PathBuf,  // parent dir; subdirs created per feature
    pub srtm_root: Option<PathBuf>, // when set, elevations are baked into .1kc files
    pub min_lat: f32,
    pub max_lat: f32,
    pub min_lon: f32,
    pub max_lon: f32,
    pub margin_degrees: f32,
    pub build_roads: bool,
    pub build_waterways: bool,
    pub build_buildings: bool,
    pub build_trees: bool,
    pub build_admin: bool,
}

pub type RoadsBboxCommand = BboxCommand;

pub fn parse<I>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let Some(command) = args.next() else {
        return Ok(Command::Gui);
    };

    match command.as_str() {
        "gui" => Ok(Command::Gui),
        "roads-bbox" => parse_roads_bbox(args).map(Command::RoadsBbox),
        "contours-bbox" => parse_contours_bbox(args).map(Command::ContoursBbox),
        "--help" | "-h" | "help" => Err(usage()),
        other => Err(format!("Unknown command '{other}'.\n\n{}", usage())),
    }
}

fn parse_roads_bbox<I>(args: I) -> Result<BboxCommand, String>
where
    I: IntoIterator<Item = String>,
{
    let mut planet_path = None;
    let mut cache_dir = None;
    let mut srtm_root = None;
    let mut min_lat = None;
    let mut max_lat = None;
    let mut min_lon = None;
    let mut max_lon = None;
    let mut margin_degrees = 0.08_f32;
    let mut build_roads = true;
    let mut build_waterways = false;
    let mut build_buildings = false;
    let mut build_trees = false;
    let mut build_admin = false;

    let mut iter = args.into_iter();
    while let Some(flag) = iter.next() {
        let value = iter
            .next()
            .ok_or_else(|| format!("Missing value for '{flag}'.\n\n{}", usage()))?;
        match flag.as_str() {
            "--planet" => planet_path = Some(PathBuf::from(value)),
            "--cache-dir" => cache_dir = Some(PathBuf::from(value)),
            "--srtm-root" => srtm_root = Some(PathBuf::from(value)),
            "--min-lat" => min_lat = Some(parse_f32("--min-lat", &value)?),
            "--max-lat" => max_lat = Some(parse_f32("--max-lat", &value)?),
            "--min-lon" => min_lon = Some(parse_f32("--min-lon", &value)?),
            "--max-lon" => max_lon = Some(parse_f32("--max-lon", &value)?),
            "--margin-deg" => margin_degrees = parse_f32("--margin-deg", &value)?,
            "--features" => {
                build_roads = false;
                for feature in value.split(',') {
                    match feature.trim() {
                        "roads" => build_roads = true,
                        "waterways" => build_waterways = true,
                        "buildings" => build_buildings = true,
                        "trees" => build_trees = true,
                        "admin" => build_admin = true,
                        other => return Err(format!("Unknown feature '{other}'.\n\n{}", usage())),
                    }
                }
            }
            other => return Err(format!("Unknown flag '{other}'.\n\n{}", usage())),
        }
    }

    let command = BboxCommand {
        planet_path: planet_path.ok_or_else(|| format!("Missing --planet.\n\n{}", usage()))?,
        cache_dir: cache_dir.ok_or_else(|| format!("Missing --cache-dir.\n\n{}", usage()))?,
        srtm_root,
        min_lat: min_lat.ok_or_else(|| format!("Missing --min-lat.\n\n{}", usage()))?,
        max_lat: max_lat.ok_or_else(|| format!("Missing --max-lat.\n\n{}", usage()))?,
        min_lon: min_lon.ok_or_else(|| format!("Missing --min-lon.\n\n{}", usage()))?,
        max_lon: max_lon.ok_or_else(|| format!("Missing --max-lon.\n\n{}", usage()))?,
        margin_degrees,
        build_roads,
        build_waterways,
        build_buildings,
        build_trees,
        build_admin,
    };

    if command.min_lat >= command.max_lat || command.min_lon >= command.max_lon {
        return Err("Invalid bbox: min values must be less than max values.".to_owned());
    }

    Ok(command)
}

fn parse_contours_bbox<I>(args: I) -> Result<ContoursBboxCommand, String>
where
    I: IntoIterator<Item = String>,
{
    let mut srtm_root = None;
    let mut cache_db_path = None;
    let mut tmp_dir = None;
    let mut min_lat = None;
    let mut max_lat = None;
    let mut min_lon = None;
    let mut max_lon = None;
    let mut zoom_buckets: Option<Vec<i32>> = None;
    let mut gdal_bin_dir = PathBuf::from("");

    let mut iter = args.into_iter();
    while let Some(flag) = iter.next() {
        let value = iter
            .next()
            .ok_or_else(|| format!("Missing value for '{flag}'.\n\n{}", usage()))?;
        match flag.as_str() {
            "--srtm-root"    => srtm_root = Some(PathBuf::from(value)),
            "--cache-db"     => cache_db_path = Some(PathBuf::from(value)),
            "--tmp-dir"      => tmp_dir = Some(PathBuf::from(value)),
            "--min-lat"      => min_lat = Some(parse_f32("--min-lat", &value)?),
            "--max-lat"      => max_lat = Some(parse_f32("--max-lat", &value)?),
            "--min-lon"      => min_lon = Some(parse_f32("--min-lon", &value)?),
            "--max-lon"      => max_lon = Some(parse_f32("--max-lon", &value)?),
            "--gdal-bin"     => gdal_bin_dir = PathBuf::from(value),
            "--zoom-buckets" => {
                let buckets: Result<Vec<i32>, _> = value
                    .split(',')
                    .map(|s| s.trim().parse::<i32>())
                    .collect();
                zoom_buckets = Some(
                    buckets.map_err(|_| format!("Invalid --zoom-buckets '{value}'"))?,
                );
            }
            other => return Err(format!("Unknown flag '{other}'.\n\n{}", usage())),
        }
    }

    let cache_db = cache_db_path
        .ok_or_else(|| format!("Missing --cache-db.\n\n{}", usage()))?;
    let derived_tmp = tmp_dir.unwrap_or_else(|| {
        cache_db
            .parent()
            .unwrap_or(Path::new("."))
            .join("srtm_focus_tmp")
    });

    Ok(ContoursBboxCommand {
        srtm_root: srtm_root.ok_or_else(|| format!("Missing --srtm-root.\n\n{}", usage()))?,
        cache_db_path: cache_db,
        tmp_dir: Some(derived_tmp),
        min_lat: min_lat.ok_or_else(|| format!("Missing --min-lat.\n\n{}", usage()))?,
        max_lat: max_lat.ok_or_else(|| format!("Missing --max-lat.\n\n{}", usage()))?,
        min_lon: min_lon.ok_or_else(|| format!("Missing --min-lon.\n\n{}", usage()))?,
        max_lon: max_lon.ok_or_else(|| format!("Missing --max-lon.\n\n{}", usage()))?,
        zoom_buckets: zoom_buckets.unwrap_or_else(|| (0..=6).collect()),
        gdal_bin_dir,
    })
}

fn parse_f32(flag: &str, value: &str) -> Result<f32, String> {
    value
        .parse::<f32>()
        .map_err(|_| format!("Invalid numeric value for {flag}: '{value}'"))
}

fn usage() -> String {
    "Usage:
  one-thousand-electric-eye-cache-builder
  one-thousand-electric-eye-cache-builder gui
  one-thousand-electric-eye-cache-builder roads-bbox \\
      --planet <planet.osm.pbf> --cache-dir <Derived/osm> \\
      --min-lat <f32> --max-lat <f32> --min-lon <f32> --max-lon <f32> \\
      [--margin-deg <f32>] [--features roads,waterways,buildings,trees,admin] \\
      [--srtm-root <dir>]
  one-thousand-electric-eye-cache-builder contours-bbox \\
      --srtm-root <dir> --cache-db <Derived/terrain/srtm_focus_cache.sqlite> \\
      --min-lat <f32> --max-lat <f32> --min-lon <f32> --max-lon <f32> \\
      [--zoom-buckets 0,1,2,3,4,5,6] [--gdal-bin <dir>] [--tmp-dir <dir>]"
        .to_owned()
}
