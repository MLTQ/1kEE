use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::path::Path;
use std::time::Duration;

pub mod contour_clip;
pub mod contour_grid;
pub mod contours;
pub mod gpkg;
pub mod vector;

pub const FILE_NAME: &str = "world.1ka";
const APPLICATION_ID: i32 = 0x314b4152;
pub const MAX_PAYLOAD: usize = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct Key {
    pub body: i32,
    pub layer: [u8; 4],
    pub grid: i32,
    pub level: i32,
    pub y: i32,
    pub x: i32,
}

fn checksum(bytes: &[u8]) -> i64 {
    i64::from(crc32fast::hash(bytes))
}

pub struct Reader {
    connection: Connection,
}

impl Reader {
    pub fn open(path: &Path) -> Result<Self, String> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| e.to_string())?;
        connection
            .busy_timeout(Duration::from_secs(2))
            .map_err(|e| e.to_string())?;
        let app: i32 = connection
            .pragma_query_value(None, "application_id", |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let version: i32 = connection
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if app != APPLICATION_ID || version != 1 {
            return Err("Unsupported tile archive format".into());
        }
        connection
            .pragma_update(None, "cache_size", -4096)
            .map_err(|e| e.to_string())?;
        Ok(Self { connection })
    }

    pub fn get(&self, key: Key) -> Result<Option<Vec<u8>>, String> {
        let mut stmt = self.connection.prepare_cached(
            "SELECT payload, checksum FROM tiles WHERE body=?1 AND layer=?2 AND grid=?3 AND level=?4 AND y=?5 AND x=?6"
        ).map_err(|e| e.to_string())?;
        let value = stmt
            .query_row(
                params![key.body, &key.layer[..], key.grid, key.level, key.y, key.x],
                |row| {
                    let bytes = row.get_ref(0)?.as_blob()?;
                    if bytes.len() > MAX_PAYLOAD || checksum(bytes) != row.get::<_, i64>(1)? {
                        return Err(rusqlite::Error::InvalidQuery);
                    }
                    Ok(bytes.to_vec())
                },
            )
            .optional()
            .map_err(|e| format!("Archive tile {key:?}: {e}"))?;
        Ok(value)
    }

    pub fn has_cell(&self, layer: [u8; 4], lat: i32, lon: i32) -> Result<bool, String> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM vector_cells WHERE layer=?1 AND lat=?2 AND lon=?3)",
                params![&layer[..], lat, lon],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())
    }

    pub fn metadata(&self, key: &str) -> Result<Option<String>, String> {
        self.connection
            .query_row("SELECT value FROM metadata WHERE key=?1", [key], |r| {
                r.get(0)
            })
            .optional()
            .map_err(|e| e.to_string())
    }
}

pub struct Writer {
    connection: Connection,
}

impl Writer {
    /// The caller owns a new staging file; never opens a published archive for mutation.
    pub fn create(path: &Path) -> Result<Self, String> {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        drop(file);
        let initialized = (|| -> Result<Connection, String> {
            let connection = Connection::open(path).map_err(|e| e.to_string())?;
            connection.execute_batch(&format!(
            "PRAGMA application_id={APPLICATION_ID}; PRAGMA user_version=1;
             PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;
             CREATE TABLE tiles(body INTEGER NOT NULL, layer BLOB NOT NULL, grid INTEGER NOT NULL,
                level INTEGER NOT NULL, y INTEGER NOT NULL, x INTEGER NOT NULL,
                payload BLOB NOT NULL, checksum INTEGER NOT NULL,
                PRIMARY KEY(body,layer,grid,level,y,x)) WITHOUT ROWID;
             CREATE TABLE vector_cells(layer BLOB NOT NULL, lat INTEGER NOT NULL, lon INTEGER NOT NULL,
                PRIMARY KEY(layer,lat,lon)) WITHOUT ROWID;
             CREATE TABLE metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;"
        )).map_err(|e| e.to_string())?;
            Ok(connection)
        })();
        match initialized {
            Ok(connection) => Ok(Self { connection }),
            Err(error) => {
                let _ = std::fs::remove_file(path);
                Err(error)
            }
        }
    }

    pub fn put_batch(
        &mut self,
        tiles: &[(Key, Vec<u8>)],
        cell: Option<([u8; 4], i32, i32)>,
    ) -> Result<(), String> {
        let tx = self.connection.transaction().map_err(|e| e.to_string())?;
        {
            let mut insert = tx
                .prepare_cached("INSERT OR REPLACE INTO tiles VALUES (?1,?2,?3,?4,?5,?6,?7,?8)")
                .map_err(|e| e.to_string())?;
            for (key, bytes) in tiles {
                if bytes.len() > MAX_PAYLOAD {
                    return Err("Archive tile exceeds 256 MiB; subdivide it before packing".into());
                }
                insert
                    .execute(params![
                        key.body,
                        &key.layer[..],
                        key.grid,
                        key.level,
                        key.y,
                        key.x,
                        bytes,
                        checksum(bytes)
                    ])
                    .map_err(|e| e.to_string())?;
            }
        }
        if let Some((layer, lat, lon)) = cell {
            tx.execute(
                "INSERT OR REPLACE INTO vector_cells VALUES (?1,?2,?3)",
                params![&layer[..], lat, lon],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }

    pub fn metadata(&self, key: &str, value: &str) -> Result<(), String> {
        self.connection
            .execute(
                "INSERT OR REPLACE INTO metadata VALUES (?1,?2)",
                params![key, value],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn finish(self) -> Result<(), String> {
        self.connection
            .execute_batch("PRAGMA optimize;")
            .map_err(|e| e.to_string())?;
        self.connection.close().map_err(|(_, e)| e.to_string())
    }
}
