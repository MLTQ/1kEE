use super::{MapTheme, Palette};
use crate::atlas_style::{AtlasStyle, LightMode};
use crate::model::follow_recap::{RECAP_SECONDS, RecapRegion, RegionalRecap};
use crate::model::{ActiveBody, AppModel, GeoPoint};
use crate::stellar_time::{JD_J2000, jd_to_unix};

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
    assert_eq!(Palette::for_view(&model).chrome, MapTheme::Atlas);
    model.globe_view.local_mode = true;
    assert_eq!(Palette::for_view(&model).chrome, MapTheme::Topo);
    model.atlas_style.light_mode = LightMode::Sun;
    assert_eq!(Palette::for_view(&model).chrome, MapTheme::Atlas);
    model.globe_view.local_mode = false;
    let night = Palette::for_view(&model);
    assert_eq!(night.chrome, MapTheme::Topo);
    assert_eq!(night.map, MapTheme::Atlas);

    model.active_body = ActiveBody::Moon;
    assert_eq!(Palette::for_view(&model).chrome, MapTheme::Atlas);
    model.active_body = ActiveBody::Earth;
    model.map_theme = MapTheme::Ghost;
    assert_eq!(Palette::for_view(&model).chrome, MapTheme::Ghost);
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
    for (seconds, expected) in [
        (0., MapTheme::Atlas),
        (RECAP_SECONDS / 2.0, MapTheme::Topo),
        (RECAP_SECONDS, MapTheme::Atlas),
    ] {
        model
            .follow_recaps
            .active
            .as_mut()
            .unwrap()
            .replay
            .advance_to(seconds);
        assert_eq!(Palette::for_view(&model).chrome, expected);
    }
}
