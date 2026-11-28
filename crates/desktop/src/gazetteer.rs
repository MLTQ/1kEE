//! Persistent read-only multi-world offline gazetteer (Earth, Moon, Mars)
//! backed by `Derived/gazetteer.sqlite` (FTS5) with seamless fallback to
//! `city_catalog` and built-in Earth/Moon/Mars landmarks + coordinate parsing.

use crate::city_catalog;
use crate::model::{ActiveBody, GeoPoint};
use crate::terrain_assets;
use rusqlite::{Connection, OpenFlags, params};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug)]
pub struct GazetteerHit {
    pub name: String,
    pub body: ActiveBody,
    pub kind: String,
    pub region: String,
    pub location: GeoPoint,
    pub elevation_m: Option<f32>,
    pub population: u32,
}

struct CachedConn {
    path: Option<PathBuf>,
    conn: Option<Connection>,
    last_query: String,
    last_limit: usize,
    last_results: Vec<GazetteerHit>,
}

fn state() -> &'static Mutex<CachedConn> {
    static STATE: OnceLock<Mutex<CachedConn>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(CachedConn {
            path: None,
            conn: None,
            last_query: String::from("\0"),
            last_limit: 0,
            last_results: Vec::new(),
        })
    })
}

/// Parse raw coordinates like `"37.7749, -122.4194"`, `"35.68N 139.76E"`, or `"-33.86 151.20"`.
pub fn parse_coordinates(input: &str) -> Option<GeoPoint> {
    let cleaned = input.trim().replace(',', " ");
    let parts: Vec<&str> = cleaned.split_whitespace().collect();
    if parts.len() != 2 {
        return None;
    }
    let lat = parse_hemi_coord(parts[0], 'N', 'S')?;
    let lon = parse_hemi_coord(parts[1], 'E', 'W')?;
    ((-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon))
        .then_some(GeoPoint { lat, lon })
}

fn parse_hemi_coord(token: &str, pos_hemi: char, neg_hemi: char) -> Option<f32> {
    let t = token.trim().trim_end_matches('°');
    if let Some(rest) = t
        .strip_suffix(pos_hemi)
        .or_else(|| t.strip_suffix(pos_hemi.to_ascii_lowercase()))
    {
        return rest.parse::<f32>().ok().filter(|v| v.is_finite());
    }
    if let Some(rest) = t
        .strip_suffix(neg_hemi)
        .or_else(|| t.strip_suffix(neg_hemi.to_ascii_lowercase()))
    {
        return rest.parse::<f32>().ok().filter(|v| v.is_finite()).map(|v| -v.abs());
    }
    t.parse::<f32>().ok().filter(|v| v.is_finite())
}

pub fn search(query: &str, limit: usize) -> Vec<GazetteerHit> {
    let trimmed = query.trim();
    if let Some(pt) = parse_coordinates(trimmed) {
        return vec![GazetteerHit {
            name: format!("Coordinates {:.4}°, {:.4}°", pt.lat, pt.lon),
            body: ActiveBody::Earth,
            kind: "coordinates".into(),
            region: "Direct Jump".into(),
            location: pt,
            elevation_m: None,
            population: 0,
        }];
    }

    let mut guard = state().lock().unwrap_or_else(|p| p.into_inner());
    if guard.last_query == trimmed && guard.last_limit == limit {
        return guard.last_results.clone();
    }

    let db_path = terrain_assets::find_derived_root(None)
        .map(|r| r.join("gazetteer.sqlite"))
        .filter(|p| p.exists());

    if guard.path != db_path {
        guard.conn = db_path
            .as_ref()
            .and_then(|p| Connection::open_with_flags(p, OpenFlags::SQLITE_OPEN_READ_ONLY).ok());
        guard.path = db_path;
    }

    let mut results = if let Some(conn) = guard.conn.as_ref() {
        search_sqlite_fts(conn, trimmed, limit)
    } else {
        Vec::new()
    };

    if results.is_empty() {
        results = search_fallback(trimmed, limit);
    }

    guard.last_query = trimmed.to_owned();
    guard.last_limit = limit;
    guard.last_results = results.clone();
    results
}

fn search_sqlite_fts(conn: &Connection, query: &str, limit: usize) -> Vec<GazetteerHit> {
    if query.is_empty() {
        let Ok(mut stmt) = conn.prepare(
            "SELECT name, body, kind, region, lat, lon, elevation_m, population
             FROM places ORDER BY importance DESC, population DESC LIMIT ?1",
        ) else {
            return Vec::new();
        };
        return stmt
            .query_map(params![limit as i64], map_hit_row)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .collect();
    }

    // Build safe FTS5 prefix tokens: "olymp mons" -> "\"olymp\"* \"mons\"*"
    let tokens: Vec<String> = query
        .split_whitespace()
        .filter_map(|tok| {
            let clean: String = tok
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '\'')
                .collect();
            (!clean.is_empty()).then(|| format!("\"{clean}\"*"))
        })
        .collect();
    if tokens.is_empty() {
        return Vec::new();
    }
    let fts_query = tokens.join(" ");
    let Ok(mut stmt) = conn.prepare(
        "SELECT p.name, p.body, p.kind, p.region, p.lat, p.lon, p.elevation_m, p.population
         FROM places_fts f
         JOIN places p ON p.id = f.rowid
         WHERE places_fts MATCH ?1
         ORDER BY p.importance DESC, p.population DESC
         LIMIT ?2",
    ) else {
        return Vec::new();
    };
    stmt.query_map(params![fts_query, limit as i64], map_hit_row)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .collect()
}

