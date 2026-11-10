use egui::{Color32, Context, Id, Visuals, style::WidgetVisuals};

const FADE_SECONDS: f32 = 0.8;

pub(super) fn night_mix(ctx: &Context, target: f32, already_in_atlas: bool) -> f32 {
    let id = Id::new("atlas-day-night");
    if !already_in_atlas {
        // Seed the animation at the viewed region when entering Atlas.
        ctx.animate_value_with_time(id, target, 0.0);
        return target;
    }
    let t = ctx.animate_value_with_time(id, target, FADE_SECONDS);
    egui::emath::easing::quadratic_in_out(t)
}

// Change text polarity over a shorter interval so labels spend less time
// close to the brightness of the fading panels.
pub(super) fn ink_mix(t: f32) -> f32 {
    egui::emath::remap_clamp(t, 0.4..=0.6, 0.0..=1.0)
}

pub(super) fn visuals(mut day: Visuals, night: Visuals, t: f32) -> Visuals {
    let fill = |a: Color32, b| a.lerp_to_gamma(b, t);
    let ink = |a: Color32, b| a.lerp_to_gamma(b, ink_mix(t));
    let text = ink(day.text_color(), night.text_color());
    let widget = |a: &mut WidgetVisuals, b: WidgetVisuals| {
        a.bg_fill = fill(a.bg_fill, b.bg_fill);
        a.weak_bg_fill = fill(a.weak_bg_fill, b.weak_bg_fill);
        a.bg_stroke.color = fill(a.bg_stroke.color, b.bg_stroke.color);
        // Atlas overrides widget text with one ink. Topo uses each widget's
        // own foreground. Make them coincide where the override is released.
        let endpoint = if t < 0.5 {
            a.fg_stroke.color
        } else {
            b.fg_stroke.color
        };
        a.fg_stroke.color = text.lerp_to_gamma(endpoint, (2.0 * t - 1.0).abs());
    };
    day.dark_mode = t >= 0.5;
    day.override_text_color = (t < 0.5).then_some(text);
    day.panel_fill = fill(day.panel_fill, night.panel_fill);
    day.window_fill = fill(day.window_fill, night.window_fill);
    day.window_stroke.color = fill(day.window_stroke.color, night.window_stroke.color);
    day.window_shadow.color = fill(day.window_shadow.color, night.window_shadow.color);
    day.popup_shadow.color = fill(day.popup_shadow.color, night.popup_shadow.color);
    day.faint_bg_color = fill(day.faint_bg_color, night.faint_bg_color);
    day.extreme_bg_color = fill(day.extreme_bg_color, night.extreme_bg_color);
    day.code_bg_color = fill(day.code_bg_color, night.code_bg_color);
    day.selection.bg_fill = fill(day.selection.bg_fill, night.selection.bg_fill);
    day.selection.stroke.color = ink(day.selection.stroke.color, night.selection.stroke.color);
    day.text_cursor.stroke.color =
        ink(day.text_cursor.stroke.color, night.text_cursor.stroke.color);
    day.hyperlink_color = ink(day.hyperlink_color, night.hyperlink_color);
    day.warn_fg_color = ink(day.warn_fg_color, night.warn_fg_color);
    day.error_fg_color = ink(day.error_fg_color, night.error_fg_color);
    widget(
        &mut day.widgets.noninteractive,
        night.widgets.noninteractive,
    );
    widget(&mut day.widgets.inactive, night.widgets.inactive);
    widget(&mut day.widgets.hovered, night.widgets.hovered);
    widget(&mut day.widgets.active, night.widgets.active);
    widget(&mut day.widgets.open, night.widgets.open);
    day
}
