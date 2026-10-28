use crate::{camera_registry, factal_stream, model::AppModel, moving_tracks, settings_store};

#[derive(Default)]
pub(super) struct Actions {
    pub save: bool,
    pub poll: bool,
    pub clear_keys: bool,
}
impl Actions {
    pub fn apply(self, model: &mut AppModel) {
        let (save_requested, poll_requested, clear_requested) =
            (self.save, self.poll, self.clear_keys);
        // ── Post-render actions ────────────────────────────────────────────────
        if save_requested {
            let had_key = model.has_factal_api_key();
            model.factal_api_key = model.factal_api_key.trim().to_owned();
            model.ny511_api_key = model.ny511_api_key.trim().to_owned();
            model.windy_webcams_api_key = model.windy_webcams_api_key.trim().to_owned();
            model.eyes_on_country_code =
                settings_store::normalize_eyes_on_country_code(&model.eyes_on_country_code);
            model.eyes_on_requests_per_minute =
                settings_store::normalize_eyes_on_requests_per_minute(
                    model.eyes_on_requests_per_minute,
                );
            model.aisstream_api_key = model.aisstream_api_key.trim().to_owned();
            moving_tracks::invalidate();
            match model.save_settings() {
                Ok(()) => {
                    model.apply_saved_settings();
                    model.factal_stream_status = if model.has_factal_api_key() {
                        "configured".into()
                    } else {
                        "inactive".into()
                    };
                    if model.has_factal_api_key() {
                        model.push_log(
                        "Settings saved locally; path resolution and live polling will refresh automatically."
                            .into(),
                    );
                        if !had_key || model.has_factal_api_key() {
                            factal_stream::invalidate();
                        }
                    } else if had_key {
                        model.push_log("Factal API key cleared; stream is inactive.".into());
                    }
                    camera_registry::invalidate();
                }
                Err(error) => {
                    model.push_log(format!("Settings save failed: {}", error));
                }
            }
        }

        if poll_requested {
            if model.has_factal_api_key() {
                factal_stream::invalidate();
                model.factal_stream_status = "syncing".into();
                model.push_log("Factal live poll requested manually.".into());
            } else {
                model.push_log("Factal live poll skipped because no API key is configured.".into());
            }
            if model.has_enabled_camera_sources() {
                camera_registry::invalidate();
                model.camera_registry_status = "syncing".into();
                model.push_log("Camera registry poll requested manually.".into());
            } else {
                model.push_log("Camera poll skipped because no live source is configured.".into());
            }
        }

        if clear_requested {
            model.factal_api_key.clear();
            model.ny511_api_key.clear();
            model.windy_webcams_api_key.clear();
            model.aisstream_api_key.clear();
            moving_tracks::invalidate();
            match model.save_settings() {
                Ok(()) => {
                    model.factal_stream_status = "inactive".into();
                    model.camera_registry_status = if model.eyes_on_enabled {
                        "configured".into()
                    } else {
                        "inactive".into()
                    };
                    factal_stream::invalidate();
                    camera_registry::invalidate();
                    model.push_log("API keys cleared; keyed streams are inactive.".into());
                }
                Err(error) => {
                    model.push_log(format!("Settings clear failed: {}", error));
                }
            }
        }
    }
}

pub(super) fn request_camera_scan(model: &mut AppModel) {
    model.eyes_on_enabled = true;
    model.camera_registry_status = "syncing".into();
    model.camera_registry_scanning = true;
    model.camera_registry_progress = 0.0;
    model.camera_registry_progress_label = "Queued a full camera-directory scan…".into();

    match model.save_settings() {
        Ok(()) => model.push_log(format!(
            "Live public-camera discovery enabled; scanning the directory at {} requests/min…",
            model.eyes_on_requests_per_minute
        )),
        Err(error) => model.push_log(format!(
            "Live camera scan started for this run, but settings could not be saved: {error}"
        )),
    }
    camera_registry::invalidate();
}
