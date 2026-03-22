use crate::camera_registry;
use crate::factal_stream;
use crate::model::AppModel;
use crate::moving_tracks;
use crate::settings_store;
use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum SettingsTab {
    #[default]
    Theme,
    Apis,
    Paths,
}

pub fn render_factal_settings(ctx: &egui::Context, model: &mut AppModel) {
    if !model.factal_settings_open {
        return;
    }

    let mut open = model.factal_settings_open;
    let mut save_requested = false;
    let mut clear_requested = false;
    let mut poll_requested = false;

    egui::Window::new("Settings")
        .open(&mut open)
        .default_size(egui::vec2(620.0, 520.0))
        .min_size(egui::vec2(480.0, 320.0))
        .frame(
            egui::Frame::window(&ctx.style())
                .fill(theme::window_fill())
                .stroke(egui::Stroke::new(1.0, theme::window_stroke())),
        )
        .show(ctx, |ui| {
            // ── Persistent tab state via egui memory ───────────────────────
            let tab_id = ui.id().with("settings_tab");
            let mut tab: SettingsTab = ui.data(|d| d.get_temp(tab_id).unwrap_or_default());

            // ── Tab bar ────────────────────────────────────────────────────
            ui.horizontal(|ui| {
                for (t, label) in [
                    (SettingsTab::Theme, "Theme"),
                    (SettingsTab::Apis,  "APIs"),
                    (SettingsTab::Paths, "Paths"),
                ] {
                    let active = tab == t;
                    let fill = if active {
                        theme::chrome_active_fill()
                    } else {
                        egui::Color32::TRANSPARENT
                    };
                    let color = if active {
                        theme::chrome_active_text()
                    } else {
                        theme::text_muted()
                    };
                    let btn = egui::Button::new(
                        egui::RichText::new(label).color(color),
                    )
                    .fill(fill)
                    .corner_radius(4.0);
                    if ui.add(btn).clicked() {
                        tab = t;
                    }
                }
            });
            ui.separator();
            ui.add_space(6.0);

            // ── Scrollable tab content ─────────────────────────────────────
            // Reserve ~52 px at the bottom for the action buttons.
            let content_height = ui.available_height() - 52.0;
            egui::ScrollArea::vertical()
                .max_height(content_height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    match tab {
                        SettingsTab::Theme => tab_theme(ui, model),
                        SettingsTab::Apis  => tab_apis(ui, model),
                        SettingsTab::Paths => tab_paths(ui, model),
                    }
                });

            // ── Always-visible action buttons ─────────────────────────────
            ui.separator();
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Save Settings").clicked() {
                    save_requested = true;
                }
                if ui.button("Poll Now").clicked() {
                    poll_requested = true;
                }
                if ui.button("Clear Keys").clicked() {
                    clear_requested = true;
                }
            });

            ui.data_mut(|d| d.insert_temp(tab_id, tab));
        });

    // ── Post-render actions ────────────────────────────────────────────────
    if save_requested {
        let had_key = model.has_factal_api_key();
        model.factal_api_key = model.factal_api_key.trim().to_owned();
        model.ny511_api_key = model.ny511_api_key.trim().to_owned();
        model.windy_webcams_api_key = model.windy_webcams_api_key.trim().to_owned();
        model.aisstream_api_key = model.aisstream_api_key.trim().to_owned();
        moving_tracks::invalidate();
        match model.save_settings() {
            Ok(()) => {
                model.apply_saved_settings();
                model.factal_stream_status = if model.has_factal_api_key() {
                    "configured".into()
                } else {
                    "demo".into()
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
                    model.push_log("Factal API key cleared; stream returned to demo mode.".into());
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
    }

    if clear_requested {
        model.factal_api_key.clear();
        model.ny511_api_key.clear();
        model.windy_webcams_api_key.clear();
        model.aisstream_api_key.clear();
        moving_tracks::invalidate();
        match model.save_settings() {
            Ok(()) => {
                model.factal_stream_status = "demo".into();
                model.camera_registry_status = "demo".into();
                factal_stream::invalidate();
                camera_registry::invalidate();
                model.push_log("API keys cleared; streams returned to demo mode.".into());
            }
            Err(error) => {
                model.push_log(format!("Settings clear failed: {}", error));
            }
        }
    }

    model.factal_settings_open = open;
}

// ── Tab content ───────────────────────────────────────────────────────────────

fn tab_theme(ui: &mut egui::Ui, model: &mut AppModel) {
    ui.heading("Map Theme");
    ui.colored_label(
        theme::text_muted(),
        "Each palette is grounded in a complementary color relationship. \
         The active theme applies immediately.",
    );
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        for &t in theme::MapTheme::ALL {
            let is_active = model.map_theme == t;
            let fill = if is_active {
                theme::chrome_active_fill()
            } else {
                egui::Color32::TRANSPARENT
            };
            let label_color = if is_active {
                theme::chrome_active_text()
            } else {
                theme::text_muted()
            };
            let btn = egui::Button::new(
                egui::RichText::new(t.label()).color(label_color).small(),
            )
            .fill(fill)
            .corner_radius(4.0);
            if ui.add(btn).clicked() {
                model.map_theme = t;
            }
        }
    });
    if let Some(active) = theme::MapTheme::ALL.iter().find(|&&t| t == model.map_theme) {
        ui.small(active.theory());
    }
}

