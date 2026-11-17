use crate::{model::AppModel, settings_store, terrain_assets, theme};

pub(super) fn render(ui: &mut egui::Ui, model: &mut AppModel) {
    ui.heading("Files & Cache");
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
    });
    if let Some(root) = &model.selected_root {
        ui.colored_label(theme::text_muted(), root.display().to_string());
    } else {
        ui.colored_label(theme::text_muted(), "none selected");
    }
    ui.add_space(6.0);

    path_row(
        ui,
        "Asset Root",
        &mut model.settings_asset_root,
        true,
        false,
    );
    path_row(ui, "Data Root", &mut model.settings_data_root, true, true);
    path_row(
        ui,
        "Derived Root",
        &mut model.settings_derived_root,
        true,
        true,
    );
    super::contour_cache::render(ui, model.selected_root.as_deref());
    ui.add_space(4.0);

    // ── Derived-directory asset diagnostics ───────────────────────────────────
    // Show the resolved Derived root and check for the specific files each
    // layer needs, so a missing/wrong directory is immediately obvious.
    {
        let root = model.selected_root.as_deref();
        let derived = terrain_assets::find_derived_root(root);

        let ok_color = egui::Color32::from_rgb(80, 200, 100);
        let warn_color = egui::Color32::from_rgb(220, 100, 60);
        let dim_color = egui::Color32::from_rgb(150, 180, 200);

        match &derived {
            Some(d) => {
                ui.colored_label(dim_color, format!("Derived root: {}", d.display()));

                // Files that each layer depends on
                let checks: &[(&str, &str)] = &[
                    (
                        "gebco_depth_1440x720.bil",
                        "Globe bathymetry fill (depth texture)",
                    ),
                    (
                        "gebco_2025_contours_200m.gpkg",
                        "Bathymetry isobaths (globe + local)",
                    ),
                    ("gebco_2025_coastline_0m.gpkg", "Global coastlines"),
                ];
                let building =
                    crate::panels::world_map::srtm_focus_cache::is_gebco_derived_building();
                for (filename, label) in checks {
                    let path = d.join("terrain").join(filename);
                    if path.exists() {
                        ui.colored_label(ok_color, format!("  ✓ {label}: terrain/{filename}"));
                    } else if building && filename != &"gebco_2025_coastline_0m.gpkg" {
                        ui.colored_label(
                            egui::Color32::from_rgb(180, 180, 60),
                            format!("  ⏳ {label}: terrain/{filename} (building…)"),
                        );
                    } else {
                        ui.colored_label(
                            warn_color,
                            format!("  ✗ {label}: terrain/{filename} NOT FOUND"),
                        );
                    }
                }
            }
            None => {
                ui.colored_label(
                    warn_color,
                    "Derived root: NOT RESOLVED — set Derived Root or Asset Root above",
                );
            }
        }
    }
    ui.add_space(4.0);

    path_row(ui, "SRTM Root", &mut model.settings_srtm_root, true, true);
    path_row(
        ui,
        "Planet PBF",
        &mut model.settings_planet_path,
        false,
        true,
    );
    path_row(
        ui,
        "GDAL Bin Dir",
        &mut model.settings_gdal_bin_dir,
        true,
        true,
    );
    path_row(
        ui,
        "Osmium Bin Dir",
        &mut model.settings_osmium_bin_dir,
        true,
        true,
    );
    ui.add_space(4.0);
    ui.checkbox(&mut model.settings_prefer_overpass, "Prefer Overpass API")
        .on_hover_text(
            "When checked, road imports always use the Overpass API even if osmium \
             and a local planet file are available.  Faster for explored areas; \
             requires internet.",
        );

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);
    ui.strong("Detailed Terrain");
    ui.label(
        egui::RichText::new(
            "Loads detailed terrain for the area in view from official sources in the US, \
             England, France, the Netherlands, Switzerland, New Zealand and Japan. \
             Where finer data is missing, the existing terrain stays visible.",
        )
        .small()
        .color(theme::text_muted()),
    );
    ui.add_space(4.0);
    ui.checkbox(
        &mut model.settings_threedep_enabled,
        "Stream detailed terrain",
    )
    .on_hover_text(
        "Fetch elevation for contours, markers and map layers as you explore. \
         Downloaded contour rasters are removed after processing. Choose USGS or \
         Washington DNR LiDAR hillshade in the Layers drawer.",
    );

    ui.collapsing("Elevation sources and credits", |ui| {
        ui.hyperlink_to(
            "United States — USGS 3DEP, 1 m",
            "https://www.usgs.gov/3d-elevation-program",
        );
        for provider in crate::elevation_sources::PROVIDERS {
            ui.hyperlink_to(provider.label(), provider.url());
        }
        ui.small(
            "Contour geometry is derived by 1kEE. Coverage and survey dates vary by location.",
        );
    });

    ui.horizontal(|ui| {
        ui.label("Chunk cache budget");
        ui.add(
            egui::Slider::new(
                &mut model.settings_threedep_cache_budget_gb,
                settings_store::MIN_THREEDEP_CACHE_BUDGET_GB
                    ..=settings_store::MAX_THREEDEP_CACHE_BUDGET_GB,
            )
            .suffix(" GB")
            .logarithmic(true),
        )
        .on_hover_text(
            "Ceiling for cached elevation chunks.  Least-recently-used chunks are \
             evicted past this; contours already extracted from them are kept.",
        );
    });

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);
    ui.strong("Local Map GPU Budget");
    ui.label(
        egui::RichText::new(
            "Local contours and roads reuse geometry uploaded to the GPU in batches, \
             so this budget controls cached geometry rather than a per-frame \
             drawing limit.  Least-recently-drawn tiles are evicted past it and \
             re-uploaded if revisited.",
        )
        .small()
        .color(theme::text_muted()),
    );
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("GPU geometry budget");
        ui.add(
            egui::Slider::new(
                &mut model.settings_local_contour_vram_budget_gb,
                settings_store::MIN_LOCAL_CONTOUR_VRAM_BUDGET_GB
                    ..=settings_store::MAX_LOCAL_CONTOUR_VRAM_BUDGET_GB,
            )
            .suffix(" GB")
            .logarithmic(true),
        );
    });
    {
        let resident_gb = crate::panels::world_map::local_contour_pass::resident_bytes() as f64
            / (1024.0 * 1024.0 * 1024.0);
        ui.label(
            egui::RichText::new(format!(
                "Resident map geometry: {resident_gb:.2} GB of {:.2} GB",
                model.settings_local_contour_vram_budget_gb
            ))
            .small()
            .color(theme::text_muted()),
        );
    }

    if let Some(derived_root) = terrain_assets::find_derived_root(model.selected_root.as_deref()) {
        let used_gb =
            crate::threedep::cache_bytes(&derived_root) as f64 / (1024.0 * 1024.0 * 1024.0);
        ui.label(
            egui::RichText::new(format!(
                "Cache in use: {used_gb:.2} GB of {:.0} GB",
                model.settings_threedep_cache_budget_gb
            ))
            .small()
            .color(theme::text_muted()),
        );
    }
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
