// Successful archive results share the event-history database. The replay still
// works when the store is unavailable. Decompress outside its connection lock.
use crate::event_store::{now_unix, with_conn};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use rusqlite::params;
use std::io::{Read, Write};

pub(super) fn get(key: &str, revision: i64, max_age_s: i64) -> Option<Vec<u8>> {
    let bytes: Vec<u8> = with_conn(|db| {
        db.query_row(
            "SELECT data FROM station_replay_cache WHERE key=? AND revision=? AND stored_at>=?",
            params![key, revision, now_unix() - max_age_s],
            |row| row.get(0),
        )
    })
    .ok()?;
    let mut data = Vec::new();
    GzDecoder::new(bytes.as_slice())
        .read_to_end(&mut data)
        .ok()?;
    Some(data)
}

pub(super) fn put(key: &str, revision: i64, data: &[u8]) {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    if encoder.write_all(data).is_err() {
        return;
    }
    if let Ok(bytes) = encoder.finish() {
        let _ = with_conn(|db| {
            db.execute(
                "INSERT INTO station_replay_cache(key,revision,stored_at,data) VALUES(?,?,?,?)
                 ON CONFLICT(key) DO UPDATE SET revision=excluded.revision, stored_at=excluded.stored_at, data=excluded.data
                 WHERE excluded.revision >= station_replay_cache.revision",
                params![key, revision, now_unix(), bytes],
            )
        });
    }
}
