//! GSI signed RGB elevation tiles, not ordinary colour map imagery.
use super::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

const LAYERS: [(&str, u8); 5] = [
    ("dem1a_png", 17),
    ("dem5a_png", 15),
    ("dem5b_png", 15),
    ("dem5c_png", 15),
    ("dem_png", 14),
];
type Key = (&'static str, u8, i32, i32);
type Pixels = Option<Arc<Vec<f32>>>;
type Cache = Mutex<HashMap<Key, (Instant, Pixels)>>;

fn decode(rgb: &[u8]) -> f32 {
    let value = (u32::from(rgb[0]) << 16) | (u32::from(rgb[1]) << 8) | u32::from(rgb[2]);
    if value == 1 << 23 {
        NODATA
    } else if value > 1 << 23 {
        (value as i32 - (1 << 24)) as f32 * 0.01
    } else {
        value as f32 * 0.01
    }
}

fn pixel(lon: f64, lat: f64, zoom: u8) -> (f64, f64) {
    let size = 256.0 * f64::from(1u32 << zoom);
    let latitude = lat.to_radians();
    (
        (lon + 180.0) / 360.0 * size - 0.5,
        (1.0 - (latitude.tan() + 1.0 / latitude.cos()).ln() / std::f64::consts::PI) * 0.5 * size
            - 0.5,
    )
}

fn tiles(bounds: Bounds, zoom: u8) -> (i32, i32, i32, i32) {
    let (x0, y0) = pixel(bounds.min_lon, bounds.max_lat, zoom);
    let (x1, y1) = pixel(bounds.max_lon, bounds.min_lat, zoom);
    (
        (x0.floor() / 256.0).floor() as i32,
        (y0.floor() / 256.0).floor() as i32,
        ((x1.ceil() + 1.0) / 256.0).floor() as i32,
        ((y1.ceil() + 1.0) / 256.0).floor() as i32,
    )
}

fn zoom_for(request: Request, maximum: u8) -> u8 {
    let dx = (request.bounds.max_lon - request.bounds.min_lon) / f64::from(request.width);
    let (_, top) = pixel(0.0, request.bounds.max_lat, 1);
    let (_, bottom) = pixel(0.0, request.bounds.min_lat, 1);
    let by_x = (360.0 / (dx * 256.0)).log2().ceil();
    let by_y = (f64::from(request.height) / (bottom - top)).log2().ceil() + 1.0;
    let mut zoom = by_x.max(by_y).clamp(1.0, f64::from(maximum)) as u8;
    while zoom > 1 {
        let (x0, y0, x1, y1) = tiles(request.bounds, zoom);
        if (x1 - x0 + 1) * (y1 - y0 + 1) <= 64 {
            break;
        }
        zoom -= 1;
    }
    zoom
}

fn tile(
    key: Key,
    received: &mut u64,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<Pixels> {
    remaining()?;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((time, pixels)) = cache.lock().unwrap().get(&key)
        && time.elapsed() < Duration::from_secs(600)
    {
        return Ok(pixels.clone());
    }
    let url = format!(
        "https://cyberjapandata.gsi.go.jp/xyz/{}/{}/{}/{}.png",
        key.0, key.1, key.2, key.3
    );
    let pixels = match http::bytes(&url, 2 * 1024 * 1024) {
        Err(Error::NoCoverage) => None,
        Err(error) => return Err(error),
        Ok(bytes) => {
            *received += bytes.len() as u64;
            progress(*received, None);
            // Bound decompression before decoding untrusted image contents.
            if bytes.get(..8) != Some(b"\x89PNG\r\n\x1a\n")
                || bytes.get(16..20) != Some(&256u32.to_be_bytes())
                || bytes.get(20..24) != Some(&256u32.to_be_bytes())
            {
                return Err(Error::Failed(
                    "invalid GSI elevation tile dimensions".into(),
                ));
            }
            let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
                .map_err(|e| Error::Failed(e.to_string()))?
                .to_rgb8();
            Some(Arc::new(
                image.as_raw().chunks_exact(3).map(decode).collect(),
            ))
        }
    };
    let mut cache = cache.lock().unwrap();
    if cache.len() >= 128 {
        let oldest = cache
            .iter()
            .min_by_key(|(_, (time, _))| *time)
            .map(|(key, _)| *key);
        if let Some(oldest) = oldest {
            cache.remove(&oldest);
        }
    }
    cache.insert(key, (Instant::now(), pixels.clone()));
    Ok(pixels)
}

fn sample(pixels: &HashMap<(i32, i32), Pixels>, x: f64, y: f64) -> Option<f32> {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let at = |x: i32, y: i32| {
        let tile = pixels
            .get(&(x.div_euclid(256), y.div_euclid(256)))?
            .as_ref()?;
        let height = tile[(y.rem_euclid(256) * 256 + x.rem_euclid(256)) as usize];
        (height != NODATA && height.is_finite()).then_some(height)
    };
    let tx = (x - f64::from(x0)) as f32;
    let ty = (y - f64::from(y0)) as f32;
    let upper = at(x0, y0)? * (1.0 - tx) + at(x0 + 1, y0)? * tx;
    let lower = at(x0, y0 + 1)? * (1.0 - tx) + at(x0 + 1, y0 + 1)? * tx;
    Some(upper * (1.0 - ty) + lower * ty)
}

pub(super) fn fetch(
    request: Request,
    scratch: &Scratch,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<PathBuf> {
    let mut heights = vec![NODATA; request.width as usize * request.height as usize];
    let mut received = 0;
    for (layer, maximum) in LAYERS {
        if heights.iter().all(|&h| h != NODATA) {
            break;
        }
        let zoom = zoom_for(request, maximum);
        let (x0, y0, x1, y1) = tiles(request.bounds, zoom);
        let mut pixels = HashMap::new();
        for y in y0..=y1 {
            for x in x0..=x1 {
                pixels.insert((x, y), tile((layer, zoom, x, y), &mut received, progress)?);
            }
        }
        let b = request.bounds;
        for row in 0..request.height {
            let lat = b.max_lat
                - (f64::from(row) + 0.5) / f64::from(request.height) * (b.max_lat - b.min_lat);
            for col in 0..request.width {
                let index = (row * request.width + col) as usize;
                if heights[index] != NODATA {
                    continue;
                }
                let lon = b.min_lon
                    + (f64::from(col) + 0.5) / f64::from(request.width) * (b.max_lon - b.min_lon);
                let (x, y) = pixel(lon, lat, zoom);
                if let Some(height) = sample(&pixels, x, y) {
                    heights[index] = height;
                }
            }
        }
    }
    if heights.iter().all(|&h| h == NODATA) {
        return Err(Error::NoCoverage);
    }
    let vrt = raster::float_grid(&heights, request, scratch)?;
    raster::warp(
        &[vrt.to_string_lossy().into_owned()],
        request,
        scratch,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signed_centimetres_and_missing_data_are_distinct() {
        assert_eq!(decode(&[0, 0, 0]), 0.0);
        assert!((decode(&[0, 0, 123]) - 1.23).abs() < 0.0001);
        assert!((decode(&[255, 255, 156]) + 1.0).abs() < 0.0001);
        assert_eq!(decode(&[128, 0, 0]), NODATA);
    }
    #[test]
    fn interpolation_crosses_tile_edges_without_interpolating_nodata() {
        let mut tiles = HashMap::from([
            ((0, 0), Some(Arc::new(vec![10.0; 256 * 256]))),
            ((1, 0), Some(Arc::new(vec![20.0; 256 * 256]))),
        ]);
        assert_eq!(sample(&tiles, 255.5, 40.0), Some(15.0));
        tiles.insert((1, 0), None);
        assert_eq!(sample(&tiles, 255.5, 40.0), None);
    }
    #[test]
    fn all_contour_tiers_bound_source_tile_fanout() {
        for (span, pixels) in [
            (0.10, 464),
            (0.03, 544),
            (0.014, 544),
            (0.0068, 544),
            (0.02, 2400),
        ] {
            let request = Request {
                bounds: Bounds {
                    min_lon: 139.7,
                    max_lon: 139.7 + span,
                    min_lat: 35.6,
                    max_lat: 35.6 + span,
                },
                width: pixels,
                height: pixels,
            };
            for maximum in [14, 15, 17] {
                let zoom = zoom_for(request, maximum);
                let (x0, y0, x1, y1) = tiles(request.bounds, zoom);
                assert!(zoom <= maximum);
                assert!((x1 - x0 + 1) * (y1 - y0 + 1) <= 64);
            }
        }
    }
}
