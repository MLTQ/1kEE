use super::{FocusContourSpec, GeoBounds};
use crate::model::GeoPoint;

pub fn feature_budget_for_zoom(zoom: f32) -> usize {
    spec_for_zoom(zoom).feature_budget
}

/// Keep the same per-tile detail while the envelope contains more small cores.
pub fn per_asset_feature_budget(zoom: f32, assets: usize) -> usize {
    let spec = spec_for_zoom(zoom);
    (spec.feature_budget / assets.max(1)).max(if spec.zoom_bucket >= 7 { 10_000 } else { 120 })
}

pub fn half_extent_for_zoom(zoom: f32) -> f32 {
    spec_for_zoom(zoom).half_extent_deg
}

pub fn zoom_bucket_for_zoom(zoom: f32) -> i32 {
    spec_for_zoom(zoom).zoom_bucket
}

pub fn contour_interval_for_zoom(zoom: f32) -> f32 {
    spec_for_zoom(zoom).interval_m
}

/// Maximum hosted core radius. Small tiles preserve detail; one spare ring
/// allows movement while neighboring cores load.
pub const THREEDEP_PREFETCH_RADIUS: i32 = 5;

/// The oblique viewport extends past its nominal half extent.
#[allow(dead_code)]
pub const OBLIQUE_VISIBLE_EXTENT_FACTOR: f32 = 2.5;

pub fn prefetch_radius_for_zoom(zoom: f32, requested: i32) -> i32 {
    let spec = spec_for_zoom(zoom);
    let visible = super::super::local_terrain_scene::visual_half_extent_for_zoom(zoom)
        * OBLIQUE_VISIBLE_EXTENT_FACTOR;
    // Subtracting a half-step center offset from (radius+0.5) cores leaves
    // exactly radius*step guaranteed coverage, even at bucket boundaries.
    let required = (visible / (spec.half_extent_deg * 0.45)).ceil() as i32;
    if spec.zoom_bucket >= FIRST_THREEDEP_BUCKET {
        required
            .saturating_add(1)
            .clamp(1, THREEDEP_PREFETCH_RADIUS)
    } else {
        required.max(requested).clamp(1, 16)
    }
}

/// Guaranteed coverage around a focus anywhere inside its center core.
#[allow(dead_code)]
pub fn region_coverage_half_extent_deg(spec: &FocusContourSpec, radius: i32) -> f32 {
    spec.half_extent_deg * 0.45 * radius as f32
}

/// Buckets at or above this index source from USGS 3DEP rather than SRTM.
/// SRTM's 1 arc-second (~30 m) posting cannot support these intervals.
pub const FIRST_THREEDEP_BUCKET: i32 = 7;

pub fn bucket_radius_for_target_radius_miles(zoom: f32, radius_miles: f32) -> i32 {
    let half_extent_deg = half_extent_for_zoom(zoom);
    let half_extent_km = half_extent_deg * 0.225 * 111.32;
    let bucket_step_km = half_extent_deg * 0.45 * 111.32;
    let target_km = radius_miles * 1.609_34;

    if target_km <= half_extent_km {
        0
    } else {
        (((target_km - half_extent_km) / bucket_step_km).ceil() as i32).clamp(0, 16)
    }
}

