//! Convert one ArcGIS point placemark into the report used by the event queue and both maps.
use crate::model::{
    EventRecord, EventSeverity, GHOSTMAPS_EVENT_PREFIX, GeoJsonFeature, GeoJsonGeometry,
};
use roxmltree::Node;
use scraper::{Html, Selector};
use sha1::{Digest, Sha1};
use std::{collections::BTreeMap, sync::LazyLock};

const SOURCE: &str = "S2 Underground / GhostMaps";
static ROWS: LazyLock<Selector> = LazyLock::new(|| Selector::parse("tr").unwrap());

#[derive(Default)]
struct Details {
    fields: BTreeMap<String, String>,
    text: String,
}

impl Details {
    fn insert(&mut self, key: &str, value: &str) {
        let value = value.trim();
        if !value.is_empty() && !value.eq_ignore_ascii_case("<null>") {
            let key = key.to_ascii_lowercase().replace([' ', '_'], "");
            self.fields.insert(key, value.into());
        }
    }

    fn parse(node: Node<'_, '_>) -> Self {
        let mut details = Self::default();
        if let Some(description) = node
            .children()
            .find(|n| n.has_tag_name("description"))
            .and_then(|n| n.text())
        {
            let html = Html::parse_fragment(description);
            for row in html.select(&ROWS) {
                let mut cells = row.child_elements().filter(|el| el.value().name() == "td");
                if let (Some(key), Some(value), None) = (cells.next(), cells.next(), cells.next()) {
                    details.insert(
                        &key.text().collect::<String>(),
                        &value.text().collect::<String>(),
                    );
                }
            }
            if details.fields.is_empty() {
                details.text = html
                    .root_element()
                    .text()
                    .flat_map(str::split_whitespace)
                    .collect::<Vec<_>>()
                    .join(" ");
            }
        }
        // Other KML publishers/export modes carry the same attributes as XML.
        for field in node
            .descendants()
            .filter(|n| n.has_tag_name("SimpleData") || n.has_tag_name("Data"))
        {
            let value = if field.has_tag_name("SimpleData") {
                field.text()
            } else {
                field
                    .children()
                    .find(|n| n.has_tag_name("value"))
                    .and_then(|n| n.text())
            };
            if let (Some(key), Some(value)) = (field.attribute("name"), value) {
                details.insert(key, value);
            }
        }
        details
    }

    fn first(&self, keys: &[&str]) -> Option<&str> {
        keys.iter()
            .find_map(|key| self.fields.get(*key).map(String::as_str))
    }
}

pub(super) fn collect(
    by_folder: &mut BTreeMap<String, Vec<EventRecord>>,
    folder: &str,
    node: Node<'_, '_>,
    features: &[GeoJsonFeature],
) {
    let mut points = features
        .iter()
        .filter_map(|feature| {
            let GeoJsonGeometry::Point(location) = feature.geometry else {
                return None;
            };
            ((-90.0..=90.0).contains(&location.lat) && (-180.0..=180.0).contains(&location.lon))
                .then_some((feature.label.as_deref(), location))
        })
        .peekable();
    if points.peek().is_none() {
        return;
    }
    let details = Details::parse(node);
    let summary = details
        .first(&["description", "notes"])
        .unwrap_or(&details.text);
    let occurred = node
        .children()
        .find(|n| n.has_tag_name("TimeStamp"))
        .and_then(|n| n.children().find(|c| c.has_tag_name("when")))
        .and_then(|n| n.text())
        .or_else(|| details.first(&["dateofevent", "eventdate", "date"]));
    let reports = by_folder.entry(folder.to_owned()).or_default();
    for (label, location) in points {
        let title = label.unwrap_or(folder);
        // ArcGIS renumbers KML IDs across exports. Match equivalent reports in
        // the overlapping rolling windows by their published content.
        let mut identity = Sha1::new();
        for field in [folder, title, occurred.unwrap_or(""), summary] {
            identity.update(field.as_bytes());
            identity.update([0]);
        }
        identity.update(location.lat.to_bits().to_be_bytes());
        identity.update(location.lon.to_bits().to_be_bytes());
        reports.push(EventRecord {
            id: format!("{GHOSTMAPS_EVENT_PREFIX}{:x}", identity.finalize()),
            title: title.to_owned(),
            summary: summary.to_owned(),
            severity: EventSeverity::Unrated,
            location_name: details
                .first(&["fullstreetaddress", "location", "geolocationnotes"])
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{folder} · {:.4}, {:.4}", location.lat, location.lon)),
            location,
            source: SOURCE.into(),
            occurred_at: occurred.unwrap_or("Undated").to_owned(),
            factal_brief: None,
        });
    }
}
