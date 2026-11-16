//! NCEM's statewide LiDAR-derived DEM (3.125 US-survey-foot cells).
use super::*;

const SERVICE: &str =
    "https://services.gis.nc.gov/secure/rest/services/Elevation/DEM03/ImageServer/exportImage";
const US_SURVEY_FOOT_M: f64 = 1200.0 / 3937.0;

pub(super) fn request_url(request: Request) -> String {
    let b = request.bounds;
    let mut url = reqwest::Url::parse(SERVICE).unwrap();
    url.query_pairs_mut().extend_pairs([
        (
            "bbox",
            format!("{},{},{},{}", b.min_lon, b.min_lat, b.max_lon, b.max_lat),
        ),
        ("bboxSR", "4326".into()),
        ("imageSR", "4326".into()),
        ("size", format!("{},{}", request.width, request.height)),
        ("format", "tiff".into()),
        ("pixelType", "F32".into()),
        ("noData", NODATA.to_string()),
        ("interpolation", "RSP_BilinearInterpolation".into()),
        ("renderingRule", r#"{"rasterFunction":"None"}"#.into()),
        ("f", "image".into()),
    ]);
    url.into()
}

fn metres(value: f32) -> f32 {
    if value.is_finite() && value != NODATA {
        (f64::from(value) * US_SURVEY_FOOT_M) as f32
    } else {
        NODATA
    }
}

pub(super) fn fetch(
    request: Request,
    scratch: &Scratch,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<PathBuf> {
    let raw = scratch.path("north-carolina.tif");
    http::download(&request_url(request), &raw, progress)?;
    // Normalize XY and nodata before converting original source elevations.
    raster::warp(
        &[raw.to_string_lossy().into_owned()],
        request,
        scratch,
        false,
    )?;
    let bytes = std::fs::read(scratch.path("check.bil"))?;
    let samples = bytes
        .chunks_exact(4)
        .map(|p| metres(f32::from_le_bytes(p.try_into().unwrap())))
        .collect::<Vec<_>>();
    let vrt = raster::float_grid(&samples, request, scratch)?;
    // Unit conversion only: retain the publisher's vertical reference.
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
    fn source_feet_are_converted_without_turning_nodata_into_valid_heights() {
        assert!((metres(1000.) - 304.8006).abs() < 0.0001);
        assert!((metres(-100.) + 30.48006).abs() < 0.00001);
        assert_eq!(metres(0.), 0.);
        assert_eq!(metres(NODATA), NODATA);
        assert_eq!(metres(f32::NAN), NODATA);
    }
}
