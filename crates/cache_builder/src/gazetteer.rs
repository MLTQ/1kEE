//! Offline multi-world SQLite FTS5 gazetteer builder (Earth, Moon, Mars).
//!
//! Produces `Derived/gazetteer.sqlite` with an FTS5 prefix index (`prefix='2 3'`)
//! combining a curated multi-world landmark core (peaks, volcanoes, trenches,
//! straits, lunar craters/maria, Martian canyons/volcanoes) with optional
//! GeoNames `populated_places.sqlite` and IAU / GEBCO CSV tables.

use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Command {
    pub output_db: PathBuf,
    pub geonames_db: Option<PathBuf>,
    pub lunar_csv: Option<PathBuf>,
    pub mars_csv: Option<PathBuf>,
    pub gebco_csv: Option<PathBuf>,
}

pub fn parse<I>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = String>,
{
    let mut iter = args.into_iter();
    let mut output_db = None;
    let mut geonames_db = None;
    let mut lunar_csv = None;
    let mut mars_csv = None;
    let mut gebco_csv = None;

    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--output" | "-o" => output_db = iter.next().map(PathBuf::from),
            "--geonames" => geonames_db = iter.next().map(PathBuf::from),
            "--lunar-csv" => lunar_csv = iter.next().map(PathBuf::from),
            "--mars-csv" => mars_csv = iter.next().map(PathBuf::from),
            "--gebco-csv" => gebco_csv = iter.next().map(PathBuf::from),
            other => return Err(format!("Unknown gazetteer flag '{other}'")),
        }
    }

    let output_db = output_db
        .ok_or_else(|| "gazetteer requires --output <path/to/gazetteer.sqlite>".to_owned())?;
    Ok(Command {
        output_db,
        geonames_db,
        lunar_csv,
        mars_csv,
        gebco_csv,
    })
}

pub fn run(cmd: &Command) -> Result<usize, String> {
    if let Some(parent) = cmd.output_db.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = cmd.output_db.with_extension("sqlite.tmp");
    let _ = std::fs::remove_file(&tmp);

    let mut conn = Connection::open(&tmp).map_err(|e| e.to_string())?;
    conn.execute_batch(
        "PRAGMA journal_mode = OFF;
         PRAGMA synchronous = OFF;
         PRAGMA temp_store = MEMORY;
         CREATE TABLE places (
             id INTEGER PRIMARY KEY,
             name TEXT NOT NULL,
             ascii_name TEXT NOT NULL,
             body TEXT NOT NULL,
             kind TEXT NOT NULL,
             region TEXT NOT NULL,
             lat REAL NOT NULL,
             lon REAL NOT NULL,
             elevation_m REAL,
             population INTEGER NOT NULL DEFAULT 0,
             importance REAL NOT NULL DEFAULT 0.0
         );
         CREATE VIRTUAL TABLE places_fts USING fts5(
             name,
             ascii_name,
             region,
             kind,
             content='places',
             content_rowid='id',
             prefix='2 3'
         );",
    )
    .map_err(|e| e.to_string())?;

    let mut total = 0usize;
    {
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let mut insert = tx
            .prepare(
                "INSERT INTO places (name, ascii_name, body, kind, region, lat, lon, elevation_m, population, importance)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )
            .map_err(|e| e.to_string())?;

        // 1. Always seed curated multi-world landmark core.
        for &(name, body, kind, region, lat, lon, elev, imp) in BUILTIN_LANDMARKS {
            insert
                .execute(params![name, name, body, kind, region, lat, lon, elev, 0_i64, imp])
                .map_err(|e| e.to_string())?;
            total += 1;
        }

        // 2. Import GeoNames populated_places.sqlite if provided.
        if let Some(geo_path) = &cmd.geonames_db
            && geo_path.exists()
        {
            let geo_conn = Connection::open(geo_path).map_err(|e| e.to_string())?;
            if let Ok(mut stmt) = geo_conn.prepare(
                "SELECT name, ascii_name, latitude, longitude, country_name, population FROM cities",
            ) && let Ok(rows) = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, f64>(3)?,
                    row.get::<_, String>(4).unwrap_or_default(),
                    row.get::<_, i64>(5).unwrap_or(0).max(0),
                ))
            }) {
                for row in rows.flatten() {
                    let (name, ascii, lat, lon, country, pop) = row;
                    let imp = ((pop as f64 + 1.0).log10() * 1.4).clamp(0.5, 10.0);
                    insert
                        .execute(params![
                            name, ascii, "earth", "city", country, lat, lon, Option::<f64>::None, pop, imp
                        ])
                        .map_err(|e| e.to_string())?;
                    total += 1;
                }
            }
        }

        // 3. Import optional IAU / GEBCO CSV tables (columns: name, lat, lon[, kind[, region]]).
        for (csv_opt, body, default_kind) in [
            (&cmd.lunar_csv, "moon", "crater"),
            (&cmd.mars_csv, "mars", "landmark"),
            (&cmd.gebco_csv, "earth", "undersea"),
        ] {
            if let Some(csv_path) = csv_opt
                && csv_path.exists()
            {
                total += import_csv_landmarks(csv_path, body, default_kind, &mut insert)?;
            }
        }

        drop(insert);
        tx.execute("INSERT INTO places_fts(places_fts) VALUES('rebuild')", [])
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
    }

    conn.execute_batch("PRAGMA journal_mode = WAL; ANALYZE;")
        .map_err(|e| e.to_string())?;
    drop(conn);
    std::fs::rename(&tmp, &cmd.output_db).map_err(|e| e.to_string())?;
    Ok(total)
}

