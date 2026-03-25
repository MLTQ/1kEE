use super::{FocusContourSpec, GeoBounds};
use crate::model::GeoPoint;

pub fn feature_budget_for_zoom(zoom: f32) -> usize {
    spec_for_zoom(zoom).feature_budget
}

pub fn half_extent_for_zoom(zoom: f32) -> f32 {
    spec_for_zoom(zoom).half_extent_deg
}

pub fn zoom_bucket_for_zoom(zoom: f32) -> i32 {
    spec_for_zoom(zoom).zoom_bucket
}

pub fn contour_interval_for_zoom(zoom: f32) -> i32 {
    spec_for_zoom(zoom).interval_m
}

pub fn bucket_radius_for_target_radius_miles(zoom: f32, radius_miles: f32) -> i32 {
    let half_extent_deg = half_extent_for_zoom(zoom);
    let half_extent_km = half_extent_deg * 111.32;
    let bucket_step_km = half_extent_deg * 0.45 * 111.32;
    let target_km = radius_miles * 1.609_34;

    if target_km <= half_extent_km {
        0
    } else {
        (((target_km - half_extent_km) / bucket_step_km).ceil() as i32).clamp(0, 8)
    }
}

pub fn spec_for_zoom(zoom: f32) -> FocusContourSpec {
    if zoom < 1.0 {
        FocusContourSpec {
            half_extent_deg: 3.6,
            raster_size: 384,
            interval_m: 50,
            simplify_step: 5,
            feature_budget: 320,
            zoom_bucket: 0,
        }
    } else if zoom < 2.0 {
        FocusContourSpec {
            half_extent_deg: 2.2,
            raster_size: 512,
            interval_m: 25,
            simplify_step: 4,
            feature_budget: 360,
            zoom_bucket: 1,
        }
    } else if zoom < 3.0 {
        FocusContourSpec {
            half_extent_deg: 1.4,
            raster_size: 576,
            interval_m: 20,
            simplify_step: 4,
            feature_budget: 400,
            zoom_bucket: 2,
        }
    } else if zoom < 4.5 {
        FocusContourSpec {
            half_extent_deg: 0.9,
            raster_size: 640,
            interval_m: 10,
            simplify_step: 3,
            feature_budget: 440,
            zoom_bucket: 3,
        }
    } else if zoom < 6.5 {
        FocusContourSpec {
            half_extent_deg: 0.55,
            raster_size: 704,
            interval_m: 10,
            simplify_step: 3,
            feature_budget: 480,
            zoom_bucket: 4,
        }
    } else if zoom < 9.5 {
        FocusContourSpec {
            half_extent_deg: 0.3,
            raster_size: 768,
            interval_m: 5,
            simplify_step: 2,
            feature_budget: 560,
            zoom_bucket: 5,
        }
    } else {
        FocusContourSpec {
            half_extent_deg: 0.16,
            raster_size: 896,
            interval_m: 5,
            simplify_step: 2,
            feature_budget: 640,
            zoom_bucket: 6,
        }
    }
}

impl GeoBounds {
    pub fn around(focus: GeoPoint, half_extent_deg: f32) -> Self {
        Self {
            min_lat: (focus.lat - half_extent_deg).clamp(-89.999, 89.999),
            max_lat: (focus.lat + half_extent_deg).clamp(-89.999, 89.999),
            min_lon: focus.lon - half_extent_deg,
            max_lon: focus.lon + half_extent_deg,
        }
    }
}
