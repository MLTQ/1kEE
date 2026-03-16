use crate::model::GeoPoint;
use crate::terrain_assets;
use rusqlite::{Connection, params};
use std::path::PathBuf;

#[derive(Clone)]
pub struct CityEntry {
    pub id: String,
    pub name: String,
    pub country: String,
    pub ascii_name: String,
    pub location: GeoPoint,
    pub population: u32,
    pub aliases: String,
}

pub fn by_id(id: &str) -> Option<CityEntry> {
    let path = catalog_db_path()?;
    let connection = Connection::open(path).ok()?;
    let mut statement = connection
        .prepare(
            "SELECT geoname_id, name, ascii_name, latitude, longitude, country_name, population, alternate_names
             FROM cities
             WHERE geoname_id = ?1",
        )
        .ok()?;

    statement
        .query_row(params![id], |row| {
            Ok(CityEntry {
                id: row.get::<_, i64>(0)?.to_string(),
                name: row.get(1)?,
                ascii_name: row.get(2)?,
                location: GeoPoint {
                    lat: row.get::<_, f64>(3)? as f32,
                    lon: row.get::<_, f64>(4)? as f32,
                },
                country: row.get(5)?,
                population: row.get::<_, i64>(6).unwrap_or_default().max(0) as u32,
                aliases: row.get::<_, String>(7).unwrap_or_default(),
            })
        })
        .ok()
}

pub fn search(query: &str, limit: usize) -> Vec<CityEntry> {
    let Some(path) = catalog_db_path() else {
        return Vec::new();
    };
    let Ok(connection) = Connection::open(path) else {
        return Vec::new();
    };

    if query.trim().is_empty() {
        return query_top_cities(&connection, limit).unwrap_or_default();
    }

    let normalized = query.trim().to_ascii_lowercase();
    let contains = format!("%{normalized}%");
    let prefix = format!("{normalized}%");
    let alt_contains = format!("%{query}%");
    let alt_prefix = format!("{query}%");

    let mut statement = match connection.prepare(
        "SELECT geoname_id, name, ascii_name, latitude, longitude, country_name, population, alternate_names
         FROM cities
         WHERE ascii_name LIKE ?1 COLLATE NOCASE
            OR name LIKE ?1 COLLATE NOCASE
            OR country_name LIKE ?1 COLLATE NOCASE
            OR alternate_names LIKE ?2
         ORDER BY
            CASE
              WHEN ascii_name LIKE ?3 COLLATE NOCASE OR name LIKE ?3 COLLATE NOCASE THEN 0
              WHEN country_name LIKE ?3 COLLATE NOCASE OR alternate_names LIKE ?4 THEN 1
              ELSE 2
            END,
            population DESC,
            ascii_name ASC
         LIMIT ?5",
    ) {
        Ok(statement) => statement,
        Err(_) => return Vec::new(),
    };

    let rows = match statement.query_map(
        params![contains, alt_contains, prefix, alt_prefix, limit as i64],
        map_city_row,
    ) {
        Ok(rows) => rows,
        Err(_) => return Vec::new(),
    };

    rows.filter_map(Result::ok).collect()
}

fn query_top_cities(connection: &Connection, limit: usize) -> rusqlite::Result<Vec<CityEntry>> {
    let mut statement = connection.prepare(
        "SELECT geoname_id, name, ascii_name, latitude, longitude, country_name, population, alternate_names
         FROM cities
         ORDER BY population DESC, ascii_name ASC
         LIMIT ?1",
    )?;

    let rows = statement.query_map(params![limit as i64], map_city_row)?;
    Ok(rows.filter_map(Result::ok).collect())
}

fn map_city_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CityEntry> {
    Ok(CityEntry {
        id: row.get::<_, i64>(0)?.to_string(),
        name: row.get(1)?,
        ascii_name: row.get(2)?,
        location: GeoPoint {
            lat: row.get::<_, f64>(3)? as f32,
            lon: row.get::<_, f64>(4)? as f32,
        },
        country: row.get(5)?,
        population: row.get::<_, i64>(6).unwrap_or_default().max(0) as u32,
        aliases: row.get::<_, String>(7).unwrap_or_default(),
    })
}

fn catalog_db_path() -> Option<PathBuf> {
    let derived_root = terrain_assets::find_derived_root(None)?;
    let path = derived_root.join("geonames/populated_places.sqlite");
    path.exists().then_some(path)
}
