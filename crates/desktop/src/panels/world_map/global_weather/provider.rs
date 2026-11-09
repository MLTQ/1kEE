//! Resolve published frame times before requesting a geographic whole-Earth WMS image.
use super::Product;
use crate::{event_store, satellite_image::SatelliteImage};
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

fn get(request: RequestBuilder, limit: u64) -> Result<Vec<u8>, String> {
    let response = request
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?;
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

pub(super) fn latest(client: &Client, product: Product) -> Result<String, String> {
    let cutoff = event_store::unix_to_date_str(event_store::now_unix() - 86400);
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
    let xml = get(request, 512 << 10)?;
    frame_time(
        product,
        std::str::from_utf8(&xml).map_err(|e| e.to_string())?,
        &cutoff,
    )
}

fn frame_time(product: Product, xml: &str, cutoff: &str) -> Result<String, String> {
    let doc =
        roxmltree::Document::parse(xml).map_err(|e| format!("Invalid provider metadata: {e}"))?;
    let stamp = match product {
        Product::TrueColor => doc
            .descendants()
            .find(|n| n.has_tag_name("Domain"))
            .and_then(|n| n.text())
            .into_iter()
            .flat_map(|s| s.split(','))
            .filter_map(|range| {
                let mut parts = range.trim().split('/');
                let first = parts.next()?;
                let last = parts.next().unwrap_or(first);
                // Only daily periods or single dates are members of this visualization.
                if first > cutoff || parts.next().is_some_and(|p| p != "P1D") {
                    return None;
                }
                Some(last.min(cutoff))
            })
            .max(),
        Product::Infrared => doc
            .descendants()
            .filter(|n| n.has_tag_name("Layer"))
            .find(|n| {
                n.children()
                    .any(|c| c.has_tag_name("Name") && c.text() == Some(RING))
            })
            .and_then(|layer| {
                layer
                    .children()
                    .find(|c| c.has_tag_name("Dimension") && c.attribute("name") == Some("time"))
            })
            .and_then(|n| n.attribute("default")),
    }
    .ok_or("Provider did not publish a frame time")?;
    let pattern: &[u8] = match product {
        Product::TrueColor => b"0000-00-00",
        Product::Infrared => b"0000-00-00T00:00:00Z",
    };
    if stamp.len() != pattern.len()
        || !stamp.bytes().zip(pattern).all(|(ch, &p)| {
            if p == b'0' {
                ch.is_ascii_digit()
            } else {
                ch == p
            }
        })
    {
        return Err("Invalid provider frame time".into());
    }
    Ok(stamp.to_owned())
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
    fn selects_latest_available_day_up_to_cutoff_including_gaps() {
        let xml = "<Domains><DimensionDomain><Domain>2026-10-01/2026-10-30/P1D,2026-11-02,2026-11-04/2026-11-09/P1D</Domain></DimensionDomain></Domains>";
        for (cutoff, expected) in [("2026-11-08", "2026-11-08"), ("2026-11-03", "2026-11-02")] {
            assert_eq!(
                frame_time(Product::TrueColor, xml, cutoff).unwrap(),
                expected
            );
        }
    }
    #[test]
    fn uses_ir_time_of_named_layer_and_rejects_missing_or_invalid_metadata() {
        let xml = r#"<Layer><Layer><Name>other</Name><Dimension name="time" default="bad"/></Layer><Layer><Name>worldcloudmap_ir108</Name><Dimension name="time" default="2026-11-09T12:00:00Z"/></Layer></Layer>"#;
        assert_eq!(
            frame_time(Product::Infrared, xml, "").unwrap(),
            "2026-11-09T12:00:00Z"
        );
        assert!(frame_time(Product::Infrared, &xml.replace(RING, "other"), "").is_err());
        assert!(
            frame_time(
                Product::Infrared,
                &xml.replace("2026-11-09T12:00:00Z", "current"),
                ""
            )
            .is_err()
        );
    }
}
