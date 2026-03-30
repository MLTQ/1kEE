use crate::util::GeoPoint;
use rusqlite::{Connection, params};
use std::collections::{HashMap, HashSet};
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
                CREATE TABLE IF NOT EXISTS admin_relations (
                    relation_id INTEGER PRIMARY KEY,
                    name TEXT,
                    admin_level INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS admin_member_ways (
                    way_id INTEGER NOT NULL,
                    relation_id INTEGER NOT NULL,
                    PRIMARY KEY (way_id, relation_id)
                );
                CREATE TABLE IF NOT EXISTS admin_way_nodes (
                    way_id INTEGER NOT NULL,
                    seq INTEGER NOT NULL,
                    node_id INTEGER NOT NULL,
                    PRIMARY KEY (way_id, seq)
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

    // ── Admin boundary methods ─────────────────────────────────────────────────

    pub fn is_relation_scan_complete(&self) -> Result<bool, String> {
        let value: Option<String> = self
            .connection
            .query_row(
                "SELECT value FROM build_state WHERE key = 'relation_scan_complete'",
                [],
                |row| row.get(0),
            )
            .ok();
        Ok(matches!(value.as_deref(), Some("1")))
    }

    pub fn mark_relation_scan_complete(&self) -> Result<(), String> {
        self.connection
            .execute(
                "INSERT INTO build_state(key, value) VALUES ('relation_scan_complete', '1')
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn is_admin_way_scan_complete(&self) -> Result<bool, String> {
        let value: Option<String> = self
            .connection
            .query_row(
                "SELECT value FROM build_state WHERE key = 'admin_way_scan_complete'",
                [],
                |row| row.get(0),
            )
            .ok();
        Ok(matches!(value.as_deref(), Some("1")))
    }

    pub fn mark_admin_way_scan_complete(&self) -> Result<(), String> {
        self.connection
            .execute(
                "INSERT INTO build_state(key, value) VALUES ('admin_way_scan_complete', '1')
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Insert a batch of admin relations (and their member way IDs) into SQLite.
    /// Each tuple: (relation_id, name, admin_level, way_ids)
    pub fn save_admin_relations_batch(
        &mut self,
        relations: &[(i64, Option<String>, u8, Vec<i64>)],
    ) -> Result<(), String> {
        if relations.is_empty() {
            return Ok(());
        }
        let tx = self
            .connection
            .transaction()
            .map_err(|error| error.to_string())?;
        {
            let mut rel_stmt = tx
                .prepare(
                    "INSERT INTO admin_relations(relation_id, name, admin_level) VALUES (?1, ?2, ?3)
                     ON CONFLICT(relation_id) DO NOTHING",
                )
                .map_err(|error| error.to_string())?;
            let mut way_stmt = tx
                .prepare(
                    "INSERT INTO admin_member_ways(way_id, relation_id) VALUES (?1, ?2)
                     ON CONFLICT(way_id, relation_id) DO NOTHING",
                )
                .map_err(|error| error.to_string())?;
            for (relation_id, name, admin_level, way_ids) in relations {
                rel_stmt
                    .execute(params![relation_id, name, *admin_level as i64])
                    .map_err(|error| error.to_string())?;
                for way_id in way_ids {
                    way_stmt
                        .execute(params![way_id, relation_id])
                        .map_err(|error| error.to_string())?;
                }
            }
        }
        tx.commit().map_err(|error| error.to_string())
    }

    pub fn count_admin_relations(&self) -> Result<usize, String> {
        let count: i64 = self
            .connection
            .query_row("SELECT COUNT(*) FROM admin_relations", [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        Ok(count.max(0) as usize)
    }

    pub fn get_admin_member_way_ids(&self) -> Result<HashSet<i64>, String> {
        let mut stmt = self
            .connection
            .prepare("SELECT way_id FROM admin_member_ways")
            .map_err(|error| error.to_string())?;
        let rows = stmt
            .query_map([], |row| row.get::<_, i64>(0))
            .map_err(|error| error.to_string())?;
        let mut set = HashSet::new();
        for row in rows {
            set.insert(row.map_err(|error| error.to_string())?);
        }
        Ok(set)
    }

    /// Insert node refs for admin ways. Each tuple: (way_id, ordered_node_ids).
    pub fn save_admin_way_nodes(&mut self, batches: &[(i64, Vec<i64>)]) -> Result<(), String> {
        if batches.is_empty() {
            return Ok(());
        }
        let tx = self
            .connection
            .transaction()
            .map_err(|error| error.to_string())?;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO admin_way_nodes(way_id, seq, node_id) VALUES (?1, ?2, ?3)
                     ON CONFLICT(way_id, seq) DO NOTHING",
                )
                .map_err(|error| error.to_string())?;
            for (way_id, node_ids) in batches {
                for (seq, node_id) in node_ids.iter().enumerate() {
                    stmt.execute(params![way_id, seq as i64, node_id])
                        .map_err(|error| error.to_string())?;
                }
            }
        }
        tx.commit().map_err(|error| error.to_string())
    }

    /// Returns all (relation_id, name, admin_level, way_ids) rows.
    pub fn get_admin_relation_ways(
        &self,
    ) -> Result<Vec<(i64, Option<String>, u8, Vec<i64>)>, String> {
        // Load all relations first.
        let mut rel_stmt = self
            .connection
            .prepare(
                "SELECT relation_id, name, admin_level FROM admin_relations ORDER BY relation_id",
            )
            .map_err(|error| error.to_string())?;
        let relations: Vec<(i64, Option<String>, u8)> = rel_stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i64>(2)? as u8,
                ))
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|error: rusqlite::Error| error.to_string())?;

        // Load all member ways grouped by relation.
        let mut way_stmt = self
            .connection
            .prepare(
                "SELECT relation_id, way_id FROM admin_member_ways ORDER BY relation_id, way_id",
            )
            .map_err(|error| error.to_string())?;
        let mut ways_by_relation: HashMap<i64, Vec<i64>> = HashMap::new();
        let rows = way_stmt
            .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))
            .map_err(|error| error.to_string())?;
        for row in rows {
            let (relation_id, way_id) = row.map_err(|error| error.to_string())?;
            ways_by_relation
                .entry(relation_id)
                .or_default()
                .push(way_id);
        }

        Ok(relations
            .into_iter()
            .map(|(rid, name, level)| {
                let way_ids = ways_by_relation.remove(&rid).unwrap_or_default();
                (rid, name, level, way_ids)
            })
            .collect())
    }

    /// For each way_id in the slice, look up its ordered node refs from admin_way_nodes,
    /// then resolve coords from candidate_nodes. Returns only ways with >= 2 coords.
    pub fn get_way_coords_for_relation(
        &self,
        way_ids: &[i64],
    ) -> Result<HashMap<i64, Vec<GeoPoint>>, String> {
        let mut result: HashMap<i64, Vec<GeoPoint>> = HashMap::new();

        for &way_id in way_ids {
            // Step 1: get ordered node IDs for this way.
            let mut node_stmt = self
                .connection
                .prepare("SELECT node_id FROM admin_way_nodes WHERE way_id = ?1 ORDER BY seq")
                .map_err(|error| error.to_string())?;
            let node_ids: Vec<i64> = node_stmt
                .query_map(params![way_id], |row| row.get::<_, i64>(0))
                .map_err(|error| error.to_string())?
                .collect::<Result<_, _>>()
                .map_err(|error: rusqlite::Error| error.to_string())?;

            if node_ids.len() < 2 {
                continue;
            }

            // Step 2: batch-look up coords.
            let points = self.points_for_refs(&node_ids)?;
            if points.len() >= 2 {
                result.insert(way_id, points);
            }
        }

        Ok(result)
    }

    /// Load every candidate node into a `HashMap<id → GeoPoint>` for O(1) in-memory
    /// lookups during the way-scan pass.  For large bounding boxes this may use several
    /// hundred MiB of RAM but eliminates all per-way SQLite queries.
    pub fn load_all_nodes(&self) -> Result<HashMap<i64, GeoPoint>, String> {
        let count: i64 = self
            .connection
            .query_row("SELECT COUNT(*) FROM candidate_nodes", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        let mut map: HashMap<i64, GeoPoint> = HashMap::with_capacity(count as usize);
        let mut stmt = self
            .connection
            .prepare("SELECT id, lat, lon FROM candidate_nodes")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    GeoPoint {
                        lat: row.get(1)?,
                        lon: row.get(2)?,
                    },
                ))
            })
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (id, pt) = row.map_err(|e| e.to_string())?;
            map.insert(id, pt);
        }
        Ok(map)
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
            let sql =
                format!("SELECT id, lat, lon FROM candidate_nodes WHERE id IN ({placeholders})");
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
