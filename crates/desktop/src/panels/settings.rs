//! One settings window; sections render content rather than opening more windows.
use crate::{model::AppModel, theme};
#[path = "settings_actions.rs"]
mod actions;
#[path = "settings_appearance.rs"]
mod appearance;
#[path = "settings_overview.rs"]
mod overview;
#[path = "settings_paths.rs"]
mod paths;
#[path = "settings_sources.rs"]
mod sources;

#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
enum Section {
    #[default]
    Overview,
    Appearance,
    Sources,
    Paths,
    Terrain,
    Activity,
}
impl Section {
    const ALL: [(Self, &'static str); 6] = [
        (Self::Overview, "Overview"),
        (Self::Appearance, "Appearance"),
        (Self::Sources, "Data Sources"),
        (Self::Paths, "Files & Cache"),
        (Self::Terrain, "Terrain Library"),
        (Self::Activity, "Activity"),
    ];
}

pub fn render_settings(ctx: &egui::Context, model: &mut AppModel) {
    if !model.settings_open {
        return;
    }
    let mut open = model.settings_open;
    let mut actions = actions::Actions::default();
    let id = egui::Id::new("settings-section");
    let mut section = ctx.data(|d| d.get_temp::<Section>(id).unwrap_or_default());
    egui::Window::new("Settings")
        .id(egui::Id::new("app-settings"))
        .open(&mut open)
        .default_size(egui::vec2(840.0, 600.0))
        .min_size(egui::vec2(640.0, 400.0))
        .frame(
            egui::Frame::window(&ctx.style())
                .fill(theme::window_fill())
                .stroke(egui::Stroke::new(1.0, theme::window_stroke())),
        )
        .show(ctx, |ui| {
            // Fixed viewport cap prevents content from growing the window every frame.
            let content_height = (ctx.screen_rect().height() - 200.0).clamp(260.0, 460.0);
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(132.0, content_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.strong("1kEE");
                        ui.add_space(12.0);
                        for (value, label) in Section::ALL {
                            let selected = section == value;
                            if ui
                                .add_sized(
                                    [132.0, 30.0],
                                    egui::Button::new(label).selected(selected),
                                )
                                .clicked()
                            {
                                section = value;
                            }
                        }
                    },
                );
                let (divider, _) =
                    ui.allocate_exact_size(egui::vec2(1.0, content_height), egui::Sense::hover());
                ui.painter().vline(
                    divider.center().x,
                    divider.y_range(),
                    ui.visuals().widgets.noninteractive.bg_stroke,
                );
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::vertical()
                        .id_salt(("settings-content", section))
                        .auto_shrink([false, false])
                        .max_height(content_height)
                        .show(ui, |ui| match section {
                            Section::Overview => overview::render(ui, model),
                            Section::Appearance => appearance::render(ui, model),
                            Section::Sources => sources::render(ui, model),
                            Section::Paths => paths::render(ui, model),
                            Section::Terrain => super::terrain_library::render(ui, model),
                            Section::Activity => super::status_log::render(ui, model),
                        });
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                actions.save = ui.button("Save Settings").clicked();
                if matches!(section, Section::Overview | Section::Sources) {
                    actions.poll = ui.button("Poll Now").clicked();
                }
                if section == Section::Sources {
                    actions.clear_keys = ui.button("Clear Keys").clicked();
                }
                if section == Section::Appearance {
                    ui.small("Appearance changes apply immediately.");
                }
            });
        });
    ctx.data_mut(|d| d.insert_temp(id, section));
    actions.apply(model);
    model.settings_open = open;
}

#[cfg(test)]
#[path = "chrome_tests.rs"]
mod tests;