fn import_csv_landmarks(
    path: &Path,
    body: &str,
    default_kind: &str,
    insert: &mut rusqlite::Statement<'_>,
) -> Result<usize, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut lines = text.lines();
    let Some(header_line) = lines.next() else {
        return Ok(0);
    };
    let headers: Vec<String> = parse_csv_row(header_line)
        .into_iter()
        .map(|h| h.trim().to_ascii_lowercase())
        .collect();
    let col_name = headers
        .iter()
        .position(|h| matches!(h.as_str(), "name" | "clean_name" | "feature_name"))
        .unwrap_or(0);
    let col_lat = headers
        .iter()
        .position(|h| matches!(h.as_str(), "lat" | "latitude" | "center_lat"))
        .unwrap_or(1);
    let col_lon = headers
        .iter()
        .position(|h| matches!(h.as_str(), "lon" | "lng" | "longitude" | "center_lon"))
        .unwrap_or(2);
    let col_kind = headers
        .iter()
        .position(|h| matches!(h.as_str(), "type" | "kind" | "feature_type"));

    let mut count = 0usize;
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let rec = parse_csv_row(line);
        let Some(name) = rec
            .get(col_name)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let Ok(lat) = rec
            .get(col_lat)
            .map(|s| s.trim())
            .unwrap_or("")
            .parse::<f64>()
        else {
            continue;
        };
        let Ok(mut lon) = rec
            .get(col_lon)
            .map(|s| s.trim())
            .unwrap_or("")
            .parse::<f64>()
        else {
            continue;
        };
        if lon > 180.0 {
            lon -= 360.0;
        }
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
            continue;
        }
        let kind = col_kind
            .and_then(|i| rec.get(i))
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or(default_kind);
        insert
            .execute(params![
                name, name, body, kind, body, lat, lon, Option::<f64>::None, 0_i64, 4.0_f64
            ])
            .map_err(|e| e.to_string())?;
        count += 1;
    }
    Ok(count)
}

fn parse_csv_row(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                if in_quotes && chars.peek() == Some(&'"') {
                    chars.next();
                    cur.push('"');
                } else {
                    in_quotes = !in_quotes;
                }
            }
            ',' if !in_quotes => {
                fields.push(std::mem::take(&mut cur));
            }
            other => cur.push(other),
        }
    }
    fields.push(cur);
    fields
}

