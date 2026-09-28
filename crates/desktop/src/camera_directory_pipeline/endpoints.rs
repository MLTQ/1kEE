//! Durable verified directory endpoints; network refreshes are explicit.
use super::{CameraConnectionState, CameraFeed, GeoPoint, validated_public_feed_url};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Cached,
    Discover,
    Check,
}

#[derive(Clone, Debug)]
pub struct Endpoint {
    pub camera: CameraFeed,
    pub verified_at: u64,
    pub checked_at: u64,
    pub reachable: bool,
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub endpoints: Vec<Endpoint>,
    pub available: bool,
    pub scanned_at: Option<u64>,
    pub pending: Option<Action>,
}

pub(super) struct Plan {
    pub action: Action,
    pub revision: i64,
    pub snapshot: Snapshot,
}

pub struct Store {
    connection: Mutex<Connection>,
}

impl Store {
    pub fn open_default() -> Result<Self, String> {
        let path = crate::settings_store::event_db_path()
            .ok_or("Cannot locate the camera cache folder")?
            .with_file_name(".1kee_camera_endpoints.sqlite");
        Self::open(&path)
    }

    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let connection = Connection::open(path).map_err(|e| e.to_string())?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS camera_scopes (
               scope TEXT PRIMARY KEY, revision INTEGER NOT NULL DEFAULT 0,
               available INTEGER NOT NULL DEFAULT 0, scanned_at INTEGER,
               pending TEXT CHECK(pending IN ('discover','check')));
             CREATE TABLE IF NOT EXISTS camera_endpoints (
               scope TEXT NOT NULL, url TEXT NOT NULL, id TEXT NOT NULL,
               label TEXT NOT NULL, kind TEXT NOT NULL, lat REAL NOT NULL, lon REAL NOT NULL,
               verified_at INTEGER NOT NULL, checked_at INTEGER NOT NULL,
               reachable INTEGER NOT NULL, PRIMARY KEY(scope,url));",
            )
            .map_err(|e| e.to_string())?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn snapshot(&self, scope: &str) -> rusqlite::Result<Snapshot> {
        Self::read(&self.connection.lock().unwrap(), scope)
    }

    fn read(db: &Connection, scope: &str) -> rusqlite::Result<Snapshot> {
        let state = db
            .query_row(
                "SELECT available,scanned_at,pending FROM camera_scopes WHERE scope=?1",
                [scope],
                |r| {
                    Ok((
                        r.get::<_, bool>(0)?,
                        r.get::<_, Option<u64>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((available, scanned_at, pending)) = state else {
            return Ok(Snapshot::default());
        };
        let mut query = db.prepare("SELECT id,label,kind,lat,lon,url,verified_at,checked_at,reachable FROM camera_endpoints WHERE scope=?1 ORDER BY label,url")?;
        let endpoints = query
            .query_map([scope], |row| {
                let reachable = row.get(8)?;
                let verified_at: u64 = row.get(6)?;
                Ok(Endpoint {
                    camera: CameraFeed {
                        id: row.get(0)?,
                        label: row.get(1)?,
                        kind: row.get(2)?,
                        location: GeoPoint {
                            lat: row.get(3)?,
                            lon: row.get(4)?,
                        },
                        stream_url: row.get(5)?,
                        provider: "Project Eyes On · Insecam".into(),
                        last_seen: format!(
                            "saved endpoint · verified {} ago",
                            age_label(verified_at)
                        ),
                        // Saved success is historical, not a new live reachability check.
                        status: if reachable {
                            CameraConnectionState::Idle
                        } else {
                            CameraConnectionState::Unreachable
                        },
                    },
                    verified_at,
                    checked_at: row.get(7)?,
                    reachable,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .filter(|entry| valid(&entry.camera))
            .collect();
        Ok(Snapshot {
            endpoints,
            available,
            scanned_at,
            pending: match pending.as_deref() {
                Some("discover") => Some(Action::Discover),
                Some("check") => Some(Action::Check),
                _ => None,
            },
        })
    }

    pub(super) fn plan(&self, scope: &str) -> rusqlite::Result<Plan> {
        let mut db = self.connection.lock().unwrap();
        let tx = db.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO camera_scopes(scope) VALUES(?1)",
            [scope],
        )?;
        let snapshot = Self::read(&tx, scope)?;
        let revision = tx.query_row(
            "SELECT revision FROM camera_scopes WHERE scope=?1",
            [scope],
            |r| r.get(0),
        )?;
        tx.commit()?;
        Ok(Plan {
            action: snapshot.pending.unwrap_or(if snapshot.available {
                Action::Cached
            } else {
                Action::Discover
            }),
            revision,
            snapshot,
        })
    }

    /// Only verified feeds enter the cache. Later failures update an existing
    /// entry so the settings list can selectively remove it.
    pub(super) fn record(
        &self,
        scope: &str,
        revision: i64,
        camera: &CameraFeed,
    ) -> rusqlite::Result<bool> {
        if !valid(camera) {
            return Ok(false);
        }
        let mut db = self.connection.lock().unwrap();
        let tx = db.transaction()?;
        let current: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM camera_scopes WHERE scope=?1 AND revision=?2)",
            params![scope, revision],
            |r| r.get(0),
        )?;
        if !current {
            return Ok(false);
        }
        let now = now();
        if camera.status == CameraConnectionState::Reachable {
            tx.execute(
                "DELETE FROM camera_endpoints WHERE scope=?1 AND id=?2 AND url<>?3",
                params![scope, camera.id, camera.stream_url],
            )?;
            tx.execute("INSERT INTO camera_endpoints(scope,url,id,label,kind,lat,lon,verified_at,checked_at,reachable)
                        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?8,1)
                        ON CONFLICT(scope,url) DO UPDATE SET id=excluded.id,label=excluded.label,kind=excluded.kind,
                        lat=excluded.lat,lon=excluded.lon,verified_at=excluded.verified_at,checked_at=excluded.checked_at,reachable=1",
                params![scope,camera.stream_url,camera.id,camera.label,camera.kind,camera.location.lat,camera.location.lon,now])?;
            tx.execute(
                "UPDATE camera_scopes SET available=1 WHERE scope=?1",
                [scope],
            )?;
        } else {
            tx.execute(
                "UPDATE camera_endpoints SET checked_at=?3,reachable=0 WHERE scope=?1 AND url=?2",
                params![scope, camera.stream_url, now],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }

    pub(super) fn complete(
        &self,
        scope: &str,
        revision: i64,
        action: Action,
    ) -> rusqlite::Result<bool> {
        let db = self.connection.lock().unwrap();
        Ok(db.execute(
            "UPDATE camera_scopes SET available=1,pending=NULL,
            scanned_at=CASE WHEN ?3 THEN ?4 ELSE scanned_at END WHERE scope=?1 AND revision=?2",
            params![scope, revision, action == Action::Discover, now()],
        )? == 1)
    }

    pub fn request(&self, scope: &str, action: Action) -> rusqlite::Result<()> {
        let pending = match action {
            Action::Discover => "discover",
            Action::Check => "check",
            Action::Cached => return Ok(()),
        };
        self.connection.lock().unwrap().execute(
            "INSERT INTO camera_scopes(scope,revision,pending) VALUES(?1,1,?2)
            ON CONFLICT(scope) DO UPDATE SET revision=revision+1,pending=excluded.pending",
            params![scope, pending],
        )?;
        Ok(())
    }

    /// Removal is durable even for the last entry. Revision checks prevent an
    /// already-running scan from restoring deleted endpoints after this commit.
    pub fn remove(&self, scope: &str, url: Option<&str>) -> rusqlite::Result<Vec<String>> {
        let mut db = self.connection.lock().unwrap();
        let tx = db.transaction()?;
        tx.execute(
            "INSERT INTO camera_scopes(scope,revision,available) VALUES(?1,1,1)
            ON CONFLICT(scope) DO UPDATE SET revision=revision+1,available=1,pending=NULL",
            [scope],
        )?;
        let mut query = tx.prepare("SELECT url FROM camera_endpoints WHERE scope=?1 AND ((?2 IS NOT NULL AND url=?2) OR (?2 IS NULL AND reachable=0))")?;
        let removed = query
            .query_map(params![scope, url], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        drop(query);
        tx.execute("DELETE FROM camera_endpoints WHERE scope=?1 AND ((?2 IS NOT NULL AND url=?2) OR (?2 IS NULL AND reachable=0))", params![scope,url])?;
        tx.commit()?;
        Ok(removed)
    }
}

fn valid(camera: &CameraFeed) -> bool {
    camera.id.starts_with("eyes-on-")
        && validated_public_feed_url(&camera.stream_url).is_some()
        && camera.location.lat.is_finite()
        && camera.location.lon.is_finite()
        && (-90.0..=90.0).contains(&camera.location.lat)
        && (-180.0..=180.0).contains(&camera.location.lon)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn age_label(timestamp: u64) -> String {
    let seconds = now().saturating_sub(timestamp);
    if seconds < 60 {
        "less than a minute".into()
    } else if seconds < 3600 {
        format!("{} min", seconds / 60)
    } else if seconds < 86400 {
        format!("{} h", seconds / 3600)
    } else {
        format!("{} days", seconds / 86400)
    }
}

#[cfg(test)]
#[path = "endpoints_tests.rs"]
mod tests;
