//! Terrain Library controls for installing and refreshing one Atlas region.
use crate::{model::AppModel, terrain_assets};
use egui::{Context, Id};
use std::path::{Path, PathBuf};
use tile_archive::atlas::{Tier, regions::Region};

#[derive(Clone, Default)]
struct State {
    busy: bool,
    message: String,
}
enum Action {
    Pin,
    Import(PathBuf),
    Verify,
    Unpin,
}
pub(super) fn show(ui: &mut egui::Ui, model: &AppModel) {
    use crate::model::ActiveBody;
    if model.active_body != ActiveBody::Earth {
        return;
    }
    let pos = if model.globe_view.local_mode {
        model.globe_view.local_center
    } else {
        model.globe_view.globe_center_latlon()
    };
    let region = Region::at(pos.lon.into(), pos.lat.into());
    let [w, s, e, n] = region.bounds();
    let id = Id::new("terrain-library-regions");
    let state = ui
        .ctx()
        .data(|d| d.get_temp::<State>(id))
        .unwrap_or_default();
    ui.collapsing("Offline Atlas regions", |ui| {
        ui.label(format!(
            "Current map center · {w:.0}° to {e:.0}° longitude, {s:.0}° to {n:.0}° latitude"
        ));
        ui.small(format!(
            "GEBCO relief · 15 arcseconds · pinned storage budget {} MiB",
            crate::gpu_config::settings().atlas_pin_bytes >> 20
        ));
        let mut action = None;
        ui.add_enabled_ui(!state.busy, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Pin / update region").clicked() {
                    action = Some(Action::Pin);
                }
                if ui.button("Install from file…").clicked()
                    && let Some(p) = rfd::FileDialog::new()
                        .add_filter("Atlas height archive", &["sqlite"])
                        .pick_file()
                {
                    action = Some(Action::Import(p));
                }
                if ui.button("Verify region").clicked() {
                    action = Some(Action::Verify);
                }
                if ui.button("Unpin region").clicked() {
                    action = Some(Action::Unpin);
                }
            })
        });
        if !state.message.is_empty() {
            ui.label(&state.message);
        }
        if let Some(action) = action {
            let root = model.selected_root.clone();
            let ctx = ui.ctx().clone();
            status(&ctx, id, true, format!("Working on {w:.0}°, {s:.0}°…"));
            let wake = ctx.clone();
            if let Err(error) = std::thread::Builder::new()
                .name("atlas-offline-region".into())
                .spawn(move || {
                    let result = run(action, root.as_deref(), region, &wake, id);
                    status(
                        &wake,
                        id,
                        false,
                        result.unwrap_or_else(|e| format!("Region {w:.0}°, {s:.0}°: {e}")),
                    );
                })
            {
                status(&ctx, id, false, error.to_string());
            }
        }
    });
}
fn status(ctx: &Context, id: Id, busy: bool, message: String) {
    ctx.data_mut(|d| d.insert_temp(id, State { busy, message }));
    ctx.request_repaint();
}
fn run(
    action: Action,
    root: Option<&Path>,
    region: Region,
    ctx: &Context,
    id: Id,
) -> Result<String, String> {
    let derived = terrain_assets::find_derived_root(root)
        .ok_or("Choose a Derived folder in Settings first")?;
    let directory = derived.join("terrain/regions");
    let path = region.path(&directory);
    match action {
        Action::Verify => {
            region.verify(&path)?;
            return Ok(format!("Verified {}", path.display()));
        }
        Action::Unpin => {
            region.unpin(&directory)?;
        }
        action => {
            region.install(
                &directory,
                crate::gpu_config::settings().atlas_pin_bytes as u64,
                |stage| {
                    if let Action::Import(source) = action {
                        return region.extract(&source, stage);
                    }
                    let atlas = derived.join("terrain").join(tile_archive::atlas::FILE_NAME);
                    if atlas.is_file() && region.extract(&atlas, stage).is_ok() {
                        return Ok(());
                    }
                    let data = terrain_assets::find_data_root(root).ok_or(
                        "This region needs prepared Atlas heights or local GEBCO source files",
                    )?;
                    use one_thousand_electric_eye_cache_builder::atlas;
                    atlas::run(
                        atlas::Command {
                            data_root: data,
                            srtm_root: None,
                            cache: stage.into(),
                            gdal_bin_dir: crate::settings_store::configured_gdal_bin_dir()
                                .unwrap_or_default(),
                            bounds: region.source_bounds(),
                            tiers: vec![Tier::Terrain],
                        },
                        &mut |p| {
                            status(
                                ctx,
                                id,
                                true,
                                format!("{} · {} / {} tiles", p.state, p.done, p.total),
                            )
                        },
                    )
                    .map(|_| ())
                },
            )?;
        }
    }
    super::world_map::invalidate_atlas_regions();
    Ok(if path.is_file() {
        format!("Pinned and verified {}", path.display())
    } else {
        format!("Unpinned {}", path.display())
    })
}