pub fn spec_for_zoom(zoom: f32) -> FocusContourSpec {
    if zoom < 1.0 {
        FocusContourSpec {
            half_extent_deg: 3.6,
            raster_size: 384,
            interval_m: 50.0,
            simplify_step: 5,
            feature_budget: 320,
            zoom_bucket: 0,
        }
    } else if zoom < 2.0 {
        FocusContourSpec {
            half_extent_deg: 2.2,
            raster_size: 512,
            interval_m: 25.0,
            simplify_step: 4,
            feature_budget: 360,
            zoom_bucket: 1,
        }
    } else if zoom < 3.0 {
        FocusContourSpec {
            half_extent_deg: 1.4,
            raster_size: 576,
            interval_m: 20.0,
            simplify_step: 4,
            feature_budget: 400,
            zoom_bucket: 2,
        }
    } else if zoom < 4.5 {
        FocusContourSpec {
            half_extent_deg: 0.9,
            raster_size: 640,
            interval_m: 10.0,
            simplify_step: 3,
            feature_budget: 440,
            zoom_bucket: 3,
        }
    } else if zoom < 6.5 {
        FocusContourSpec {
            half_extent_deg: 0.55,
            raster_size: 704,
            interval_m: 10.0,
            simplify_step: 3,
            feature_budget: 480,
            zoom_bucket: 4,
        }
    } else if zoom < 9.5 {
        FocusContourSpec {
            half_extent_deg: 0.3,
            raster_size: 768,
            interval_m: 5.0,
            simplify_step: 2,
            feature_budget: 560,
            zoom_bucket: 5,
        }
    } else if zoom < 13.0 {
        FocusContourSpec {
            half_extent_deg: 0.16,
            raster_size: 896,
            interval_m: 5.0,
            simplify_step: 2,
            feature_budget: 640,
            zoom_bucket: 6,
        }
    // USGS tiers retain their legacy address step and contour interval.
    // CoreTile derives a smaller, aligned source raster from each spec; stored
    // geometry covers only the core. The per-asset reader floor preserves the
    // old detail allowance when the viewport selects more small tiles.
    } else if zoom < 21.0 {
        FocusContourSpec {
            half_extent_deg: 0.210,
            raster_size: 2048,
            interval_m: 5.0,
            simplify_step: 2,
            feature_budget: 250_000,
            zoom_bucket: 7,
        }
    } else if zoom < 31.0 {
        FocusContourSpec {
            half_extent_deg: 0.067,
            raster_size: 2400,
            interval_m: 2.0,
            simplify_step: 2,
            feature_budget: 250_000,
            zoom_bucket: 8,
        }
    } else if zoom < 44.0 {
        FocusContourSpec {
            half_extent_deg: 0.0305,
            raster_size: 2400,
            interval_m: 1.0,
            simplify_step: 2,
            feature_budget: 250_000,
            zoom_bucket: 9,
        }
    } else {
        FocusContourSpec {
            half_extent_deg: 0.0148,
            raster_size: 2400,
            interval_m: 0.5,
            simplify_step: 2,
            feature_budget: 250_000,
            zoom_bucket: 10,
        }
    }
}

/// True when a spec's contours must come from 3DEP.
pub fn spec_uses_threedep(spec: &FocusContourSpec) -> bool {
    spec.zoom_bucket >= FIRST_THREEDEP_BUCKET
}

/// Zoom specs for the lunar (SLDEM2015) contour pipeline.
/// Same tile geometry as SRTM but coarser intervals — lunar relief spans
/// ~20 km (vs ~18 km for Earth) and most features are broad maria or basins.
pub fn lunar_spec_for_zoom(zoom: f32) -> FocusContourSpec {
    if zoom < 1.0 {
        FocusContourSpec {
            half_extent_deg: 3.6,
            raster_size: 384,
            interval_m: 1000.0,
            simplify_step: 5,
            feature_budget: 320,
            zoom_bucket: 0,
        }
    } else if zoom < 2.0 {
        FocusContourSpec {
            half_extent_deg: 2.2,
            raster_size: 512,
            interval_m: 500.0,
            simplify_step: 4,
            feature_budget: 360,
            zoom_bucket: 1,
        }
    } else if zoom < 3.0 {
        FocusContourSpec {
            half_extent_deg: 1.4,
            raster_size: 576,
            interval_m: 200.0,
            simplify_step: 4,
            feature_budget: 400,
            zoom_bucket: 2,
        }
    } else if zoom < 4.5 {
        FocusContourSpec {
            half_extent_deg: 0.9,
            raster_size: 640,
            interval_m: 100.0,
            simplify_step: 3,
            feature_budget: 440,
            zoom_bucket: 3,
        }
    } else {
        FocusContourSpec {
            half_extent_deg: 0.55,
            raster_size: 704,
            interval_m: 50.0,
            simplify_step: 3,
            feature_budget: 480,
            zoom_bucket: 4,
        }
    }
}

/// Zoom specs for the Mars CTX contour pipeline.
pub fn mars_spec_for_zoom(zoom: f32) -> FocusContourSpec {
    // Uses the same zoom tiers as the moon
    lunar_spec_for_zoom(zoom)
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
