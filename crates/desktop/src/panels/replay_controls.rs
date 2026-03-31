use crate::event_store;
use crate::model::{AppModel, EventSeverity};
use crate::theme;

const PANEL_WIDTH: f32 = 480.0;
/// Maximum history the range slider can reach back (matches the DB pruning window).
const MAX_DAYS: f64 = 365.0;

/// Floating cinematic-style overlay for replay controls.
/// Rendered inside the world-map `egui::Ui` so it floats over the globe.
pub fn render_replay_controls(ui: &mut egui::Ui, model: &mut AppModel) {
    if !model.replay_mode {
        return;
    }

    let map_rect = ui.min_rect();
    let anchor = egui::pos2(
        map_rect.center().x - PANEL_WIDTH * 0.5,
        map_rect.bottom() - 200.0,
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
                    ui.set_width(PANEL_WIDTH);
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
                "· {} events · {} → {}",
                event_count,
                event_store::unix_to_date_str(model.replay_from_unix),
                event_store::unix_to_date_str(model.replay_to_unix),
            ))
            .small(),
        );

        if crate::factal_stream::is_history_fetching() {
            ui.colored_label(
                egui::Color32::from_rgb(255, 186, 73),
                egui::RichText::new("⟳").small(),
            );
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let exit_btn = egui::Button::new(egui::RichText::new("✕").small())
                .fill(egui::Color32::TRANSPARENT);
            if ui.add(exit_btn).on_hover_text("Exit replay").clicked() {
                model.toggle_replay();
            }
        });
    });

    // ── Scrub bar (playback timeline with event tick marks) ───────────────
    ui.add_space(6.0);
    draw_scrub_bar(ui, model);

    // ── Range slider (time window selection) ──────────────────────────────
    ui.add_space(8.0);
    ui.colored_label(
        theme::text_muted(),
        egui::RichText::new("Time window").small(),
    );
    ui.add_space(2.0);
    let now = event_store::now_unix();
    let mut from_unix = model.replay_from_unix;
    let mut to_unix = model.replay_to_unix;
    let range_changed = draw_range_slider(ui, &mut from_unix, &mut to_unix, now);
    if range_changed {
        model.replay_from_unix = from_unix;
        model.replay_to_unix = to_unix;
        model.replay_from_str = event_store::unix_to_date_str(from_unix);
        model.replay_to_str = event_store::unix_to_date_str(to_unix);
    }

    // ── Date text inputs ──────────────────────────────────────────────────
    let date_changed = draw_date_inputs(ui, model);

    if range_changed || date_changed {
        model.rebuild_replay_state();
    }

    // ── Duration slider ───────────────────────────────────────────────────
    ui.add_space(4.0);
    let dur_changed = ui
        .horizontal(|ui| {
            ui.label(egui::RichText::new("Duration").small());
            ui.spacing_mut().slider_width = 330.0;
            ui.add(
                egui::Slider::new(&mut model.replay_duration_secs, 60_u32..=300)
                    .show_value(true)
                    .suffix(" s"),
            )
            .changed()
        })
        .inner;
    if dur_changed {
        model.rebuild_replay_state();
    }

    // ── Transport controls ────────────────────────────────────────────────
    ui.add_space(4.0);
    draw_transport(ui, model);
}

// ── Scrub bar ─────────────────────────────────────────────────────────────────

