//! Resolve published frame times before requesting a geographic whole-Earth WMS image.
use super::Product;
use crate::{event_store, satellite_image::SatelliteImage};
use chrono::{DateTime, NaiveDate, Utc};
use reqwest::blocking::{Client, RequestBuilder};
use std::{
    io::{Cursor, Read},
    time::Duration,
};

const VIIRS: &str = "VIIRS_NOAA20_CorrectedReflectance_TrueColor";
const RING: &str = "worldcloudmap_ir108";
const NASA_DOMAINS: &str = "https://gibs.earthdata.nasa.gov/wmts/epsg4326/best/wmts.cgi";
const NASA_MAP: &str = "https://gibs.earthdata.nasa.gov/wms/epsg4326/best/wms.cgi";
const EUMET_RING: &str = "https://view.eumetsat.int/geoserver/mumi/worldcloudmap_ir108/wms";
pub(super) const MAX_BYTES: u64 = 40 << 20;

pub(super) fn client() -> Result<Client, String> {
    Client::builder()
        .user_agent("1kEE global weather imagery")
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(90))
        .build()
        .map_err(|e| e.to_string())
}

fn check_ir_time(stamp: &str, warning: &str) -> Result<(), String> {
    // GeoServer can also report "Nearest value used" for an exact match.
    if let Some((_, value)) = warning.split_once("time=") {
        let actual = value.split_whitespace().next().unwrap_or_default();
        if parse_stamp(Product::Infrared, actual) != parse_stamp(Product::Infrared, stamp) {
            return Err(format!("Requested {stamp}, provider returned {actual}"));
        }
    }
    Ok(())
}

fn get(
    request: RequestBuilder,
    limit: u64,
    infrared_time: Option<&str>,
) -> Result<Vec<u8>, String> {
    let response = request
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?;
    if let Some(stamp) = infrared_time {
        for warning in response.headers().get_all(reqwest::header::WARNING) {
            check_ir_time(stamp, warning.to_str().map_err(|e| e.to_string())?)?;
        }
    }
    let mut bytes = Vec::new();
    response
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("Provider response exceeds the size limit".into());
    }
    Ok(bytes)
}

pub(super) fn catalog(client: &Client, product: Product) -> Result<Vec<String>, String> {
    let request = match product {
        Product::TrueColor => client.get(NASA_DOMAINS).query(&[
            ("SERVICE", "WMTS"),
            ("REQUEST", "DescribeDomains"),
            ("VERSION", "1.0.0"),
            ("LAYER", VIIRS),
            ("TILEMATRIXSET", "250m"),
        ]),
        Product::Infrared => client.get(EUMET_RING).query(&[
            ("SERVICE", "WMS"),
            ("REQUEST", "GetCapabilities"),
            ("VERSION", "1.3.0"),
        ]),
    };
    let xml = get(request, 512 << 10, None)?;
    frame_times(
        product,
        std::str::from_utf8(&xml).map_err(|e| e.to_string())?,
        event_store::now_unix(),
    )
}

pub(super) fn parse_stamp(product: Product, s: &str) -> Option<i64> {
    match product {
        Product::TrueColor => Some(
            NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .ok()?
                .and_hms_opt(0, 0, 0)?
                .and_utc()
                .timestamp(),
        ),
        Product::Infrared => Some(DateTime::parse_from_rfc3339(s).ok()?.timestamp()),
    }
}

fn format_stamp(product: Product, seconds: i64) -> Option<String> {
    let time = DateTime::<Utc>::from_timestamp(seconds, 0)?;
    let format = match product {
        Product::TrueColor => "%Y-%m-%d",
        Product::Infrared => "%Y-%m-%dT%H:%M:%SZ",
    };
    Some(time.format(format).to_string())
}

fn frame_times(product: Product, xml: &str, now: i64) -> Result<Vec<String>, String> {
    let doc =
        roxmltree::Document::parse(xml).map_err(|e| format!("Invalid provider metadata: {e}"))?;
    let (domain, default, cutoff, period, seconds) = match product {
        Product::TrueColor => (
            doc.descendants()
                .find(|n| n.has_tag_name("Domain"))
                .and_then(|n| n.text()),
            None,
            now.div_euclid(86400) * 86400 - 1,
            "P1D",
            86400,
        ),
        Product::Infrared => {
            let dimension = doc
                .descendants()
                .filter(|n| n.has_tag_name("Layer"))
                .find(|n| {
                    n.children()
                        .any(|c| c.has_tag_name("Name") && c.text() == Some(RING))
                })
                .and_then(|n| {
                    n.children().find(|c| {
                        c.has_tag_name("Dimension") && c.attribute("name") == Some("time")
                    })
                })
                .ok_or("Missing infrared time dimension")?;
            let default = dimension
                .attribute("default")
                .and_then(|s| parse_stamp(product, s));
            (
                dimension.text(),
                default,
                default.unwrap_or(now).min(now),
                "PT3H",
                10800,
            )
        }
    };
    let mut ranges = Vec::new();
    for range in domain.unwrap_or("").split(',') {
        let mut parts = range.trim().split('/');
        let Some(first) = parts.next().and_then(|s| parse_stamp(product, s)) else {
            continue;
        };
        let last = match parts.next() {
            None => first,
            Some(end) if parts.next() == Some(period) && parts.next().is_none() => {
                let Some(end) = parse_stamp(product, end) else {
                    continue;
                };
                end
            }
            _ => continue,
        }
        .min(cutoff);
        if first <= last {
            ranges.push((first, first + ((last - first) / seconds) * seconds));
        }
    }
    if let Some(time) = default.filter(|&t| t <= cutoff) {
        ranges.push((time, time));
    }
    let latest = ranges
        .iter()
        .map(|r| r.1)
        .max()
        .ok_or("Provider did not publish a frame time")?;
    // Expand only the trailing week, even when the provider advertises years.
    let mut times = std::collections::BTreeSet::new();
    for (first, last) in ranges {
        let mut t = last;
        while t >= first && t >= latest - 7 * 86400 {
            if let Some(stamp) = format_stamp(product, t) {
                times.insert(stamp);
            }
            t -= seconds;
        }
    }
    Ok(times.into_iter().collect())
}

