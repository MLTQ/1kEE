//! Bounded on-demand bare-earth elevation outside the USGS service area.
mod http;
mod japan;
mod new_zealand;
mod raster;
mod services;
mod swiss;

use crate::model::GeoPoint;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
pub use tile_archive::contour_grid::Bounds;

pub const NODATA: f32 = -999_999.0;
pub const PROVIDERS: [Provider; 6] = [
    Provider::England,
    Provider::Netherlands,
    Provider::Switzerland,
    Provider::France,
    Provider::NewZealand,
    Provider::Japan,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Provider {
    England,
    Netherlands,
    Switzerland,
    France,
    NewZealand,
    Japan,
}

impl Provider {
    pub fn label(self) -> &'static str {
        match self {
            Self::England => "England — © Environment Agency 2022, 1 m (OGL)",
            Self::Netherlands => "Netherlands — AHN / PDOK, 0.5 m",
            Self::Switzerland => "Switzerland — © swisstopo, 0.5–2 m",
            Self::France => "France — © IGN LiDAR HD, 0.5 m",
            Self::NewZealand => "New Zealand — Toitū Te Whenua LINZ, 1 m (CC BY 4.0)",
            Self::Japan => "Japan — Geospatial Information Authority of Japan, 1–10 m",
        }
    }
    pub fn url(self) -> &'static str {
        match self {
            Self::England => {
                "https://www.data.gov.uk/dataset/01b3ee39-da3f-47b6-83da-dc98e73a461f/lidar-composite-digital-terrain-model-dtm-1m"
            }
            Self::Netherlands => "https://www.ahn.nl/dataroom",
            Self::Switzerland => "https://www.swisstopo.admin.ch/en/height-model-swissalti3d",
            Self::France => "https://www.data.gouv.fr/datasets/mnt-lidar-hd",
            Self::NewZealand => "https://github.com/linz/elevation",
            Self::Japan => "https://maps.gsi.go.jp/development/ichiran.html#dem",
        }
    }
    fn contains(self, p: GeoPoint) -> bool {
        let (west, south, east, north) = match self {
            Self::England => (-6.5, 49.8, 2.1, 55.9),
            Self::Netherlands => (3.2, 50.7, 7.3, 53.7),
            Self::Switzerland => (5.9, 45.7, 10.6, 47.9),
            Self::France => (-5.2, 41.3, 9.7, 51.2),
            Self::NewZealand => (165.0, -48.0, 179.9, -33.0),
            Self::Japan => (122.0, 24.0, 146.0, 46.0),
        };
        (west..=east).contains(&p.lon) && (south..=north).contains(&p.lat)
    }
}

/// A geographic screen, not a promise that every pixel has survey coverage.
pub fn possible_at(point: GeoPoint) -> bool {
    PROVIDERS.iter().any(|provider| provider.contains(point))
}

#[derive(Debug)]
pub enum Error {
    NoCoverage,
    Failed(String),
}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Failed(error.to_string())
    }
}
type Result<T> = std::result::Result<T, Error>;

thread_local! { static DEADLINE: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) }; }
struct Deadline(Option<Instant>);
impl Deadline {
    fn start() -> Self {
        Self(DEADLINE.replace(Some(Instant::now() + Duration::from_secs(180))))
    }
}
impl Drop for Deadline {
    fn drop(&mut self) {
        DEADLINE.set(self.0);
    }
}
fn remaining() -> Result<Duration> {
    if crate::panels::world_map::srtm_focus_cache::gdal::shutdown_requested()
        .load(Ordering::Relaxed)
    {
        return Err(Error::Failed("elevation request cancelled".into()));
    }
    DEADLINE
        .get()
        .map_or(Some(Duration::from_secs(180)), |end| {
            end.checked_duration_since(Instant::now())
        })
        .filter(|left| !left.is_zero())
        .ok_or_else(|| Error::Failed("elevation request timed out".into()))
}

#[derive(Clone, Copy)]
struct Request {
    bounds: Bounds,
    width: u32,
    height: u32,
}
impl Request {
    fn center(self) -> GeoPoint {
        GeoPoint {
            lat: ((self.bounds.min_lat + self.bounds.max_lat) * 0.5) as f32,
            lon: ((self.bounds.min_lon + self.bounds.max_lon) * 0.5) as f32,
        }
    }
    fn valid(self) -> bool {
        let b = self.bounds;
        [b.min_lon, b.min_lat, b.max_lon, b.max_lat]
            .iter()
            .all(|v| v.is_finite())
            && b.min_lon < b.max_lon
            && b.min_lat < b.max_lat
            && b.min_lon >= -180.0
            && b.max_lon <= 180.0
            && b.min_lat >= -85.0
            && b.max_lat <= 85.0
            && (16..=2400).contains(&self.width)
            && (16..=2400).contains(&self.height)
            && b.max_lon - b.min_lon <= 0.25
            && b.max_lat - b.min_lat <= 0.25
    }
}

/// Produces one north-up Float32 EPSG:4326 TIFF with our common nodata value.
/// Only success replaces the caller's destination. Intermediates are disposable.
pub fn fetch(
    bounds: Bounds,
    width: u32,
    height: u32,
    destination: &Path,
    mut on_bytes: impl FnMut(u64, Option<u64>),
) -> Result<Provider> {
    let _deadline = Deadline::start();
    let request = Request {
        bounds,
        width,
        height,
    };
    if !request.valid() {
        return Err(Error::Failed("invalid elevation window".into()));
    }
    let scratch = Scratch::new(destination)?;
    let mut failure = None;
    for provider in PROVIDERS
        .into_iter()
        .filter(|p| p.contains(request.center()))
    {
        let _permit = Permit::acquire(provider)?;
        let result = match provider {
            Provider::Japan => japan::fetch(request, &scratch, &mut on_bytes),
            Provider::Switzerland => swiss::fetch(request, &scratch),
            Provider::NewZealand => new_zealand::fetch(request, &scratch),
            _ => services::fetch(provider, request, &scratch, &mut on_bytes),
        };
        match result {
            Ok(path) => {
                std::fs::rename(path, destination)?;
                return Ok(provider);
            }
            Err(Error::NoCoverage) => {}
            Err(Error::Failed(message)) => {
                failure = Some(format!("{}: {message}", provider.label()))
            }
        }
    }
    Err(failure.map(Error::Failed).unwrap_or(Error::NoCoverage))
}

struct Scratch(PathBuf);
impl Scratch {
    fn new(destination: &Path) -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let parent = destination
            .parent()
            .ok_or_else(|| Error::Failed("missing output directory".into()))?;
        std::fs::create_dir_all(parent)?;
        let path = parent.join(format!(
            "elevation-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// Contours and point samples share two active requests per public provider.
static ACTIVE: [AtomicUsize; 6] = [const { AtomicUsize::new(0) }; 6];
struct Permit(usize);
impl Permit {
    fn acquire(provider: Provider) -> Result<Self> {
        let index = provider as usize;
        loop {
            remaining()?;
            if ACTIVE[index]
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                    (n < 2).then_some(n + 1)
                })
                .is_ok()
            {
                return Ok(Self(index));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        ACTIVE[self.0].fetch_sub(1, Ordering::Release);
    }
}

#[cfg(test)]
mod tests;
