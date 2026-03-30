/// Stellar Observatory panel — time controls for the stellar correspondence layer.
///
/// Lets the user pick any epoch from seconds-scale animation to deep prehistory,
/// choose historical presets, animate at configurable speeds, and toggle
/// precession, planets, and orbital trails.

use crate::model::AppModel;
use crate::stellar_time;
use crate::theme;

pub fn render_stellar_observatory(ctx: &egui::Context, model: &mut AppModel) {
    if !model.stellar_observatory_open {
        return;
    }

    let mut open = model.stellar_observatory_open;

    egui::Window::new("Stellar Observatory")
        .open(&mut open)
        .default_size(egui::vec2(460.0, 350.0))
        .min_size(egui::vec2(380.0, 260.0))
        .frame(
            egui::Frame::window(&ctx.style())
                .fill(theme::window_fill())
                .stroke(egui::Stroke::new(1.0, theme::window_stroke())),
        )
        .show(ctx, |ui| {
            ui.colored_label(
                theme::text_muted(),
                "Geographic Position mapping for stars and planets at any epoch.",
            );
            ui.add_space(6.0);

            // ── Epoch display ──────────────────────────────────────────────
            let epoch_str = stellar_time::jd_to_string(model.stellar_jd);
            let jd_str    = format!("JD {:.2}", model.stellar_jd);
            ui.horizontal(|ui| {
                ui.strong("Epoch:");
                ui.monospace(&epoch_str);
                ui.colored_label(theme::text_muted(), &jd_str);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (label, col) = if model.stellar_live {
                        ("● LIVE", theme::hot_color())
                    } else {
                        ("○ MANUAL", theme::text_muted())
                    };
                    if ui
                        .button(egui::RichText::new(label).color(col).small())
                        .on_hover_text("Toggle between real-time tracking and manual epoch control")
                        .clicked()
                    {
                        model.stellar_live = !model.stellar_live;
                        if model.stellar_live {
                            model.stellar_anim_speed = 0.0;
                        }
                    }
                });
            });

            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);

            // ── Historical presets ─────────────────────────────────────────
            ui.colored_label(theme::text_muted(), "HISTORICAL PRESETS");
            ui.add_space(3.0);
            ui.horizontal_wrapped(|ui| {
                let now_jd = stellar_time::unix_to_jd(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs_f64(),
                );
                for (label, jd, tip) in &[
                    ("Now",              now_jd,                             "Current moment"),
                    ("J2000",            stellar_time::epoch::J2000,         "Standard reference epoch 2000-01-01 12:00 TT"),
                    ("Apollo 11",        stellar_time::epoch::APOLLO_11,     "Moon landing: 1969-07-20 20:17 UTC"),
                    ("Trinity 1945",     stellar_time::epoch::TRINITY,       "First nuclear detonation: 1945-07-16"),
                    ("Fall of Rome",     stellar_time::epoch::FALL_OF_ROME,  "Conventional end of the Western Roman Empire: 476 CE"),
                    ("Giza ~2560 BCE",   stellar_time::epoch::GIZA_PYRAMIDS, "Great Pyramid construction era"),
                    ("Sphinx ~2500 BCE", stellar_time::epoch::SPHINX,        "Great Sphinx construction era"),
                    ("Göbekli ~9600 BCE",stellar_time::epoch::GOBEKLI_TEPE,  "Göbekli Tepe — world's oldest known monumental architecture"),
                    ("Lascaux ~17k BCE", stellar_time::epoch::LASCAUX,       "Lascaux cave paintings"),
                ] {
                    if ui
                        .small_button(*label)
                        .on_hover_text(*tip)
                        .clicked()
                    {
                        model.stellar_live      = false;
                        model.stellar_anim_speed = 0.0;
                        model.stellar_jd        = *jd;
                    }
                }
            });

            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);

            // ── Animation controls ─────────────────────────────────────────
            ui.colored_label(theme::text_muted(), "ANIMATION");
            ui.add_space(3.0);
            ui.horizontal_wrapped(|ui| {
                let is_playing = model.stellar_anim_speed != 0.0 && !model.stellar_live;
                let play_label = if is_playing { "⏸ Pause" } else { "▶ Play" };
                if ui.small_button(play_label).clicked() {
                    if is_playing {
                        model.stellar_anim_speed = 0.0;
                    } else {
                        model.stellar_live = false;
                        // Default to 1 day/s if speed was zero
                        if model.stellar_anim_speed == 0.0 {
                            model.stellar_anim_speed = 1.0;
                        }
                    }
                }

                ui.separator();

                // Speed presets (JD per real second)
                for (label, jd_per_sec) in &[
                    ("1 s/s",    1.0 / 86_400.0),
                    ("1 min/s",  1.0 / 1_440.0),
                    ("1 hr/s",   1.0 / 24.0),
                    ("1 day/s",  1.0_f64),
                    ("1 mo/s",   30.437),
                    ("1 yr/s",   365.25),
                    ("100 yr/s", 36_525.0),
                    ("1 ky/s",   365_250.0),
                ] {
                    let active = !model.stellar_live
                        && model.stellar_anim_speed != 0.0
                        && (model.stellar_anim_speed - jd_per_sec).abs()
                            < jd_per_sec.abs() * 0.05;
                    let fill = if active { theme::chrome_active_fill() } else { egui::Color32::TRANSPARENT };
                    let col  = if active { theme::chrome_active_text() } else { theme::text_muted() };
                    let btn  = egui::Button::new(egui::RichText::new(*label).small().color(col))
                        .fill(fill)
                        .corner_radius(3.0);
                    if ui.add(btn).clicked() {
                        model.stellar_live       = false;
                        model.stellar_anim_speed = *jd_per_sec;
                    }
                }
            });

            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);

            // ── Layer toggles ──────────────────────────────────────────────
            ui.colored_label(theme::text_muted(), "LAYERS");
            ui.add_space(3.0);
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut model.stellar_precess, "Precess stars")
                    .on_hover_text(
                        "Rotate star coordinates from J2000.0 to the current epoch using the \
                         IAU 1976 precession model.  Required for correct pole stars, \
                         constellation positions, and zodiac alignment at historical dates.",
                    );
                ui.checkbox(&mut model.show_planets, "Planets")
                    .on_hover_text("Show Sun, Moon, and the eight planets as geographic positions.");
                ui.checkbox(&mut model.show_planet_trails, "Trails")
                    .on_hover_text(
                        "Draw each planet's ground track over time.  Retrograde motion appears \
                         as loops in the path — most dramatic for Mars, Jupiter, and Saturn.",
                    );
            });

            if model.show_planet_trails {
                ui.horizontal(|ui| {
                    ui.label("Trail span:");
                    ui.add(
                        egui::DragValue::new(&mut model.planet_trail_years)
                            .speed(0.1)
                            .range(0.05_f32..=500.0)
                            .suffix(" yr"),
                    )
                    .on_hover_text(
                        "Length of the trail in years, centred on the current epoch. \
                         Suggested: 2 yr for Mars (shows one retrograde arc), \
                         5 yr for Jupiter, 10 yr for Saturn.",
                    );
                    ui.small_button("Auto")
                        .on_hover_text("Reset trail span to the planet-specific default")
                        .clicked()
                        .then(|| model.planet_trail_years = 0.0);
                });
            }

            // ── Precision warning for extreme epochs ───────────────────────
            let t_centuries = stellar_time::j2000_centuries(model.stellar_jd);
            if t_centuries.abs() > 40.0 {
                ui.add_space(6.0);
                let err_deg = (t_centuries.abs() / 40.0).min(15.0);
                ui.colored_label(
                    egui::Color32::from_rgb(210, 165, 55),
                    format!(
                        "⚠  {:.0} centuries from J2000 — planet positions approximate \
                         (±{:.0}° at this range).",
                        t_centuries.abs(),
                        err_deg,
                    ),
                );
            }
        });

    model.stellar_observatory_open = open;
}