pub(super) fn size(product: Product) -> [u32; 2] {
    match product {
        Product::TrueColor => [4096, 2048],
        Product::Infrared => [2048, 1024],
    }
}

pub(super) fn fetch_png(client: &Client, product: Product, stamp: &str) -> Result<Vec<u8>, String> {
    fetch_map(
        client,
        product,
        stamp,
        [-180., -90., 180., 90.],
        size(product),
    )
}

pub(super) fn fetch_map(
    client: &Client,
    product: Product,
    stamp: &str,
    bounds: [f64; 4],
    [w, h]: [u32; 2],
) -> Result<Vec<u8>, String> {
    let (url, layer) = match product {
        Product::TrueColor => (NASA_MAP, VIIRS),
        Product::Infrared => (EUMET_RING, RING),
    };
    let [west, south, east, north] = bounds;
    // WMS 1.3 orders EPSG:4326 bounds by latitude, then longitude.
    get(
        client.get(url).query(&[
            ("SERVICE", "WMS"),
            ("VERSION", "1.3.0"),
            ("REQUEST", "GetMap"),
            ("LAYERS", layer),
            ("STYLES", ""),
            ("FORMAT", "image/png"),
            ("TRANSPARENT", "TRUE"),
            ("TIME", stamp),
            ("CRS", "EPSG:4326"),
            ("BBOX", &format!("{south},{west},{north},{east}")),
            ("WIDTH", &w.to_string()),
            ("HEIGHT", &h.to_string()),
        ]),
        MAX_BYTES,
        matches!(product, Product::Infrared).then_some(stamp),
    )
}

pub(super) fn decode(
    product: Product,
    bytes: &[u8],
    max_side: usize,
) -> Result<SatelliteImage, String> {
    let [w, h] = size(product);
    let mut image = decode_png(bytes, [w, h])?;
    let width = w.min(max_side.clamp(2, 4096) as u32);
    if width < w {
        image = image.resize_exact(width, width / 2, image::imageops::FilterType::Triangle);
    }
    let pixels = image.into_rgba8();
    if !pixels
        .pixels()
        .any(|p| p[3] != 0 && p.0[..3].iter().any(|&v| v != 0))
    {
        return Err("Provider returned an empty map".into());
    }
    Ok(SatelliteImage {
        bounds: [-180.0, -90.0, 180.0, 90.0],
        pixels: egui::ColorImage::from_rgba_unmultiplied(
            [pixels.width() as usize, pixels.height() as usize],
            &pixels,
        ),
    })
}

pub(super) fn decode_png(bytes: &[u8], [w, h]: [u32; 2]) -> Result<image::DynamicImage, String> {
    let reader = || image::ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Png);
    if reader()
        .into_dimensions()
        .map_err(|e| format!("Invalid map PNG: {e}"))?
        != (w, h)
    {
        return Err("Provider returned unexpected map dimensions".into());
    }
    reader().decode().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_accepts_an_exact_infrared_match_in_geoserver_warning() {
        let warning = "99 Nearest value used: time=2026-11-06T21:00:00.000Z ISO8601 (mumi:worldcloudmap_ir108)";
        assert!(check_ir_time("2026-11-06T21:00:00Z", warning).is_ok());
        assert!(check_ir_time("2026-11-07T00:00:00Z", warning).is_err());
    }
    #[test]
    fn daily_catalog_honors_gaps_calendar_boundaries_and_the_utc_cutoff() {
        let xml = "<Domains><Domain>2018-01-01/2024-02-27/P1D,2024-02-29,2024-03-02/2024-03-06/P1D</Domain></Domains>";
        let now = parse_stamp(Product::Infrared, "2024-03-04T01:00:00Z").unwrap();
        assert_eq!(
            frame_times(Product::TrueColor, xml, now).unwrap(),
            [
                "2024-02-25",
                "2024-02-26",
                "2024-02-27",
                "2024-02-29",
                "2024-03-02",
                "2024-03-03"
            ]
        );
    }
    #[test]
    fn infrared_catalog_uses_the_named_layer_and_only_advertised_slots() {
        let xml = r#"<Layer><Layer><Name>other</Name><Dimension name="time" default="bad"/></Layer><Layer><Name>worldcloudmap_ir108</Name><Dimension name="time" default="2026-11-09T12:00:00Z">2026-11-08T21:00:00.000Z/2026-11-09T04:00:00.000Z/PT3H,2026-11-09T12:00:00.000Z</Dimension></Layer></Layer>"#;
        let now = parse_stamp(Product::Infrared, "2026-11-09T14:00:00Z").unwrap();
        assert_eq!(
            frame_times(Product::Infrared, xml, now).unwrap(),
            [
                "2026-11-08T21:00:00Z",
                "2026-11-09T00:00:00Z",
                "2026-11-09T03:00:00Z",
                "2026-11-09T12:00:00Z"
            ]
        );
        assert!(frame_times(Product::Infrared, &xml.replace(RING, "other"), now).is_err());
        assert!(frame_times(Product::TrueColor, "<Domain>bad</Domain>", now).is_err());
    }
}
