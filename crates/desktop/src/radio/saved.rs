//! Small user-managed stream list, separate from private API configuration.
use super::{RadioFeed, stream_url, valid_coordinates};
use std::io::Read;
use std::path::{Path, PathBuf};

pub(crate) fn path() -> Option<PathBuf> {
    crate::settings_store::executable_dir().map(|dir| dir.join(".1kee_radio_feeds.json"))
}

pub(crate) fn load(path: &Path) -> Result<Vec<RadioFeed>, String> {
    match std::fs::File::open(path) {
        Ok(file) => {
            let mut body = Vec::new();
            file.take(1024 * 1024 + 1)
                .read_to_end(&mut body)
                .map_err(|e| e.to_string())?;
            if body.len() > 1024 * 1024 {
                return Err("Saved radio feeds file is too large.".into());
            }
            let mut feeds: Vec<RadioFeed> =
                serde_json::from_slice(&body).map_err(|e| format!("Saved radio feeds: {e}"))?;
            feeds.retain(|feed| !feed.name.trim().is_empty() && stream_url(&feed.url).is_ok()
                && !feed.kinds.is_empty()
                && matches!((feed.latitude, feed.longitude), (Some(a), Some(b)) if valid_coordinates(a, b)));
            feeds.truncate(200);
            Ok(feeds)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("Could not read saved radio feeds: {e}")),
    }
}

pub(crate) fn save(path: &Path, feeds: &[RadioFeed]) -> Result<(), String> {
    if feeds.len() > 200 {
        return Err("The saved feed limit is 200.".into());
    }
    let body = serde_json::to_vec_pretty(feeds).map_err(|e| e.to_string())?;
    if body.len() > 1024 * 1024 {
        return Err("The saved radio feed list is too large.".into());
    }
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, body)
        .and_then(|_| std::fs::rename(&temp, path))
        .map_err(|e| format!("Could not save radio feeds: {e}"))
}