fn map_hit_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GazetteerHit> {
    let body_str: String = row.get(1)?;
    let body = match body_str.to_ascii_lowercase().as_str() {
        "moon" => ActiveBody::Moon,
        "mars" => ActiveBody::Mars,
        _ => ActiveBody::Earth,
    };
    Ok(GazetteerHit {
        name: row.get(0)?,
        body,
        kind: row.get(2)?,
        region: row.get(3)?,
        location: GeoPoint {
            lat: row.get::<_, f64>(4)? as f32,
            lon: row.get::<_, f64>(5)? as f32,
        },
        elevation_m: row.get::<_, Option<f64>>(6)?.map(|v| v as f32),
        population: row.get::<_, i64>(7).unwrap_or(0).max(0) as u32,
    })
}

fn search_fallback(query: &str, limit: usize) -> Vec<GazetteerHit> {
    let q = query.to_ascii_lowercase();
    let mut hits: Vec<GazetteerHit> = Vec::new();

    for &(name, body, kind, region, lat, lon, elev) in CORE_MULTI_WORLD {
        if q.is_empty()
            || name.to_ascii_lowercase().contains(&q)
            || region.to_ascii_lowercase().contains(&q)
            || kind.to_ascii_lowercase().contains(&q)
        {
            hits.push(GazetteerHit {
                name: name.into(),
                body,
                kind: kind.into(),
                region: region.into(),
                location: GeoPoint { lat, lon },
                elevation_m: elev,
                population: 0,
            });
        }
    }

    if hits.len() < limit {
        for city in city_catalog::search(query, limit - hits.len()) {
            hits.push(GazetteerHit {
                name: city.name.clone(),
                body: ActiveBody::Earth,
                kind: "city".into(),
                region: city.location_label(),
                location: city.location,
                elevation_m: None,
                population: city.population,
            });
        }
    }
    hits.truncate(limit);
    hits
}

const CORE_MULTI_WORLD: &[(&str, ActiveBody, &str, &str, f32, f32, Option<f32>)] = &[
    ("Mount Everest", ActiveBody::Earth, "peak", "Himalayas, Nepal/China", 27.9881, 86.9250, Some(8849.0)),
    ("K2", ActiveBody::Earth, "peak", "Karakoram, Pakistan/China", 35.8808, 76.5133, Some(8611.0)),
    ("Mount Fuji", ActiveBody::Earth, "volcano", "Honshu, Japan", 35.3606, 138.7274, Some(3776.0)),
    ("Kilauea", ActiveBody::Earth, "volcano", "Hawaii, USA", 19.4069, -155.2834, Some(1247.0)),
    ("Mount St. Helens", ActiveBody::Earth, "volcano", "Cascades, Washington, USA", 46.1914, -122.1956, Some(2549.0)),
    ("Mariana Trench", ActiveBody::Earth, "trench", "Western Pacific Ocean", 11.3733, 142.5917, Some(-10935.0)),
    ("Strait of Hormuz", ActiveBody::Earth, "strait", "Persian Gulf / Gulf of Oman", 26.5667, 56.2500, None),
    ("Tycho Crater", ActiveBody::Moon, "crater", "Southern Lunar Highlands", -43.31, -11.36, Some(-4800.0)),
    ("Copernicus Crater", ActiveBody::Moon, "crater", "Oceanus Procellarum", 9.62, -20.08, Some(-3800.0)),
    ("Mare Tranquillitatis", ActiveBody::Moon, "mare", "Lunar Near Side (Apollo 11)", 8.5, 31.4, Some(-1500.0)),
    ("Shackleton Crater", ActiveBody::Moon, "crater", "Lunar South Pole", -89.67, 129.78, Some(-4200.0)),
    ("South Pole-Aitken Basin", ActiveBody::Moon, "basin", "Lunar Far Side", -53.0, -169.0, Some(-8000.0)),
    ("Olympus Mons", ActiveBody::Mars, "volcano", "Tharsis Bulge", 18.65, -133.8, Some(21229.0)),
    ("Valles Marineris", ActiveBody::Mars, "canyon", "Equatorial Mars", -13.9, -59.2, Some(-7000.0)),
    ("Jezero Crater", ActiveBody::Mars, "crater", "Isidis Planitia (Perseverance)", 18.38, 77.58, Some(-2500.0)),
    ("Gale Crater", ActiveBody::Mars, "crater", "Aeolis Quadrangle (Curiosity)", -5.4, 137.8, Some(-4500.0)),
    ("Hellas Planitia", ActiveBody::Mars, "basin", "Southern Mars", -42.4, 70.5, Some(-7152.0)),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_decimal_and_hemisphere_coordinates() {
        let p = parse_coordinates("37.7749, -122.4194").unwrap();
        assert!((p.lat - 37.7749).abs() < 1e-4);
        assert!((p.lon + 122.4194).abs() < 1e-4);

        let p2 = parse_coordinates("35.68N 139.76E").unwrap();
        assert!((p2.lat - 35.68).abs() < 1e-3);
        assert!((p2.lon - 139.76).abs() < 1e-3);

        let p3 = parse_coordinates("33.86S 151.20W").unwrap();
        assert!(p3.lat < 0.0 && p3.lon < 0.0);
    }

    #[test]
    fn fallback_search_finds_moon_and_mars_landmarks() {
        let mars = search("Olympus", 5);
        assert!(mars.iter().any(|h| h.body == ActiveBody::Mars && h.name.contains("Olympus")));
        let moon = search("Shackleton", 5);
        assert!(moon.iter().any(|h| h.body == ActiveBody::Moon && h.name.contains("Shackleton")));
    }
}
