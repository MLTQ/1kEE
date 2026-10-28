//! Restore discarded capacity only when the original raw snapshot is identical.
use super::{Capacity, Feature};
use serde::Deserialize;
use std::{collections::BTreeMap, sync::OnceLock};

#[derive(Deserialize)]
struct Catalog {
    sources: Vec<Source>,
}
#[derive(Deserialize)]
struct Source {
    id: String,
    url: String,
    file: String,
    sha256: String,
    unit: String,
    source_release: String,
    capacities: BTreeMap<String, f64>,
}

fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("capacity_snapshot.json"))
            .expect("validated bundled pipeline capacities")
    })
}

pub(super) fn enrich(features: &mut [Feature], manifest: &str) {
    let Ok(manifest) = serde_json::from_str::<serde_json::Value>(manifest) else {
        return;
    };
    let (Some(sources), Some(files)) =
        (manifest["sources"].as_array(), manifest["files"].as_array())
    else {
        return;
    };
    for source in &catalog().sources {
        if !sources
            .iter()
            .any(|s| s["id"] == source.id && s["url"] == source.url)
            || !files
                .iter()
                .any(|f| f["file"] == source.file && f["sha256"] == source.sha256)
        {
            continue;
        }
        for feature in features
            .iter_mut()
            .filter(|f| f.info.source == source.id && f.info.capacity.is_none())
        {
            if let Some(&value) = source.capacities.get(&feature.info.source_id) {
                feature.info.capacity = Some(Capacity {
                    value,
                    unit: source.unit.clone(),
                    source_release: source.source_release.clone(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supplement_requires_exact_provenance_and_never_replaces_archived_capacity() {
        let source = &catalog().sources[0];
        let (id, value) = source.capacities.first_key_value().unwrap();
        let make = || {
            Feature {
            info: serde_json::from_value(serde_json::json!({
                "source":source.id,"source_id":id,"name":"","operator":"","owner":"",
                "product":"gas","status":"unknown","accuracy":"unknown","historical":false,"planned":false
            })).unwrap(),
            points: Vec::new(),
        }
        };
        let mut manifest = serde_json::json!({"sources":[{"id":source.id,"url":source.url}],
            "files":[{"file":source.file,"sha256":source.sha256}]});
        let mut features = vec![make()];
        enrich(&mut features, &manifest.to_string());
        assert_eq!(features[0].info.capacity.as_ref().unwrap().value, *value);
        features[0].info.capacity.as_mut().unwrap().value = 123.0;
        enrich(&mut features, &manifest.to_string());
        assert_eq!(features[0].info.capacity.as_ref().unwrap().value, 123.0);
        manifest["files"][0]["sha256"] = "different raw file".into();
        let mut features = vec![make()];
        enrich(&mut features, &manifest.to_string());
        assert!(features[0].info.capacity.is_none());
        manifest["files"][0]["sha256"] = source.sha256.clone().into();
        manifest["sources"][0]["url"] = "different release".into();
        enrich(&mut features, &manifest.to_string());
        assert!(features[0].info.capacity.is_none());
        enrich(&mut features, "{}");
        assert!(features[0].info.capacity.is_none());
    }

    #[test]
    fn bundled_values_are_finite_nonnegative_and_keep_source_units() {
        let sources = &catalog().sources;
        assert_eq!(sources.len(), 2);
        for source in sources {
            assert!(source.sha256.len() == 64 && !source.url.is_empty());
            assert!(
                source
                    .capacities
                    .values()
                    .all(|v| v.is_finite() && *v >= 0.0)
            );
            let (count, unit) = match source.id.as_str() {
                "gem-gas" => (2246, "bcm/y of gas"),
                "gem-oil" => (1150, "boe/d"),
                other => panic!("unexpected source {other}"),
            };
            assert_eq!(source.capacities.len(), count);
            assert_eq!(source.unit, unit);
        }
    }
}
