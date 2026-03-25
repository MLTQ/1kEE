use crate::model::AppModel;
use crate::theme;

/// Floating cinematic-style overlay for replay controls.
/// Rendered inside the world-map `egui::Ui` so it floats over the globe.
pub fn render_replay_controls(ui: &mut egui::Ui, model: &mut AppModel) {
    if !model.replay_mode {
        return;
    }

    let map_rect = ui.min_rect();
    // Anchor to bottom-centre of the map area.
    let panel_width = 380.0f32;
    let anchor = egui::pos2(
        map_rect.center().x - panel_width * 0.5,
        map_rect.bottom() - 140.0,
    );

    egui::Area::new("replay_controls".into())
        .fixed_pos(anchor)
        .order(egui::Order::Foreground)
        .interactable(true)
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(theme::panel_fill(230))
                .stroke(egui::Stroke::new(1.0, theme::panel_stroke()))
                .corner_radius(10.0)
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(panel_width);
                    draw_controls(ui, model);
                });
        });
}

fn draw_controls(ui: &mut egui::Ui, model: &mut AppModel) {
    // ── Header row ────────────────────────────────────────────────────────
    ui.horizontal(|ui| {
        ui.colored_label(
            egui::Color32::from_rgb(100, 195, 255),
            egui::RichText::new("REPLAY").small().strong(),
        );

        let event_count = model
            .replay_state
            .as_ref()
            .map(|s| s.events.len())
            .unwrap_or(0);
        ui.colored_label(
            theme::text_muted(),
            egui::RichText::new(format!(
                "· {} events · last {} days",
                event_count, model.replay_days
            ))
            .small(),
        );

        // History fetch status
        if crate::factal_stream::is_history_fetching() {
            ui.colored_label(
                egui::Color32::from_rgb(255, 186, 73),
                egui::RichText::new("⟳ loading history").small(),
            );
        } else if !model.replay_history_status.is_empty() && event_count == 0 {
            ui.colored_label(
                theme::text_muted(),
                egui::RichText::new(&model.replay_history_status).small(),
            );
        }

        // Exit button — right-aligned
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let exit_btn = egui::Button::new(egui::RichText::new("✕").small())
                .fill(egui::Color32::TRANSPARENT);
            if ui.add(exit_btn).on_hover_text("Exit replay").clicked() {
                model.toggle_replay();
            }
        });
    });

    // ── Progress bar ──────────────────────────────────────────────────────
    let progress = model.replay_state.as_ref().map(|s| s.progress()).unwrap_or(0.0);
    let finished = model.replay_state.as_ref().map(|s| s.is_finished()).unwrap_or(false);

    ui.add_space(4.0);
    ui.add(
        egui::ProgressBar::new(progress)
            .show_percentage()
            .animate(!model.replay_state.as_ref().map(|s| s.is_paused()).unwrap_or(true)),
    );

    // ── Sliders ───────────────────────────────────────────────────────────
    ui.add_space(2.0);
    let days_changed = ui
        .horizontal(|ui| {
            ui.label(egui::RichText::new("Days").small());
            ui.spacing_mut().slider_width = 240.0;
            ui.add(
                egui::Slider::new(&mut model.replay_days, 1_u32..=365)
                    .show_value(true)
                    .suffix(" days"),
            )
            .changed()
        })
        .inner;

    let dur_changed = ui
        .horizontal(|ui| {
            ui.label(egui::RichText::new("Duration").small());
            ui.spacing_mut().slider_width = 240.0;
            ui.add(
                egui::Slider::new(&mut model.replay_duration_secs, 60_u32..=300)
                    .show_value(true)
                    .suffix(" s"),
            )
            .changed()
        })
        .inner;

    // Rebuild replay state if sliders changed.
    if days_changed || dur_changed {
        model.rebuild_replay_state();
    }

    // ── Transport controls ────────────────────────────────────────────────
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        // Play / Pause
        let is_paused = model
            .replay_state
            .as_ref()
            .map(|s| s.is_paused())
            .unwrap_or(true);

        let pp_label = if is_paused || finished { "▶ Play" } else { "⏸ Pause" };
        let pp_fill = if !is_paused && !finished {
            egui::Color32::from_rgb(20, 80, 140)
        } else {
            egui::Color32::TRANSPARENT
        };
        let pp_text = if !is_paused && !finished {
            egui::Color32::from_rgb(100, 195, 255)
        } else {
            theme::text_muted()
        };
        let pp_btn = egui::Button::new(egui::RichText::new(pp_label).small().color(pp_text))
            .fill(pp_fill)
            .corner_radius(4.0);
        if ui.add(pp_btn).clicked() {
            if let Some(state) = &mut model.replay_state {
                if finished {
                    state.restart();
                } else if state.is_paused() {
                    state.resume();
                } else {
                    state.pause();
                }
            } else {
                model.rebuild_replay_state();
            }
        }

        // Restart
        let rst_btn = egui::Button::new(egui::RichText::new("↺ Restart").small())
            .fill(egui::Color32::TRANSPARENT);
        if ui.add(rst_btn).clicked() {
            if let Some(state) = &mut model.replay_state {
                state.restart();
            }
        }
    });
}