fn draw_scrub_bar(ui: &mut egui::Ui, model: &mut AppModel) {
    let bar_h = 18.0f32;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), bar_h),
        egui::Sense::click_and_drag(),
    );

    if !ui.is_rect_visible(rect) {
        return;
    }

    let painter = ui.painter();

    // Track background.
    painter.rect_filled(rect, 4.0, egui::Color32::from_rgb(18, 22, 32));

    let progress = model
        .replay_state
        .as_ref()
        .map(|s| s.progress())
        .unwrap_or(0.0);
    let sim_from = model.replay_state.as_ref().map(|s| s.sim_from).unwrap_or(0);
    let sim_to = model.replay_state.as_ref().map(|s| s.sim_to).unwrap_or(1);
    let sim_span = (sim_to - sim_from).max(1) as f32;

    // Event tick marks — drawn before the progress fill so they show through.
    if let Some(state) = model.replay_state.as_ref() {
        for (unix, event) in &state.events {
            let frac = ((*unix - sim_from) as f32 / sim_span).clamp(0.0, 1.0);
            let x = rect.left() + frac * rect.width();
            let color = severity_tick_color(&event.severity);
            painter.line_segment(
                [
                    egui::pos2(x, rect.top() + 2.0),
                    egui::pos2(x, rect.bottom() - 2.0),
                ],
                egui::Stroke::new(1.5, color),
            );
        }
    }

    // Semi-transparent progress fill over the ticks (dims played region).
    if progress > 0.005 {
        let fill = egui::Rect::from_min_max(
            rect.min,
            egui::pos2(rect.left() + progress * rect.width(), rect.max.y),
        );
        painter.rect_filled(
            fill,
            4.0,
            egui::Color32::from_rgba_unmultiplied(30, 90, 160, 65),
        );
    }

    // Playhead line.
    let head_x = rect.left() + progress * rect.width();
    painter.line_segment(
        [
            egui::pos2(head_x, rect.top()),
            egui::pos2(head_x, rect.bottom()),
        ],
        egui::Stroke::new(2.0, egui::Color32::from_rgb(100, 195, 255)),
    );

    // Border.
    painter.rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(1.0, theme::panel_stroke()),
        egui::StrokeKind::Outside,
    );

    // Seek on click/drag.
    if response.dragged() || response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            let frac = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            if let Some(state) = &mut model.replay_state {
                state.seek(frac);
            }
        }
    }
}

fn severity_tick_color(sev: &EventSeverity) -> egui::Color32 {
    match sev {
        EventSeverity::Critical => egui::Color32::from_rgba_unmultiplied(210, 55, 55, 210),
        EventSeverity::Elevated => egui::Color32::from_rgba_unmultiplied(210, 140, 30, 180),
        EventSeverity::Advisory => egui::Color32::from_rgba_unmultiplied(75, 115, 165, 140),
    }
}

// ── Two-thumb range slider ────────────────────────────────────────────────────

/// Custom two-handled range slider spanning 365 days → now.
/// Returns `true` if either value changed.
fn draw_range_slider(ui: &mut egui::Ui, from_unix: &mut i64, to_unix: &mut i64, now: i64) -> bool {
    let height = 26.0f32;
    let handle_r = 7.0f32;
    let track_h = 4.0f32;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );

    if !ui.is_rect_visible(rect) {
        return false;
    }

    let span_secs = MAX_DAYS * 86_400.0;
    let origin = now as f64 - span_secs;

    let unix_to_x = |unix: i64| -> f32 {
        let frac = ((unix as f64 - origin) / span_secs).clamp(0.0, 1.0) as f32;
        rect.left() + frac * rect.width()
    };
    let x_to_unix = |x: f32| -> i64 {
        let frac = ((x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64;
        (origin + frac * span_secs) as i64
    };

    let from_x = unix_to_x(*from_unix);
    let to_x = unix_to_x(*to_unix);
    let mid_y = rect.center().y;

    // Interaction zones — slightly larger than the visual handles.
    let from_resp = ui.interact(
        egui::Rect::from_center_size(
            egui::pos2(from_x, mid_y),
            egui::vec2(handle_r * 2.5, height),
        ),
        ui.id().with("range_from"),
        egui::Sense::drag(),
    );
    let to_resp = ui.interact(
        egui::Rect::from_center_size(egui::pos2(to_x, mid_y), egui::vec2(handle_r * 2.5, height)),
        ui.id().with("range_to"),
        egui::Sense::drag(),
    );

    let mut changed = false;
    let min_gap = 3_600i64; // 1 hour minimum window
    let oldest_allowed = now - (MAX_DAYS as i64) * 86_400;

    if from_resp.dragged() {
        if let Some(pos) = from_resp.interact_pointer_pos() {
            *from_unix = x_to_unix(pos.x).clamp(oldest_allowed, *to_unix - min_gap);
            changed = true;
        }
    }
    if to_resp.dragged() {
        if let Some(pos) = to_resp.interact_pointer_pos() {
            *to_unix = x_to_unix(pos.x).clamp(*from_unix + min_gap, now);
            changed = true;
        }
    }

    // Recompute positions after potential changes.
    let from_x = unix_to_x(*from_unix);
    let to_x = unix_to_x(*to_unix);

    let painter = ui.painter();

    // Full track.
    let track_rect = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, mid_y),
        egui::vec2(rect.width(), track_h),
    );
    painter.rect_filled(
        track_rect,
        track_h / 2.0,
        egui::Color32::from_rgb(28, 33, 48),
    );

    // Active range fill.
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(from_x, mid_y - track_h / 2.0),
            egui::pos2(to_x, mid_y + track_h / 2.0),
        ),
        0.0,
        egui::Color32::from_rgb(35, 105, 195),
    );

    // Handles.
    let handle_color = |resp: &egui::Response| -> egui::Color32 {
        if resp.dragged() {
            egui::Color32::WHITE
        } else if resp.hovered() {
            egui::Color32::from_rgb(200, 225, 255)
        } else {
            egui::Color32::from_rgb(140, 190, 255)
        }
    };
    painter.circle_filled(
        egui::pos2(from_x, mid_y),
        handle_r,
        handle_color(&from_resp),
    );
    painter.circle_filled(egui::pos2(to_x, mid_y), handle_r, handle_color(&to_resp));

    // "Today" tick at the far-right.
    let today_x = unix_to_x(now);
    painter.line_segment(
        [
            egui::pos2(today_x, mid_y + track_h / 2.0 + 2.0),
            egui::pos2(today_x, mid_y + track_h / 2.0 + 6.0),
        ],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(70, 80, 100)),
    );

    changed
}

