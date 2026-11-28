//! Live event-follow timing and broadcast director controls within the shared Settings window.
use crate::{model::AppModel, settings_store, theme};

pub(super) fn render(ui: &mut egui::Ui, model: &mut AppModel) {
    ui.heading("Follow Events");
    ui.colored_label(
        theme::text_muted(),
        "Choose how the broadcast director prioritizes stories and frames each stop.",
    );
    ui.small("Every five minutes of live following, a one-minute 24hr recap visits the next region, including a dedicated Middle East recap. Camera feeds and the Brief return after the recap.");
    ui.add_space(8.0);
    let mut seconds = model.event_follow.dwell_seconds();
    if ui
        .add(
            egui::Slider::new(
                &mut seconds,
                settings_store::MIN_EVENT_FOLLOW_DWELL_SECONDS
                    ..=settings_store::MAX_EVENT_FOLLOW_DWELL_SECONDS,
            )
            .logarithmic(true)
            .suffix(" s")
            .text("Base dwell time"),
        )
        .changed()
    {
        model.event_follow.set_dwell_seconds(seconds);
    }

    ui.add_space(8.0);
    ui.checkbox(
        &mut model.follow_director.smart_director,
        "Smart Convergence Director (multi-sensor priority + regional continuity)",
    );
    ui.checkbox(
        &mut model.follow_director.adaptive_framing,
        "Adaptive camera framing (zoom, pitch, orbit radius & dwell pacing by footprint)",
    );
    ui.checkbox(
        &mut model.follow_director.skip_empty_recap_regions,
        "Skip empty regional recap windows",
    );
    ui.add_space(4.0);
    ui.add(
        egui::Slider::new(
            &mut model.follow_director.min_quake_interrupt_mag,
            settings_store::MIN_QUAKE_INTERRUPT_MAG..=settings_store::MAX_QUAKE_INTERRUPT_MAG,
        )
        .fixed_decimals(1)
        .prefix("M")
        .text("Min quake interrupt magnitude"),
    );

    let convergences: Vec<_> = model
        .convergence
        .clusters()
        .iter()
        .filter(|c| c.is_convergence())
        .take(5)
        .collect();
    if !convergences.is_empty() {
        ui.add_space(8.0);
        ui.separator();
        ui.strong(format!("Active Convergences ({})", convergences.len()));
        for cluster in convergences {
            if let Some(badge) = cluster.badge_text() {
                ui.colored_label(theme::topo_color(), badge);
            }
            ui.small(format!(
                "{} · score {:.1}",
                cluster.anchor_event.title, cluster.priority_score
            ));
        }
    }

    ui.add_space(6.0);
    ui.small("Applies to the current stop. Use Save Settings to remember your choice.");
}
