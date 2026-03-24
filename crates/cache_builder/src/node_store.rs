use crate::util::GeoPoint;
use rusqlite::{Connection, params};
use std::fs;
use std::path::Path;

pub struct NodeStore {
    connection: Connection,
}

impl NodeStore {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let connection = Connection::open(path).map_err(|error| error.to_string())?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(|error| error.to_string())?;
        connection
            .pragma_update(None, "synchronous", "NORMAL")
            .map_err(|error| error.to_string())?;
        connection
            .pragma_update(None, "temp_store", "MEMORY")
            .map_err(|error| error.to_string())?;
        connection
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS candidate_nodes (
                    id INTEGER PRIMARY KEY,
                    lat REAL NOT NULL,
                    lon REAL NOT NULL
                );
                CREATE TABLE IF NOT EXISTS build_state (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );
                ",
            )
            .map_err(|error| error.to_string())?;
        Ok(Self { connection })
    }

    pub fn reset(&mut self) -> Result<(), String> {
        self.connection
            .execute_batch(
                "
                DELETE FROM candidate_nodes;
                DELETE FROM build_state;
                INSERT INTO build_state(key, value) VALUES ('complete', '0');
                ",
            )
            .map_err(|error| error.to_string())
    }

    pub fn is_complete(&self) -> Result<bool, String> {
        let value: Option<String> = self
            .connection
            .query_row(
                "SELECT value FROM build_state WHERE key = 'complete'",
                [],
                |row| row.get(0),
            )
            .ok();
        Ok(matches!(value.as_deref(), Some("1")))
    }

    pub fn mark_complete(&self) -> Result<(), String> {
        self.connection
            .execute(
                "INSERT INTO build_state(key, value) VALUES ('complete', '1')
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn insert_batch(&mut self, batch: &[(i64, GeoPoint)]) -> Result<(), String> {
        if batch.is_empty() {
            return Ok(());
        }
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| error.to_string())?;
        {
            let mut statement = transaction
                .prepare(
                    "INSERT INTO candidate_nodes(id, lat, lon) VALUES (?1, ?2, ?3)
                     ON CONFLICT(id) DO UPDATE SET lat = excluded.lat, lon = excluded.lon",
                )
                .map_err(|error| error.to_string())?;
            for (id, point) in batch {
                statement
                    .execute(params![id, point.lat, point.lon])
                    .map_err(|error| error.to_string())?;
            }
        }
        transaction.commit().map_err(|error| error.to_string())
    }

    pub fn count(&self) -> Result<usize, String> {
        let count: i64 = self
            .connection
            .query_row("SELECT COUNT(*) FROM candidate_nodes", [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        Ok(count.max(0) as usize)
    }

    pub fn points_for_refs(&self, refs: &[i64]) -> Result<Vec<GeoPoint>, String> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT lat, lon FROM candidate_nodes WHERE id = ?1")
            .map_err(|error| error.to_string())?;
        let mut points = Vec::with_capacity(refs.len());
        for node_id in refs {
            if let Ok(point) = statement.query_row([node_id], |row| {
                Ok(GeoPoint {
                    lat: row.get(0)?,
                    lon: row.get(1)?,
                })
            }) {
                points.push(point);
            }
        }
        Ok(points)
    }
}
