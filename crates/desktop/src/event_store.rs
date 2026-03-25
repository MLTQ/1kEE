/// Persistent SQLite store for Factal event history (up to 365 days).
///
/// Opened once at startup; all public functions are safe to call from any
/// thread — they acquire the global `Mutex<Connection>` internally.
use crate::model::{EventRecord, EventSeverity, FactalBrief, GeoPoint};
use rusqlite::{Connection, params};
use std::sync::{Mutex, OnceLock};

fn db() -> &'static Mutex<Option<Connection>> {
    static DB: OnceLock<Mutex<Option<Connection>>> = OnceLock::new();
    DB.get_or_init(|| Mutex::new(None))
}

fn with_conn<T, F: FnOnce(&mut Connection) -> rusqlite::Result<T>>(f: F) -> Result<T, String> {
    let mut guard = db().lock().map_err(|_| "event store lock poisoned".to_string())?;
    let conn = guard.as_mut().ok_or("event store not opened")?;
    f(conn).map_err(|e| e.to_string())
}

/// Open (or create) the event database.  Must be called once before any
/// other function.  Prunes records older than 365 days on open.
pub fn open() {
    let Some(path) = crate::settings_store::event_db_path() else {
        return;
    };
    let conn = match Connection::open(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("event_store: open failed: {e}");
            return;
        }
    };
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    let _ = conn.pragma_update(None, "synchronous", "NORMAL");
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS stored_events (
            factal_id      TEXT    PRIMARY KEY,
            title          TEXT    NOT NULL DEFAULT '',
            summary        TEXT    NOT NULL DEFAULT '',
            severity_value INTEGER NOT NULL DEFAULT 0,
            lat            REAL    NOT NULL,
            lon            REAL    NOT NULL,
            location_name  TEXT    NOT NULL DEFAULT '',
            occurred_unix  INTEGER NOT NULL,
            raw_json       TEXT    NOT NULL DEFAULT ''
        );
        CREATE INDEX IF NOT EXISTS idx_occurred ON stored_events(occurred_unix);",
    );
    // Prune records older than 365 days.
    let cutoff = now_unix() - 365 * 86_400;
    let _ = conn.execute(
        "DELETE FROM stored_events WHERE occurred_unix < ?1",
        params![cutoff],
    );
    *db().lock().unwrap() = Some(conn);
}

/// Insert or ignore a batch of `EventRecord`s (must carry a `FactalBrief`
/// with a parseable `occurred_at_raw` timestamp; records without one are
/// skipped).
pub fn upsert_events(events: &[EventRecord]) {
    let _ = with_conn(|conn| {
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO stored_events
                 (factal_id, title, summary, severity_value, lat, lon, location_name, occurred_unix, raw_json)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
                 ON CONFLICT(factal_id) DO NOTHING",
            )?;
            for event in events {
                let Some(brief) = &event.factal_brief else { continue };
                let Some(unix) = brief.occurred_at_raw.as_deref().and_then(parse_iso_to_unix)
                else {
                    continue;
                };
                stmt.execute(params![
                    brief.factal_id,
                    event.title,
                    event.summary,
                    brief.severity_value.unwrap_or(0),
                    event.location.lat as f64,
                    event.location.lon as f64,
                    event.location_name,
                    unix,
                    brief.raw_json_pretty,
                ])?;
            }
        }
        tx.commit()
    });
}

