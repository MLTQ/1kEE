use crate::{model::AppModel, moving_tracks, settings_store, theme};
#[path = "camera_endpoint_settings.rs"]
mod camera_endpoints;

pub(super) fn render(ui: &mut egui::Ui, model: &mut AppModel) {
    // ── Factal ────────────────────────────────────────────────────────────
    ui.heading("Factal");
    ui.colored_label(
        theme::text_muted(),
        "Sync all geolocated Factal events with severity 2+ from the past 24 hours. \
         Refreshes one minute after each completed sync; older events are available in Replay.",
    );
    ui.add_space(8.0);

    ui.label("API Key");
    ui.add_sized(
        [ui.available_width(), 30.0],
        egui::TextEdit::singleline(&mut model.factal_api_key)
            .password(true)
            .hint_text("Token ..."),
    );
    ui.add_space(4.0);
    ui.small("Stored locally in the executable directory settings file.");
    ui.small(format!("Stream status: {}", model.factal_stream_status));

    ui.add_space(14.0);
    ui.separator();
    ui.add_space(10.0);

    // ── Camera APIs ───────────────────────────────────────────────────────
    ui.heading("Camera APIs");
    ui.colored_label(
        theme::text_muted(),
        "Configure live camera-source adapters. 511NY is the first high-confidence \
         traffic-camera source; Windy Webcams adds broader regional webcam coverage \
         around the current focus.",
    );
    ui.add_space(8.0);

    ui.label("511NY API Key");
    ui.add_sized(
        [ui.available_width(), 30.0],
        egui::TextEdit::singleline(&mut model.ny511_api_key)
            .password(true)
            .hint_text("511NY developer key"),
    );
    ui.add_space(6.0);

    ui.label("Windy Webcams API Key");
    ui.add_sized(
        [ui.available_width(), 30.0],
        egui::TextEdit::singleline(&mut model.windy_webcams_api_key)
            .password(true)
            .hint_text("Windy Webcams API key"),
    );
    ui.add_space(4.0);
    ui.small(format!(
        "Camera registry status: {}",
        model.camera_registry_status
    ));
    ui.small(
        "Optional no-key sources can be declared in Data/camera_sources/public_sources.json \
         and Data/camera_sources/scrape_sources.json under the asset root.",
    );

    ui.add_space(10.0);
    ui.separator();
    ui.add_space(8.0);
    ui.heading("Project Eyes On Directory");
    ui.colored_label(
        theme::text_muted(),
        "Opt in to a paced Insecam directory sync adapted from Project Eyes On. \
         It reads public listing/detail metadata and briefly probes only the \
         directory-advertised public-IP feed URLs; broad search-engine scanning is excluded.",
    );
    ui.add_space(6.0);
    ui.checkbox(
        &mut model.eyes_on_enabled,
        "Enable public-directory camera discovery",
    );
    ui.horizontal(|ui| {
        ui.label("Country");
        ui.add_sized(
            [64.0, 26.0],
            egui::TextEdit::singleline(&mut model.eyes_on_country_code)
                .char_limit(2)
                .hint_text("ALL"),
        );
        ui.label("Requests/min");
        ui.add(
            egui::DragValue::new(&mut model.eyes_on_requests_per_minute).range(
                settings_store::MIN_EYES_ON_REQUESTS_PER_MINUTE
                    ..=settings_store::MAX_EYES_ON_REQUESTS_PER_MINUTE,
            ),
        );
    });
    ui.small(
        "Blank country means the global popularity listing. Scanning follows the directory's \
         advertised page count, with empty/repeated-page fallbacks. Request starts are paced \
         globally. Saved working endpoints are reused on startup and ordinary polls. Coordinates are \
         approximate source metadata.",
    );
    camera_endpoints::render(ui, model);

    ui.add_space(14.0);
    ui.separator();
    ui.add_space(10.0);

    // ── AISStream ─────────────────────────────────────────────────────────
    ui.heading("AISStream");
    ui.colored_label(
        theme::text_muted(),
        "Live AIS vessel positions via AISStream WebSocket. Free tier available at aisstream.io.",
    );
    ui.add_space(8.0);

    ui.label("API Key");
    ui.add_sized(
        [ui.available_width(), 30.0],
        egui::TextEdit::singleline(&mut model.aisstream_api_key)
            .password(true)
            .hint_text("AISStream API key"),
    );
    ui.add_space(4.0);
    ui.small(format!("Ship layer status: {}", moving_tracks::status()));
    ui.small("Enable the Ships layer in the Operations Globe to start polling.");
}
