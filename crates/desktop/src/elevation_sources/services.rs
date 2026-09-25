//! Numeric WCS/WMS requests; never request shaded or colourized map layers.
use super::*;
use reqwest::Url;

pub(super) fn request_url(provider: Provider, request: Request) -> Result<String> {
    let b = request.bounds;
    let bbox = format!("{},{},{},{}", b.min_lon, b.min_lat, b.max_lon, b.max_lat);
    let (base, layer) = match provider {
        Provider::England => (
            "https://environment.data.gov.uk/spatialdata/lidar-composite-digital-terrain-model-dtm-1m/wcs",
            "13787b9a-26a4-4775-8523-806d13af58fc:Lidar_Composite_Elevation_DTM_1m",
        ),
        Provider::Netherlands => ("https://service.pdok.nl/rws/ahn/wcs/v1_0", "dtm_05m"),
        Provider::France => (
            "https://data.geopf.fr/wms-r/wms",
            "IGNF_LIDAR-HD_MNT_ELEVATION.ELEVATIONGRIDCOVERAGE.WGS84G",
        ),
        _ => return Err(Error::Failed("not a coverage service".into())),
    };
    let mut url = Url::parse(base).unwrap();
    if provider == Provider::France {
        url.query_pairs_mut().extend_pairs([
            ("SERVICE", "WMS"),
            ("VERSION", "1.3.0"),
            ("REQUEST", "GetMap"),
            ("LAYERS", layer),
            ("STYLES", ""),
            ("CRS", "CRS:84"),
            ("BBOX", &bbox),
            ("FORMAT", "image/geotiff"),
        ]);
    } else {
        url.query_pairs_mut().extend_pairs([
            ("service", "WCS"),
            ("version", "1.0.0"),
            ("request", "GetCoverage"),
            ("coverage", layer),
            ("crs", "EPSG:4326"),
            ("response_crs", "EPSG:4326"),
            ("bbox", &bbox),
            (
                "format",
                if provider == Provider::England {
                    "GeoTIFF"
                } else {
                    "image/tiff"
                },
            ),
        ]);
    }
    url.query_pairs_mut()
        .append_pair("WIDTH", &request.width.to_string())
        .append_pair("HEIGHT", &request.height.to_string());
    Ok(url.into())
}

pub(super) fn fetch(
    provider: Provider,
    request: Request,
    scratch: &Scratch,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<PathBuf> {
    let path = scratch.path("source.tif");
    http::download(&request_url(provider, request)?, &path, progress)?;
    // GDAL validates the TIFF, single-band numeric values, bounds and nodata.
    raster::warp(
        &[path.to_string_lossy().into_owned()],
        request,
        scratch,
        false,
    )
}
