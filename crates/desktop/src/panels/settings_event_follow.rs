//! Live event-follow timing controls within the shared Settings window.
use crate::{model::AppModel, settings_store, theme};

pub(super) fn render(ui: &mut egui::Ui, model: &mut AppModel) {
    ui.heading("Follow Events");
    ui.colored_label(
        theme::text_muted(),
        "Choose how long to stay at each event after arrival.",
    );
    ui.small("Every five minutes of live following, a one-minute 24hr recap visits the next continent. Camera feeds and the Brief return after the recap.");
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
            .text("Dwell time"),
        )
        .changed()
    {
        model.event_follow.set_dwell_seconds(seconds);
    }
    ui.small("Applies to the current stop. Use Save Settings to remember your choice.");
}