fn tab_apis(ui: &mut egui::Ui, model: &mut AppModel) {
    // ── Factal ────────────────────────────────────────────────────────────
    ui.heading("Factal");
    ui.colored_label(
        theme::text_muted(),
        "Configure the private Factal token used for live event polling every minute.",
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
    ui.small("Stored locally in the executable directory settings file for this demo build.");
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
    ui.small(format!("Camera registry status: {}", model.camera_registry_status));
    ui.small(
        "Optional no-key sources can be declared in Data/camera_sources/public_sources.json \
         and Data/camera_sources/scrape_sources.json under the asset root.",
    );

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

fn tab_paths(ui: &mut egui::Ui, model: &mut AppModel) {
    ui.heading("Paths");
    ui.colored_label(
        theme::text_muted(),
        "Leave Data/Derived/SRTM/Planet/GDAL blank to use the executable-folder defaults \
         and PATH-based GDAL discovery.",
    );
    ui.add_space(8.0);

    // Quick asset-root picker
    ui.horizontal(|ui| {
        ui.label("Asset Root (quick pick)");
        if ui.button("📂 Browse Folder…").clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .set_directory(
                    model
                        .selected_root
                        .clone()
                        .or_else(settings_store::effective_asset_root)
                        .unwrap_or_default(),
                )
                .pick_folder()
            {
                model.set_selected_root(path);
            }
        }
        if let Some(root) = &model.selected_root {
            ui.colored_label(theme::text_muted(), root.display().to_string());
        } else {
            ui.colored_label(theme::text_muted(), "none selected");
        }
    });
    ui.add_space(6.0);

    path_row(ui, "Asset Root",    &mut model.settings_asset_root,    true,  false);
    path_row(ui, "Data Root",     &mut model.settings_data_root,     true,  true);
    path_row(ui, "Derived Root",  &mut model.settings_derived_root,  true,  true);
    path_row(ui, "SRTM Root",     &mut model.settings_srtm_root,     true,  true);
    path_row(ui, "Planet PBF",    &mut model.settings_planet_path,   false, true);
    path_row(ui, "GDAL Bin Dir",  &mut model.settings_gdal_bin_dir,  true,  true);
    path_row(ui, "Osmium Bin Dir",&mut model.settings_osmium_bin_dir,true,  true);
    ui.add_space(4.0);
    ui.checkbox(&mut model.settings_prefer_overpass, "Prefer Overpass API")
        .on_hover_text(
            "When checked, road imports always use the Overpass API even if osmium \
             and a local planet file are available.  Faster for explored areas; \
             requires internet.",
        );
}

fn path_row(ui: &mut egui::Ui, label: &str, value: &mut String, folder: bool, allow_clear: bool) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add_sized(
            [ui.available_width() - 140.0, 28.0],
            egui::TextEdit::singleline(value).hint_text("Default / auto-detect"),
        );
        if ui.button("Browse").clicked() {
            if folder {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    *value = path.display().to_string();
                }
            } else if let Some(path) = rfd::FileDialog::new().pick_file() {
                *value = path.display().to_string();
            }
        }
        if allow_clear && ui.button("Clear").clicked() {
            value.clear();
        }
    });
}