/// Curated multi-world geographic & planetary landmarks baked into every gazetteer.
const BUILTIN_LANDMARKS: &[(&str, &str, &str, &str, f64, f64, Option<f64>, f64)] = &[
    // Earth iconic peaks, volcanoes, straits & ocean trenches
    ("Mount Everest", "earth", "peak", "Himalayas, Nepal/China", 27.9881, 86.9250, Some(8848.86), 9.8),
    ("K2", "earth", "peak", "Karakoram, Pakistan/China", 35.8808, 76.5133, Some(8611.0), 9.4),
    ("Denali", "earth", "peak", "Alaska Range, USA", 63.0695, -151.0074, Some(6190.0), 9.2),
    ("Mont Blanc", "earth", "peak", "Alps, France/Italy", 45.8326, 6.8652, Some(4808.0), 9.0),
    ("Matterhorn", "earth", "peak", "Pennine Alps, Switzerland/Italy", 45.9763, 7.6586, Some(4478.0), 9.0),
    ("Mount Fuji", "earth", "volcano", "Honshu, Japan", 35.3606, 138.7274, Some(3776.0), 9.4),
    ("Kilauea", "earth", "volcano", "Hawaii, USA", 19.4069, -155.2834, Some(1247.0), 9.0),
    ("Mount St. Helens", "earth", "volcano", "Cascades, Washington, USA", 46.1914, -122.1956, Some(2549.0), 8.9),
    ("Mount Etna", "earth", "volcano", "Sicily, Italy", 37.7510, 14.9934, Some(3357.0), 8.9),
    ("Mariana Trench (Challenger Deep)", "earth", "trench", "Western Pacific Ocean", 11.3733, 142.5917, Some(-10935.0), 9.6),
    ("Puerto Rico Trench", "earth", "trench", "Atlantic Ocean", 19.7000, -67.3000, Some(-8376.0), 8.5),
    ("Strait of Hormuz", "earth", "strait", "Persian Gulf / Gulf of Oman", 26.5667, 56.2500, Some(0.0), 9.2),
    ("Bab el-Mandeb", "earth", "strait", "Red Sea / Gulf of Aden", 12.5833, 43.3333, Some(0.0), 9.0),
    ("Strait of Malacca", "earth", "strait", "Indian Ocean / South China Sea", 2.5000, 101.2000, Some(0.0), 9.0),
    ("Suez Canal", "earth", "canal", "Egypt", 30.5852, 32.2654, Some(0.0), 9.1),
    ("Panama Canal", "earth", "canal", "Panama", 9.0800, -79.6800, Some(0.0), 9.1),
    ("Grand Canyon", "earth", "canyon", "Arizona, USA", 36.1069, -112.1129, Some(2100.0), 9.0),
    // Moon (IAU nomenclature)
    ("Tycho Crater", "moon", "crater", "Southern Lunar Highlands", -43.31, -11.36, Some(-4800.0), 9.5),
    ("Copernicus Crater", "moon", "crater", "Eastern Oceanus Procellarum", 9.62, -20.08, Some(-3800.0), 9.4),
    ("Mare Tranquillitatis", "moon", "mare", "Lunar Near Side (Apollo 11)", 8.5, 31.4, Some(-1500.0), 9.6),
    ("Tranquility Base (Apollo 11)", "moon", "landing site", "Mare Tranquillitatis", 0.6741, 23.4730, Some(-1800.0), 9.7),
    ("Clavius Crater", "moon", "crater", "Southern Lunar Highlands", -58.4, -14.4, Some(-3500.0), 9.1),
    ("Shackleton Crater", "moon", "crater", "Lunar South Pole", -89.67, 129.78, Some(-4200.0), 9.5),
    ("South Pole-Aitken Basin", "moon", "basin", "Lunar Far Side", -53.0, -169.0, Some(-8000.0), 9.3),
    ("Aristarchus Plateau", "moon", "crater", "Oceanus Procellarum", 23.73, -47.49, Some(-3700.0), 9.0),
    ("Mons Huygens", "moon", "mountain", "Montes Apenninus", 19.92, -2.86, Some(5500.0), 9.1),
    ("Plato Crater", "moon", "crater", "Mare Imbrium", 51.62, -9.38, Some(-2000.0), 9.0),
    // Mars (IAU nomenclature)
    ("Olympus Mons", "mars", "volcano", "Tharsis Bulge", 18.65, -133.8, Some(21229.0), 9.8),
    ("Valles Marineris", "mars", "canyon", "Equatorial Mars", -13.9, -59.2, Some(-7000.0), 9.7),
    ("Jezero Crater (Perseverance)", "mars", "crater", "Isidis Planitia", 18.38, 77.58, Some(-2500.0), 9.6),
    ("Gale Crater (Curiosity)", "mars", "crater", "Aeolis Quadrangle", -5.4, 137.8, Some(-4500.0), 9.5),
    ("Hellas Planitia", "mars", "basin", "Southern Hemisphere", -42.4, 70.5, Some(-7152.0), 9.4),
    ("Ascraeus Mons", "mars", "volcano", "Tharsis Montes", 11.92, -104.08, Some(18225.0), 9.0),
    ("Pavonis Mons", "mars", "volcano", "Tharsis Montes", 1.48, -112.96, Some(14058.0), 8.9),
    ("Arsia Mons", "mars", "volcano", "Tharsis Montes", -8.26, -120.09, Some(17761.0), 8.9),
    ("Korolev Crater", "mars", "crater", "Planum Boreum", 72.77, -164.58, Some(-2000.0), 9.0),
    ("Utopia Planitia", "mars", "plain", "Northern Lowlands", 46.7, 117.5, Some(-4000.0), 8.9),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_fts5_gazetteer_with_prefix_search_across_earth_moon_and_mars() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("gazetteer.sqlite");
        let cmd = Command {
            output_db: out.clone(),
            geonames_db: None,
            lunar_csv: None,
            mars_csv: None,
            gebco_csv: None,
        };
        let count = run(&cmd).unwrap();
        assert!(count >= 30);

        let conn = Connection::open(&out).unwrap();
        let body: String = conn
            .query_row(
                "SELECT p.body FROM places_fts f JOIN places p ON p.id = f.rowid
                 WHERE places_fts MATCH 'olymp*' LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(body, "mars");

        let moon_name: String = conn
            .query_row(
                "SELECT p.name FROM places_fts f JOIN places p ON p.id = f.rowid
                 WHERE places_fts MATCH 'shack*' LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(moon_name.contains("Shackleton"));
    }
}
