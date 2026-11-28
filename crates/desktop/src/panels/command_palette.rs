//! Spotlight-style `Ctrl+K` Multi-World Command Palette (Earth, Moon, Mars & Coordinates).

use crate::gazetteer::{self, GazetteerHit};
use crate::model::{ActiveBody, AppModel};
use crate::theme;

#[derive(Default)]
pub struct CommandPalette {
    pub open: bool,
    pub query: String,
    pub selected_idx: usize,
    focus_input: bool,
}

impl CommandPalette {
    pub fn toggle(&mut self) {
        self.open = !self.open;
        if self.open {
            self.focus_input = true;
            self.selected_idx = 0;
        }
    }

    pub fn render(&mut self, ctx: &egui::Context, model: &mut AppModel) {
        // Global Ctrl+K / Cmd+K shortcut
        let shortcut = ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::COMMAND, egui::Key::K)
                || i.consume_key(egui::Modifiers::CTRL, egui::Key::K)
        });
        if shortcut {
            self.toggle();
        }
        if !self.open {
            return;
        }

        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.open = false;
            return;
        }

        let hits = gazetteer::search(&self.query, 14);
        if !hits.is_empty() {
            self.selected_idx = self.selected_idx.min(hits.len() - 1);
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)) {
                self.selected_idx = (self.selected_idx + 1) % hits.len();
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) {
                self.selected_idx = (self.selected_idx + hits.len() - 1) % hits.len();
            }
        } else {
            self.selected_idx = 0;
        }

        let mut chosen: Option<GazetteerHit> = None;
        let enter_pressed = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        if enter_pressed && let Some(hit) = hits.get(self.selected_idx) {
            chosen = Some(hit.clone());
        }

        egui::Window::new("Jump to Location (Ctrl+K)")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 72.0))
            .default_width(480.0)
            .min_width(480.0)
            .max_width(480.0)
            .frame(
                egui::Frame::window(&ctx.style())
                    .fill(theme::panel_fill(245))
                    .stroke(egui::Stroke::new(1.2, theme::topo_color().gamma_multiply(0.8)))
                    .corner_radius(10.0),
            )
            .show(ctx, |ui| {
                let edit = ui.add_sized(
                    [ui.available_width(), 30.0],
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text("Search Earth, Moon, Mars, peaks, craters, or lat, lon…"),
                );
                if self.focus_input {
                    edit.request_focus();
                    self.focus_input = false;
                }
                if edit.changed() {
                    self.selected_idx = 0;
                }

                ui.add_space(6.0);
                if hits.is_empty() {
                    ui.colored_label(theme::text_muted(), "No matching locations.");
                } else {
                    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                        for (idx, hit) in hits.iter().enumerate() {
                            let is_sel = idx == self.selected_idx;
                            let fill = if is_sel {
                                theme::selected_item_fill()
                            } else {
                                theme::item_fill()
                            };
                            let resp = egui::Frame::new()
                                .fill(fill)
                                .corner_radius(6.0)
                                .inner_margin(egui::Margin::symmetric(8, 6))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        let (body_tag, body_col) = match hit.body {
                                            ActiveBody::Earth => ("EARTH", theme::topo_color()),
                                            ActiveBody::Moon => {
                                                ("MOON", egui::Color32::from_rgb(155, 200, 248))
                                            }
                                            ActiveBody::Mars => {
                                                ("MARS", egui::Color32::from_rgb(245, 130, 85))
                                            }
                                        };
                                        ui.colored_label(
                                            body_col,
                                            egui::RichText::new(body_tag).monospace().small(),
                                        );
                                        ui.strong(&hit.name);
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.small(format!(
                                                    "{:.2}°, {:.2}°",
                                                    hit.location.lat, hit.location.lon
                                                ));
                                            },
                                        );
                                    });
                                    ui.horizontal(|ui| {
                                        ui.colored_label(
                                            theme::text_muted(),
                                            egui::RichText::new(format!(
                                                "{} · {}",
                                                hit.kind, hit.region
                                            ))
                                            .small(),
                                        );
                                        if let Some(elev) = hit.elevation_m {
                                            ui.small(format!("· {:.0} m", elev));
                                        }
                                    });
                                })
                                .response
                                .interact(egui::Sense::click());
                            if resp.clicked() {
                                chosen = Some(hit.clone());
                            }
                            ui.add_space(4.0);
                        }
                    });
                }
                ui.add_space(4.0);
                ui.small("↑/↓ select · Enter jump · Esc close");
            });

        if let Some(hit) = chosen {
            apply_jump(ctx, model, &hit);
            self.open = false;
        }
    }
}

fn apply_jump(ctx: &egui::Context, model: &mut AppModel, hit: &GazetteerHit) {
    model.stop_event_follow();
    model.focused_city_id = None;
    if model.active_body != hit.body {
        model.active_body = hit.body;
        if hit.body != ActiveBody::Earth {
            model.globe_view.local_mode = false;
        }
        model.map_theme = match hit.body {
            ActiveBody::Earth => theme::MapTheme::Topo,
            ActiveBody::Moon => theme::MapTheme::Lunar,
            ActiveBody::Mars => theme::MapTheme::MarsDark,
        };
        theme::set_theme(ctx, model.map_theme);
    }
    model.globe_view.stop_motion();
    model.globe_view.focus_on(hit.location);
    model.globe_view.range_m = model.globe_view.range_m.min(180_000.0);
    model.globe_view.sync_legacy_from_unified();
    model.push_log(format!(
        "Jumped to {} ({:.4}°, {:.4}°)",
        hit.name, hit.location.lat, hit.location.lon
    ));
}
