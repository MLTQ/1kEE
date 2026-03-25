use crate::util::GeoPoint;
use rusqlite::{Connection, params};
use std::collections::HashMap;
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

    /// Persist a byte-offset checkpoint for a named scan pass ("node_scan" or "way_scan").
    /// The offset is the file position of the *next* blob to process, so resuming from it
    /// is safe even if the process was killed mid-blob.
    pub fn save_scan_offset(&self, pass: &str, offset: u64) -> Result<(), String> {
        self.connection
            .execute(
                "INSERT INTO build_state(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![format!("scan_offset_{pass}"), offset.to_string()],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Retrieve a previously saved scan offset, or `None` if no checkpoint exists.
    pub fn get_scan_offset(&self, pass: &str) -> Result<Option<u64>, String> {
        let value: Option<String> = self
            .connection
            .query_row(
                "SELECT value FROM build_state WHERE key = ?1",
                params![format!("scan_offset_{pass}")],
                |row| row.get(0),
            )
            .ok();
        Ok(value.and_then(|v| v.parse().ok()))
    }

    /// Remove a scan checkpoint once the pass has completed successfully.
    pub fn clear_scan_offset(&self, pass: &str) -> Result<(), String> {
        self.connection
            .execute(
                "DELETE FROM build_state WHERE key = ?1",
                params![format!("scan_offset_{pass}")],
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

    /// Look up coordinates for a batch of node IDs.  Uses a single SQL `IN (…)` per
    /// chunk of 999 IDs (SQLite's default variable limit) instead of one round-trip per
    /// node, giving a 10-50× speedup for typical ways.
    pub fn points_for_refs(&self, refs: &[i64]) -> Result<Vec<GeoPoint>, String> {
        if refs.is_empty() {
            return Ok(Vec::new());
        }

        // Collect results keyed by node ID so we can reconstruct in original order.
        let mut id_to_point: HashMap<i64, GeoPoint> = HashMap::with_capacity(refs.len());

        for chunk in refs.chunks(999) {
            let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT id, lat, lon FROM candidate_nodes WHERE id IN ({placeholders})"
            );
            let mut stmt = self
                .connection
                .prepare(&sql)
                .map_err(|error| error.to_string())?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        GeoPoint {
                            lat: row.get(1)?,
                            lon: row.get(2)?,
                        },
                    ))
                })
                .map_err(|error| error.to_string())?;
            for row in rows {
                let (id, point) = row.map_err(|error| error.to_string())?;
                id_to_point.insert(id, point);
            }
        }

        // Return points in the same order as `refs`, skipping any missing nodes.
        Ok(refs
            .iter()
            .filter_map(|id| id_to_point.get(id).copied())
            .collect())
    }
}
