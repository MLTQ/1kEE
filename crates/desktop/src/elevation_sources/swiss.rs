use super::*;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

const ITEMS: &str =
    "https://data.geo.admin.ch/api/stac/v0.9/collections/ch.swisstopo.swissalti3d/items";

fn assets(
    page: &Value,
    prefer_two_metres: bool,
    selected: &mut BTreeMap<String, (String, String)>,
) -> Result<()> {
    let features = page["features"]
        .as_array()
        .ok_or_else(|| Error::Failed("invalid swissALTI3D catalog".into()))?;
    for feature in features {
        let id = feature["id"]
            .as_str()
            .ok_or_else(|| Error::Failed("missing swissALTI3D tile ID".into()))?;
        let cell = id.rsplit('_').next().unwrap_or(id);
        let Some(assets) = feature["assets"].as_object() else {
            continue;
        };
        let mut rasters: Vec<_> = assets
            .iter()
            .filter(|(name, asset)| {
                name.ends_with(".tif")
                    && name.contains("_2056_")
                    && asset["href"].as_str().is_some()
            })
            .collect();
        rasters.sort_by_key(|(name, _)| {
            if prefer_two_metres {
                if name.contains("_2_2056_") { 0 } else { 1 }
            } else if name.contains("_0.5_2056_") {
                0
            } else {
                1
            }
        });
        let Some((_, asset)) = rasters.first() else {
            continue;
        };
        let href = asset["href"].as_str().unwrap();
        http::validate_url(href)?;
        // IDs carry survey year followed by the stable 1 km cell address.
        // Only one edition per cell is needed; older and newer surveys overlap.
        if selected.get(cell).is_none_or(|(old, _)| id > old.as_str()) {
            selected.insert(cell.to_owned(), (id.to_owned(), href.to_owned()));
        }
    }
    Ok(())
}

pub(super) fn fetch(request: Request, scratch: &Scratch) -> Result<PathBuf> {
    let b = request.bounds;
    let mut url = format!(
        "{ITEMS}?bbox={},{},{},{}&limit=100",
        b.min_lon, b.min_lat, b.max_lon, b.max_lat
    );
    let prefer_two_metres = (b.max_lat - b.min_lat) * 111_320.0 / f64::from(request.height) >= 2.0;
    let mut selected = BTreeMap::new();
    let mut visited = HashSet::new();
    loop {
        if visited.len() >= 12 || !visited.insert(url.clone()) {
            return Err(Error::Failed(
                "swissALTI3D catalog pagination exceeded its limit".into(),
            ));
        }
        let page = http::json(&url)?;
        assets(&page, prefer_two_metres, &mut selected)?;
        if selected.len() > 256 {
            return Err(Error::Failed("too many swissALTI3D source tiles".into()));
        }
        let next = page["links"]
            .as_array()
            .and_then(|links| links.iter().find(|link| link["rel"] == "next"))
            .and_then(|link| link["href"].as_str());
        let Some(next) = next else {
            break;
        };
        url = http::resolve(&url, next)?;
    }
    let inputs = selected
        .into_values()
        .map(|(_, href)| format!("/vsicurl/{href}"))
        .collect::<Vec<_>>();
    raster::warp(&inputs, request, scratch, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_uses_latest_survey_and_requested_resolution_without_duplicates() {
        let page = serde_json::json!({"features": [
            {"id":"swissalti3d_2025_2600-1198", "assets":{
                "a_0.5_2056_5728.tif":{"href":"https://data.geo.admin.ch/fine.tif"},
                "a_2_2056_5728.tif":{"href":"https://data.geo.admin.ch/coarse.tif"}}},
            {"id":"swissalti3d_2019_2600-1198", "assets":{
                "a_0.5_2056_5728.tif":{"href":"https://data.geo.admin.ch/old.tif"}}}
        ]});
        let mut selected = BTreeMap::new();
        assets(&page, false, &mut selected).unwrap();
        assert_eq!(selected.len(), 1);
        assert!(selected["2600-1198"].1.ends_with("/fine.tif"));
        selected.clear();
        assets(&page, true, &mut selected).unwrap();
        assert!(selected["2600-1198"].1.ends_with("/coarse.tif"));
    }
}
