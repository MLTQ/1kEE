use super::{MapTheme, Palette};
use crate::atlas_style::{AtlasStyle, LightMode};
use crate::model::follow_recap::{RECAP_SECONDS, RecapRegion, RegionalRecap};
use crate::model::{ActiveBody, AppModel, GeoPoint};
use crate::stellar_time::{JD_J2000, jd_to_unix};

#[test]
fn atlas_fade_reverses_from_the_visible_mix_and_reaches_both_endpoints() {
    let ctx = egui::Context::default();
    let sample = |time, target, already_in_atlas| {
        let mut mix = 0.0;
        let input = egui::RawInput {
            time: Some(time),
            predicted_dt: 0.0,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            mix = super::fade::night_mix(ctx, target, already_in_atlas)
        });
        mix
    };
    assert_eq!(sample(0.0, 0.0, false), 0.0);
    assert_eq!(sample(10.0, 1.0, true), 0.0);
    let halfway = sample(10.4, 1.0, true);
    assert!((halfway - 0.5).abs() < 0.001);
    assert_eq!(sample(10.4, 0.0, true), halfway);
    let returning = sample(10.6, 0.0, true);
    assert!(returning > 0.0 && returning < halfway);
    assert_eq!(sample(11.3, 0.0, true), 0.0);
    sample(12.0, 1.0, true);
    assert_eq!(sample(13.0, 1.0, true), 1.0);
    assert_eq!(sample(14.0, 0.0, false), 0.0);
    assert_eq!(sample(14.1, 0.0, true), 0.0);
}

#[test]
fn atlas_fade_blends_windows_widgets_and_custom_panel_fills() {
    let day = super::visuals_for(MapTheme::Atlas);
    let night = super::visuals_for(MapTheme::Topo);
    let palette = Palette {
        map: MapTheme::Atlas,
        night_mix: 0.5,
    };
    let halfway = palette.visuals();
    assert_eq!(
        halfway.window_fill,
        day.window_fill.lerp_to_gamma(night.window_fill, 0.5)
    );
    assert_eq!(
        halfway.widgets.hovered.weak_bg_fill,
        day.widgets
            .hovered
            .weak_bg_fill
            .lerp_to_gamma(night.widgets.hovered.weak_bg_fill, 0.5)
    );
    let panel = |theme| super::panel_fill_for(theme, 180);
    assert_eq!(
        palette.chrome_color(panel),
        panel(MapTheme::Atlas).lerp_to_gamma(panel(MapTheme::Topo), 0.5)
    );
    assert_eq!(
        Palette {
            night_mix: 0.0,
            ..palette
        }
        .visuals(),
        day
    );
    assert_eq!(
        Palette {
            night_mix: 1.0,
            ..palette
        }
        .visuals(),
        night
    );
}

#[test]
fn atlas_chrome_follows_the_view_center_and_only_the_active_sun_offset() {
    let mut model = AppModel::new();
    model.map_theme = MapTheme::Atlas;
    model.stellar_jd = JD_J2000; // Noon UTC, early January.
    model.atlas_style = AtlasStyle {
        time_offset_hours: 12.0,
        ..Default::default()
    };
    model.globe_view.focus_on(GeoPoint { lat: 0., lon: 0. });
    model.globe_view.local_center = GeoPoint { lat: 0., lon: 180. };
    assert_eq!(Palette::for_view(&model).night_mix, 0.0);
    model.globe_view.local_mode = true;
    assert_eq!(Palette::for_view(&model).night_mix, 1.0);
    model.atlas_style.light_mode = LightMode::Sun;
    assert_eq!(Palette::for_view(&model).night_mix, 0.0);
    model.globe_view.local_mode = false;
    let night = Palette::for_view(&model);
    assert_eq!(night.night_mix, 1.0);
    assert_eq!(night.map, MapTheme::Atlas);

    model.active_body = ActiveBody::Moon;
    assert_eq!(Palette::for_view(&model).night_mix, 0.0);
    model.active_body = ActiveBody::Earth;
    model.map_theme = MapTheme::Ghost;
    assert_eq!(Palette::for_view(&model).map, MapTheme::Ghost);
}

#[test]
fn atlas_chrome_tracks_sunset_and_sunrise_during_a_recap() {
    let mut model = AppModel::new();
    model.map_theme = MapTheme::Atlas;
    model.stellar_jd = JD_J2000;
    model.atlas_style = AtlasStyle::default();
    model.globe_view.focus_on(GeoPoint { lat: 0., lon: 0. });
    model.follow_recaps.active = Some(RegionalRecap::with_events(
        RecapRegion::Europe,
        0.0,
        jd_to_unix(JD_J2000) as i64,
        model.globe_view,
        Vec::new(),
    ));
    for (seconds, expected) in [(0., 0.0), (RECAP_SECONDS / 2.0, 1.0), (RECAP_SECONDS, 0.0)] {
        model
            .follow_recaps
            .active
            .as_mut()
            .unwrap()
            .replay
            .advance_to(seconds);
        assert_eq!(Palette::for_view(&model).night_mix, expected);
    }
}
