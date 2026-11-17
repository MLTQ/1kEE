//! Numeric DEM and a clipped display mesh. No synthetic terrain fills gaps.
use super::{Batch, Vertex, serial};
use lidar_pipeline::region::Region;
use std::{path::Path, sync::Arc};
use tiff::{
    decoder::{Decoder, DecodingResult},
    tags::Tag,
};

pub(crate) struct Terrain {
    pub batch: Arc<Batch>,
    pub indices: Vec<u32>,
    pub label: String,
    pub navd88: bool,
    pub width: usize,
    pub height: usize,
    pub heights: Vec<f32>,
    pub origin: [f64; 2],
    pub scale: [f64; 2],
}
impl Terrain {
    pub fn sample(&self, lon: f64, lat: f64) -> Option<f32> {
        let x = (lon - self.origin[0]) / self.scale[0] - 0.5;
        let y = (lat - self.origin[1]) / self.scale[1] - 0.5;
        if x < 0. || y < 0. || x >= (self.width - 1) as f64 || y >= (self.height - 1) as f64 {
            return None;
        }
        let (ix, iy) = (x.floor() as usize, y.floor() as usize);
        let (fx, fy) = ((x - ix as f64) as f32, (y - iy as f64) as f32);
        let v = [
            self.heights[iy * self.width + ix],
            self.heights[iy * self.width + ix + 1],
            self.heights[(iy + 1) * self.width + ix],
            self.heights[(iy + 1) * self.width + ix + 1],
        ];
        if v.iter().any(|v| !valid(*v)) {
            return None;
        }
        Some((v[0] * (1. - fx) + v[1] * fx) * (1. - fy) + (v[2] * (1. - fx) + v[3] * fx) * fy)
    }
    pub fn center_height(&self) -> f32 {
        let v = self.heights[self.height / 2 * self.width + self.width / 2];
        if valid(v) {
            v
        } else {
            self.heights
                .iter()
                .copied()
                .find(|v| valid(*v))
                .unwrap_or(0.)
        }
    }
}
fn valid(h: f32) -> bool {
    h.is_finite() && (-12_000. ..=10_000.).contains(&h)
}

static ACTIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
struct Permit;
impl Drop for Permit {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, std::sync::atomic::Ordering::Release);
    }
}
pub(super) fn load(region: Region, active: &Path) -> Result<Terrain, String> {
    use std::sync::atomic::Ordering;
    loop {
        if !active.is_file() {
            return Err("Target closed".into());
        }
        let running = ACTIVE.load(Ordering::Acquire);
        if running < 2
            && ACTIVE
                .compare_exchange(running, running + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _permit = Permit;
    let [w, s, e, n] = region.bounds();
    if w < -180. || e > 180. {
        return Err("DEM window crosses the date line; point coordinates remain available".into());
    }
    let temp = tempfile::tempdir().map_err(|e| e.to_string())?;
    let raster = temp.path().join("terrain.tif");
    let center = crate::model::GeoPoint {
        lat: region.lat as f32,
        lon: region.lon as f32,
    };
    let mut provider = None;
    let fetched = if crate::elevation_sources::possible_at(center) {
        match crate::elevation_sources::fetch_target(region, 2048, &raster, |_, _| {}) {
            Ok(p) => {
                provider = Some(p);
                true
            }
            Err(crate::elevation_sources::Error::NoCoverage) => false,
            Err(crate::elevation_sources::Error::Failed(error)) => return Err(error),
        }
    } else {
        false
    };
    if !active.is_file() {
        return Err("Target closed".into());
    }
    if !fetched
        && !crate::threedep::fetch_tile_raster_with_progress(s, w, n, e, 2048, &raster, |_, _| {})
    {
        return Err("No fine DEM returned here; source points remain independent".into());
    }
    if !active.is_file() {
        return Err("Target closed".into());
    }
    let name = provider.map_or("USGS 3DEP bare-earth DEM", |p| p.label());
    read(&raster, region, name)
}

pub(super) fn read(path: &Path, region: Region, label: &str) -> Result<Terrain, String> {
    let mut decoder = Decoder::new(std::io::BufReader::new(
        std::fs::File::open(path).map_err(|e| e.to_string())?,
    ))
    .map_err(|e| e.to_string())?;
    let (width, height) = decoder.dimensions().map_err(|e| e.to_string())?;
    let navd88 = decoder
        .get_tag_u16_vec(Tag::GeoKeyDirectoryTag)
        .ok()
        .is_some_and(|keys| {
            keys.get(4..).is_some_and(|entries| {
                entries
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|key| key[0] == 4096 && key[1] == 0 && key[2] == 1 && key[3] == 5703)
            })
        });
    if width < 2 || height < 2 || width > 2400 || height > 2400 {
        return Err("Unexpected Target DEM size".into());
    }
    let tie = decoder
        .get_tag_f64_vec(Tag::ModelTiepointTag)
        .map_err(|e| e.to_string())?;
    let scale = decoder
        .get_tag_f64_vec(Tag::ModelPixelScaleTag)
        .map_err(|e| e.to_string())?;
    if tie.len() < 6 || scale.len() < 2 || scale[0] <= 0. || scale[1] <= 0. {
        return Err("Unsupported DEM geographic transform".into());
    }
    // Both source adapters explicitly export EPSG:4326; use the actual TIFF
    // transform, including pixel centers, rather than assuming its footprint.
    let origin = [tie[3] - tie[0] * scale[0], tie[4] + tie[1] * scale[1]];
    let scale = [scale[0], -scale[1]];
    let DecodingResult::F32(heights) = decoder.read_image().map_err(|e| e.to_string())? else {
        return Err("Target needs numeric Float32 elevations".into());
    };
    if heights.len() != width as usize * height as usize || !heights.iter().any(|v| valid(*v)) {
        return Err("No valid DEM elevations in Target".into());
    }
    let (width, height) = (width as usize, height as usize);
    let stride = 4;
    let xs = (0..width)
        .step_by(stride)
        .chain(std::iter::once(width - 1))
        .collect::<Vec<_>>();
    let ys = (0..height)
        .step_by(stride)
        .chain(std::iter::once(height - 1))
        .collect::<Vec<_>>();
    let mut points = Vec::with_capacity(xs.len() * ys.len());
    for &y in &ys {
        for &x in &xs {
            let ll = [
                origin[0] + (x as f64 + 0.5) * scale[0],
                origin[1] + (y as f64 + 0.5) * scale[1],
            ];
            let [east, north] = region.offset(ll[0], ll[1]);
            let h = heights[y * width + x];
            points.push(Vertex {
                position: [
                    east as f32,
                    north as f32,
                    if valid(h) { h } else { f32::NAN },
                ],
                color: [85, 112, 119, 255],
                class: 0,
            });
        }
    }
    let mut indices = Vec::new();
    let row = xs.len();
    for y in 0..ys.len() - 1 {
        for x in 0..xs.len() - 1 {
            let a = y * row + x;
            let b = a + 1;
            let c = a + row;
            let d = c + 1;
            for triangle in [[a, b, c], [b, d, c]] {
                if triangle.iter().all(|i| points[*i].position[2].is_finite()) {
                    indices.extend(triangle.map(|i| i as u32));
                }
            }
        }
    }
    let spacing = scale[1].abs() * 111_320.;
    if indices.is_empty() {return Err("No DEM measurements intersect the selected circle".into());}
    Ok(Terrain {
        batch: Arc::new(Batch {
            source: None,
            id: serial(),
            points,
        }),
        indices,
        label: format!(
            "{label} · sampled grid ~{spacing:.1} m · display mesh ~{:.1} m",
            spacing * stride as f64
        ),
        navd88,
        width,
        height,
        heights,
        origin,
        scale,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dem_sampling_preserves_valid_zero_negative_and_missing_heights() {
        let terrain = Terrain {
            batch: Arc::new(Batch {
                source: None,
                id: 0,
                points: vec![],
            }),
            indices: vec![],
            label: String::new(),
            navd88: false,
            width: 3,
            height: 3,
            heights: vec![0., -2., -4., 2., 0., -2., 4., 2., -999999.],
            origin: [0., 3.],
            scale: [1., -1.],
        };
        assert_eq!(terrain.sample(1., 2.), Some(0.));
        assert_eq!(terrain.sample(1.5, 2.5), Some(-2.));
        assert_eq!(terrain.sample(2., 1.), None);
        assert_eq!(terrain.sample(-1., 0.), None);
    }
}