/// Load all events in `[from_unix, to_unix]`, sorted oldest-first.
pub fn load_events_in_range(from_unix: i64, to_unix: i64) -> Vec<(i64, EventRecord)> {
    with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT factal_id, title, summary, severity_value, lat, lon, location_name, occurred_unix
             FROM stored_events
             WHERE occurred_unix BETWEEN ?1 AND ?2
             ORDER BY occurred_unix ASC",
        )?;
        let rows = stmt.query_map(params![from_unix, to_unix], |row| {
            let factal_id: String = row.get(0)?;
            let title: String = row.get(1)?;
            let summary: String = row.get(2)?;
            let severity_value: i64 = row.get(3)?;
            let lat: f64 = row.get(4)?;
            let lon: f64 = row.get(5)?;
            let location_name: String = row.get(6)?;
            let occurred_unix: i64 = row.get(7)?;
            Ok((
                occurred_unix,
                EventRecord {
                    id: format!("hist-{factal_id}"),
                    title,
                    summary,
                    severity: severity_from_value(severity_value),
                    location_name,
                    location: GeoPoint {
                        lat: lat as f32,
                        lon: lon as f32,
                    },
                    source: "History".into(),
                    occurred_at: unix_to_date_str(occurred_unix),
                    factal_brief: Some(FactalBrief {
                        factal_id,
                        severity_value: Some(severity_value),
                        occurred_at_raw: None,
                        point_wkt: None,
                        vertical: None,
                        subvertical: None,
                        topics: Vec::new(),
                        content: None,
                        raw_json_pretty: String::new(),
                    }),
                },
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    })
    .unwrap_or_default()
}

/// Unix timestamp of the oldest stored event, or `None` if the store is empty.
pub fn oldest_event_unix() -> Option<i64> {
    with_conn(|conn| {
        conn.query_row(
            "SELECT MIN(occurred_unix) FROM stored_events",
            [],
            |r| r.get::<_, Option<i64>>(0),
        )
    })
    .ok()
    .flatten()
}

/// Unix timestamp of the newest stored event, or `None` if the store is empty.
pub fn newest_event_unix() -> Option<i64> {
    with_conn(|conn| {
        conn.query_row(
            "SELECT MAX(occurred_unix) FROM stored_events",
            [],
            |r| r.get::<_, Option<i64>>(0),
        )
    })
    .ok()
    .flatten()
}

/// Total number of stored events (for deciding whether to backfill).
pub fn event_count() -> usize {
    with_conn(|conn| {
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM stored_events", [], |r| r.get(0))?;
        Ok(n.max(0) as usize)
    })
    .unwrap_or(0)
}

// ── Time utilities ────────────────────────────────────────────────────────────

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Format a unix timestamp as `YYYY-MM-DD` for the Factal `date__gte` param.
pub fn unix_to_date_str(unix: i64) -> String {
    let mut remaining = unix / 86_400; // whole days since epoch
    let mut year = 1970i64;
    loop {
        let ylen = if is_leap(year) { 366 } else { 365 };
        if remaining < ylen {
            break;
        }
        remaining -= ylen;
        year += 1;
    }
    let mut month = 1i64;
    loop {
        let mlen = month_len(month, year);
        if remaining < mlen {
            break;
        }
        remaining -= mlen;
        month += 1;
    }
    format!("{year:04}-{month:02}-{:02}", remaining + 1)
}

/// Parse an ISO-8601 timestamp string to unix seconds.
pub fn parse_iso_to_unix(s: &str) -> Option<i64> {
    let s = s.trim();
    // Strip timezone: "...+HH:MM" suffix (but don't strip the date's '-')
    let s = if let Some(pos) = s[10..].rfind('+') {
        &s[..10 + pos]
    } else {
        s
    };
    let s = s.trim_end_matches('Z');

    let (date_str, time_str) = if let Some(p) = s.find('T').or_else(|| s.find(' ')) {
        (&s[..p], &s[p + 1..])
    } else {
        (s, "00:00:00")
    };

    let dp: Vec<&str> = date_str.splitn(3, '-').collect();
    let year: i64 = dp.first()?.parse().ok()?;
    let month: i64 = dp.get(1)?.parse().ok()?;
    let day: i64 = dp.get(2)?.parse().ok()?;

    let tp: Vec<&str> = time_str.splitn(3, ':').collect();
    let hour: i64 = tp.first().unwrap_or(&"0").parse().ok()?;
    let minute: i64 = tp.get(1).unwrap_or(&"0").parse().ok()?;
    let second: i64 = tp
        .get(2)
        .unwrap_or(&"0")
        .split('.')
        .next()
        .unwrap_or("0")
        .parse()
        .ok()?;

    let days = days_since_epoch(year, month, day);
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn days_since_epoch(year: i64, month: i64, day: i64) -> i64 {
    let mut days = 0i64;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }
    for m in 1..month {
        days += month_len(m, year);
    }
    days + day - 1
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn month_len(month: i64, year: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(year) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

fn severity_from_value(v: i64) -> EventSeverity {
    if v >= 4 {
        EventSeverity::Critical
    } else if v >= 2 {
        EventSeverity::Elevated
    } else {
        EventSeverity::Advisory
    }
}