// ── Date text inputs ──────────────────────────────────────────────────────────

fn draw_date_inputs(ui: &mut egui::Ui, model: &mut AppModel) -> bool {
    let mut changed = false;
    let now = event_store::now_unix();

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("From")
                .small()
                .color(theme::text_muted()),
        );
        let from_resp = ui.add(
            egui::TextEdit::singleline(&mut model.replay_from_str)
                .desired_width(82.0)
                .font(egui::TextStyle::Small),
        );
        if from_resp.changed() {
            if let Some(unix) = event_store::parse_iso_to_unix(&model.replay_from_str) {
                model.replay_from_unix = unix.clamp(
                    now - (MAX_DAYS as i64) * 86_400,
                    model.replay_to_unix - 3_600,
                );
                changed = true;
            }
        }
        // Reset the buffer to a valid date when the field loses focus.
        if from_resp.lost_focus() {
            model.replay_from_str = event_store::unix_to_date_str(model.replay_from_unix);
        }

        ui.label(egui::RichText::new("→").small().color(theme::text_muted()));

        let to_resp = ui.add(
            egui::TextEdit::singleline(&mut model.replay_to_str)
                .desired_width(82.0)
                .font(egui::TextStyle::Small),
        );
        if to_resp.changed() {
            if let Some(unix) = event_store::parse_iso_to_unix(&model.replay_to_str) {
                model.replay_to_unix = unix.clamp(model.replay_from_unix + 3_600, now);
                changed = true;
            }
        }
        if to_resp.lost_focus() {
            model.replay_to_str = event_store::unix_to_date_str(model.replay_to_unix);
        }
    });

    changed
}

// ── Transport controls ────────────────────────────────────────────────────────

fn draw_transport(ui: &mut egui::Ui, model: &mut AppModel) {
    let finished = model
        .replay_state
        .as_ref()
        .map(|s| s.is_finished())
        .unwrap_or(false);
    let is_paused = model
        .replay_state
        .as_ref()
        .map(|s| s.is_paused())
        .unwrap_or(true);

    ui.horizontal(|ui| {
        let pp_label = if is_paused || finished {
            "▶ Play"
        } else {
            "⏸ Pause"
        };
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

        let rst_btn = egui::Button::new(egui::RichText::new("↺ Restart").small())
            .fill(egui::Color32::TRANSPARENT);
        if ui.add(rst_btn).clicked() {
            if let Some(state) = &mut model.replay_state {
                state.restart();
            }
        }
    });
}
