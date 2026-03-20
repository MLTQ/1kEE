use crate::model::GeoPoint;
use crate::terrain_assets;
use rusqlite::{Connection, params};
use std::path::PathBuf;

#[derive(Clone)]
pub struct CityEntry {
    pub id: String,
    pub name: String,
    pub region: Option<String>,
    pub country: String,
    #[allow(dead_code)]
    pub ascii_name: String,
    #[allow(dead_code)]
    pub country_code: String,
    #[allow(dead_code)]
    pub admin1_code: String,
    pub location: GeoPoint,
    pub population: u32,
    #[allow(dead_code)]
    pub aliases: String,
}

impl CityEntry {
    pub fn location_label(&self) -> String {
        match &self.region {
            Some(region) => format!("{}, {}, {}", self.name, region, self.country),
            None => format!("{}, {}", self.name, self.country),
        }
    }
}

pub fn by_id(id: &str) -> Option<CityEntry> {
    let path = catalog_db_path()?;
    let connection = Connection::open(path).ok()?;
    let mut statement = connection
        .prepare(
            "SELECT geoname_id, name, ascii_name, latitude, longitude, country_code, country_name, admin1_code, population, alternate_names
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
                country_code: row.get::<_, String>(5).unwrap_or_default(),
                country: row.get(6)?,
                admin1_code: row.get::<_, String>(7).unwrap_or_default(),
                region: region_name(
                    row.get::<_, String>(5).unwrap_or_default().as_str(),
                    row.get::<_, String>(7).unwrap_or_default().as_str(),
                ),
                location: GeoPoint {
                    lat: row.get::<_, f64>(3)? as f32,
                    lon: row.get::<_, f64>(4)? as f32,
                },
                population: row.get::<_, i64>(8).unwrap_or_default().max(0) as u32,
                aliases: row.get::<_, String>(9).unwrap_or_default(),
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
        "SELECT geoname_id, name, ascii_name, latitude, longitude, country_code, country_name, admin1_code, population, alternate_names
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
        "SELECT geoname_id, name, ascii_name, latitude, longitude, country_code, country_name, admin1_code, population, alternate_names
         FROM cities
         ORDER BY population DESC, ascii_name ASC
         LIMIT ?1",
    )?;

    let rows = statement.query_map(params![limit as i64], map_city_row)?;
    Ok(rows.filter_map(Result::ok).collect())
}

fn map_city_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CityEntry> {
    let country_code = row.get::<_, String>(5).unwrap_or_default();
    let admin1_code = row.get::<_, String>(7).unwrap_or_default();

    Ok(CityEntry {
        id: row.get::<_, i64>(0)?.to_string(),
        name: row.get(1)?,
        ascii_name: row.get(2)?,
        region: region_name(country_code.as_str(), admin1_code.as_str()),
        country_code,
        admin1_code,
        location: GeoPoint {
            lat: row.get::<_, f64>(3)? as f32,
            lon: row.get::<_, f64>(4)? as f32,
        },
        country: row.get(6)?,
        population: row.get::<_, i64>(8).unwrap_or_default().max(0) as u32,
        aliases: row.get::<_, String>(9).unwrap_or_default(),
    })
}

fn catalog_db_path() -> Option<PathBuf> {
    let derived_root = terrain_assets::find_derived_root(None)?;
    let path = derived_root.join("geonames/populated_places.sqlite");
    path.exists().then_some(path)
}

fn region_name(country_code: &str, admin1_code: &str) -> Option<String> {
    let admin1_code = admin1_code.trim();
    if admin1_code.is_empty() {
        return None;
    }

    let region = match (country_code.trim(), admin1_code) {
        ("US", "AL") => "Alabama",
        ("US", "AK") => "Alaska",
        ("US", "AZ") => "Arizona",
        ("US", "AR") => "Arkansas",
        ("US", "CA") => "California",
        ("US", "CO") => "Colorado",
        ("US", "CT") => "Connecticut",
        ("US", "DE") => "Delaware",
        ("US", "FL") => "Florida",
        ("US", "GA") => "Georgia",
        ("US", "HI") => "Hawaii",
        ("US", "ID") => "Idaho",
        ("US", "IL") => "Illinois",
        ("US", "IN") => "Indiana",
        ("US", "IA") => "Iowa",
        ("US", "KS") => "Kansas",
        ("US", "KY") => "Kentucky",
        ("US", "LA") => "Louisiana",
        ("US", "ME") => "Maine",
        ("US", "MD") => "Maryland",
        ("US", "MA") => "Massachusetts",
        ("US", "MI") => "Michigan",
        ("US", "MN") => "Minnesota",
        ("US", "MS") => "Mississippi",
        ("US", "MO") => "Missouri",
        ("US", "MT") => "Montana",
        ("US", "NE") => "Nebraska",
        ("US", "NV") => "Nevada",
        ("US", "NH") => "New Hampshire",
        ("US", "NJ") => "New Jersey",
        ("US", "NM") => "New Mexico",
        ("US", "NY") => "New York",
        ("US", "NC") => "North Carolina",
        ("US", "ND") => "North Dakota",
        ("US", "OH") => "Ohio",
        ("US", "OK") => "Oklahoma",
        ("US", "OR") => "Oregon",
        ("US", "PA") => "Pennsylvania",
        ("US", "RI") => "Rhode Island",
        ("US", "SC") => "South Carolina",
        ("US", "SD") => "South Dakota",
        ("US", "TN") => "Tennessee",
        ("US", "TX") => "Texas",
        ("US", "UT") => "Utah",
        ("US", "VT") => "Vermont",
        ("US", "VA") => "Virginia",
        ("US", "WA") => "Washington",
        ("US", "WV") => "West Virginia",
        ("US", "WI") => "Wisconsin",
        ("US", "WY") => "Wyoming",
        ("US", "DC") => "District of Columbia",
        ("CA", "01") => "Alberta",
        ("CA", "02") => "British Columbia",
        ("CA", "03") => "Manitoba",
        ("CA", "04") => "New Brunswick",
        ("CA", "05") => "Newfoundland and Labrador",
        ("CA", "07") => "Nova Scotia",
        ("CA", "08") => "Ontario",
        ("CA", "09") => "Prince Edward Island",
        ("CA", "10") => "Quebec",
        ("CA", "11") => "Saskatchewan",
        ("CA", "12") => "Yukon",
        ("CA", "13") => "Northwest Territories",
        ("CA", "14") => "Nunavut",
        ("AU", "01") => "New South Wales",
        ("AU", "02") => "Queensland",
        ("AU", "03") => "South Australia",
        ("AU", "04") => "Tasmania",
        ("AU", "05") => "Victoria",
        ("AU", "06") => "Western Australia",
        ("AU", "07") => "Australian Capital Territory",
        ("AU", "08") => "Northern Territory",
        _ => return Some(admin1_code.to_string()),
    };

    Some(region.to_string())
}
