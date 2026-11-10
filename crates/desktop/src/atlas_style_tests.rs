use super::*;

#[test]
fn defaults_and_partial_settings_preserve_the_atlas_light() {
    let old: AtlasStyle = serde_json::from_str("{}").unwrap();
    assert_eq!(old, AtlasStyle::default());
    let settings: crate::settings_store::AppSettings = serde_json::from_str("{}").unwrap();
    assert_eq!(settings.atlas_style, old);
    let style: AtlasStyle =
        serde_json::from_str(r#"{"azimuth_deg":90,"color_richness":1.3}"#).unwrap();
    assert_eq!(style.altitude_deg, 35.0);
    let mut settings = settings;
    settings.atlas_style = style;
    let restored: crate::settings_store::AppSettings =
        serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
    assert_eq!(restored.atlas_style, style);
    assert_eq!(
        serde_json::from_str::<AtlasStyle>(&serde_json::to_string(&style).unwrap()).unwrap(),
        style
    );
    let light = old.light(0.0);
    assert!(light[0] < 0.0 && light[1] > 0.0 && light[2] > 0.0);
}

#[test]
fn invalid_controls_are_bounded_before_gpu_use() {
    let style = AtlasStyle {
        azimuth_deg: -90.0,
        altitude_deg: f32::NAN,
        color_richness: f32::INFINITY,
        relief_strength: 99.0,
        time_offset_hours: -20.0,
        ..Default::default()
    }
    .normalized();
    assert_eq!(style.azimuth_deg, 270.0);
    assert_eq!(style.altitude_deg, 35.0);
    assert_eq!(style.color_richness, 1.0);
    assert_eq!(style.relief_strength, 2.0);
    assert_eq!(style.time_offset_hours, -12.0);
}

#[test]
fn sunlight_moves_with_time_and_reuses_the_app_solar_position() {
    let style = AtlasStyle {
        light_mode: LightMode::Sun,
        ..Default::default()
    };
    let jd = crate::stellar_time::JD_J2000;
    let noon = style.light(jd);
    let midnight = style.light(jd + 0.5);
    assert!(
        noon[..3]
            .iter()
            .zip(midnight)
            .map(|(a, b)| a * b)
            .sum::<f32>()
            < -0.6
    );
    assert!((noon[..3].iter().map(|v| v * v).sum::<f32>() - 1.0).abs() < 0.00001);
    let offset = AtlasStyle {
        time_offset_hours: 12.0,
        ..style
    };
    assert_eq!(offset.light(jd), midnight);
    assert_eq!(style.light(jd), style.light(jd + 1.0 / 86400.0));
}

#[test]
fn polar_day_and_night_depend_on_latitude_and_season() {
    use crate::model::GeoPoint;
    let style = AtlasStyle::default();
    let north = GeoPoint {
        lat: 85.0,
        lon: 0.0,
    };
    let south = GeoPoint {
        lat: -85.0,
        lon: 0.0,
    };
    let winter = crate::stellar_time::JD_J2000;
    let summer = winter + 182.0;
    for day_fraction in [0.0, 0.5] {
        assert!(style.is_night_at(winter + day_fraction, north));
        assert!(!style.is_night_at(winter + day_fraction, south));
        assert!(!style.is_night_at(summer + day_fraction, north));
        assert!(style.is_night_at(summer + day_fraction, south));
    }
}
