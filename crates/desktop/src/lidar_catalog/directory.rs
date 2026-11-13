use super::{GeoJsonLayer, GeoPoint, Survey, connection, footprint, geography, overlapping};
use rusqlite::Row;
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};

// Keep this predicate aligned with the optional covering index in lidar_catalog.py.
// The query also works on read-only catalogs created before the index existed.
pub(super) const MAPPABLE: &str =
    "(id LIKE 'NOAAID:%' OR id LIKE 'USGS_3DEP_ID:%' OR stac IS NOT NULL
    OR source IN ('linz-coastal','eba') OR id LIKE 'INFOMAR:%:SR_URL'
    OR (source='swisssurface3d' AND asset LIKE '%.copc.laz') OR product='Lidar raster'
    OR (product IN ('NASA lidar raster','NASA lidar point cloud') AND asset IS NOT NULL))";

#[derive(Clone)]
pub(super) struct Entry {
    pub id: String,
    pub label: String,
    pub local: Option<Survey>,
}

pub(super) type Directory = BTreeMap<&'static str, BTreeMap<&'static str, Vec<Entry>>>;

pub(super) fn survey_row(row: &Row<'_>) -> rusqlite::Result<Survey> {
    Ok(Survey {
        id: row.get(0)?,
        name: row.get(1)?,
        product: row.get(2)?,
        date: row.get(3)?,
        page: row.get(4)?,
        asset: row.get(5)?,
        source: row.get(6)?,
    })
}

pub(super) fn collection(id: &str, name: &str) -> Option<(String, String)> {
    let slash = id.starts_with("LINZ:").then(|| id.rfind('/')).flatten()?;
    Some((
        id[..=slash].into(),
        name.split_once(" · ").map_or(name, |p| p.0).into(),
    ))
}

pub(super) fn open(path: &Path, id: &str) -> Result<(Survey, GeoJsonLayer, GeoPoint), String> {
    let db = connection(path)?;
    let survey = db
        .query_row(
            "SELECT id,name,product,date,page,asset,source FROM surveys WHERE id=?",
            [id],
            survey_row,
        )
        .map_err(|e| e.to_string())?;
    let (mut survey, layer, focus) = footprint(&db, survey)?;
    if let Some((id, title)) = collection(&survey.id, &survey.name) {
        survey.id = id;
        survey.name = title;
    }
    Ok((survey, layer, focus))
}

pub(super) fn label(name: &str, date: Option<&str>) -> String {
    // Many publishers bake the flight year into the start of the title.
    let (title, prefix) = name
        .split_once(' ')
        .and_then(|(token, rest)| {
            let years = token.trim_end_matches(':');
            let mut parts = years.split(['-', '–', '/']);
            let year =
                |s: &str| s.len() == 4 && s.parse::<u16>().is_ok_and(|y| (1800..2200).contains(&y));
            if parts.all(year) && !rest.trim().is_empty() {
                Some((rest.trim_start(), Some(years)))
            } else {
                None
            }
        })
        .unwrap_or((name, None));
    let date = date
        .and_then(|d| d.split('T').next())
        .filter(|d| !d.is_empty());
    match (prefix, date) {
        (Some(p), Some(d)) if !d.starts_with(p) => format!("{title} · {p} ({d})"),
        (_, Some(d)) => format!("{title} · {d}"),
        (Some(p), None) => format!("{title} · {p}"),
        _ => title.to_owned(),
    }
}

fn insert(groups: &mut Directory, places: [Option<geography::Place>; 2], entry: Entry) {
    for (group, place) in places.into_iter().flatten() {
        groups
            .entry(group)
            .or_default()
            .entry(place)
            .or_default()
            .push(entry.clone());
    }
}

fn sort(groups: &mut Directory) {
    for places in groups.values_mut() {
        for entries in places.values_mut() {
            entries.sort_by_cached_key(|e| (e.label.to_lowercase(), e.id.clone()));
        }
    }
}

pub(super) fn at_center(path: &Path, at: GeoPoint) -> Result<Directory, String> {
    let mut groups = Directory::new();
    for s in overlapping(path, at)? {
        insert(
            &mut groups,
            geography::places(&s.source, at.lon, at.lat),
            Entry {
                id: s.id.clone(),
                label: label(&s.name, s.date.as_deref()),
                local: Some(s),
            },
        );
    }
    sort(&mut groups);
    Ok(groups)
}

pub(super) fn worldwide(path: &Path) -> Result<Directory, String> {
    let db = connection(path)?;
    let mut transects = HashSet::<String>::new();
    if let Ok(bulk) = connection(&path.with_file_name("bulk.sqlite"))
        && let Ok(mut stmt) = bulk.prepare("SELECT DISTINCT survey FROM transects")
    {
        let rows = stmt
            .query_map([], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        for row in rows {
            transects.insert(row.map_err(|e| e.to_string())?);
        }
    }
    let mut groups = Directory::new();
    let mut collections = HashSet::new();
    let sql = format!(
        "SELECT source,id,name,date,west,south,east,north FROM surveys WHERE {MAPPABLE} ORDER BY source,id"
    );
    let mut stmt = db.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                [r.get::<_, f32>(4)?, r.get(5)?, r.get(6)?, r.get(7)?],
            ))
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (source, id, name, date, [w, s, e, n]) = row.map_err(|e| e.to_string())?;
        if id.starts_with("EBA:") && !transects.contains(&id) {
            continue;
        }
        // Catalog bounds use min/max longitude, including footprints split at ±180.
        let lon = if e - w > 180. {
            ((w + e) / 2. + 360.).rem_euclid(360.) - 180.
        } else {
            (w + e) / 2.
        };
        let places = geography::places(&source, lon, (s + n) / 2.);
        if let Some((key, title)) = collection(&id, &name) {
            // Keep a real tile ID for focusing this collection in each place it covers.
            for place in places.into_iter().flatten() {
                if collections.insert((place, key.clone())) {
                    insert(
                        &mut groups,
                        [Some(place), None],
                        Entry {
                            id: id.clone(),
                            label: label(&title, date.as_deref()),
                            local: None,
                        },
                    );
                }
            }
        } else {
            insert(
                &mut groups,
                places,
                Entry {
                    id,
                    label: label(&name, date.as_deref()),
                    local: None,
                },
            );
        }
    }
    sort(&mut groups);
    Ok(groups)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_lead_and_dates_distinguish_repeated_flights() {
        assert_eq!(
            label("1998 Coastal flight", Some("1998-03-01T00:00:00Z")),
            "Coastal flight · 1998-03-01"
        );
        assert_eq!(label("2010–2012 Shelf", None), "Shelf · 2010–2012");
        assert_eq!(label("3DEP Pacific", None), "3DEP Pacific");
    }
}
