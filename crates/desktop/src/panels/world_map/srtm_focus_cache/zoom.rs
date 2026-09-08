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

pub fn contour_interval_for_zoom(zoom: f32) -> f32 {
    spec_for_zoom(zoom).interval_m
}

/// Deep 3DEP tiers stream their source raster over the network, so their
/// prefetch envelope is tightened. Their bucket step is small enough that a
/// radius of 2 still covers roughly 1.9x the visible half-width.
pub fn prefetch_radius_for_zoom(zoom: f32, requested: i32) -> i32 {
    if spec_for_zoom(zoom).zoom_bucket >= FIRST_THREEDEP_BUCKET {
        requested.min(2)
    } else {
        requested
    }
}

/// Buckets at or above this index source from USGS 3DEP rather than SRTM.
/// SRTM's 1 arc-second (~30 m) posting cannot support these intervals.
pub const FIRST_THREEDEP_BUCKET: i32 = 7;

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
    // ── USGS 3DEP tiers ──────────────────────────────────────────────────
    //
    // Past this point SRTM's ~1 arc-second posting is the limit, not the
    // raster size, so these buckets stream 1 m bare-earth 3DEP instead.
    // Each `half_extent_deg` is chosen to match `visual_half_extent_for_zoom`
    // at the tier's opening zoom, so a prefetch radius of 2 already covers
    // ~1.9x the visible half-width. That keeps the envelope at 5x5 tiles
    // rather than the 13x13 the local SRTM tiers use, because every one of
    // these tiles costs a network request rather than a local warp.
    } else if zoom < 22.0 {
        FocusContourSpec {
            half_extent_deg: 0.09,
            raster_size: 1536,
            interval_m: 5.0,
            simplify_step: 2,
            feature_budget: 700,
            zoom_bucket: 7,
        }
    } else if zoom < 32.0 {
        FocusContourSpec {
            half_extent_deg: 0.035,
            raster_size: 1536,
            interval_m: 2.0,
            simplify_step: 2,
            feature_budget: 900,
            zoom_bucket: 8,
        }
    } else if zoom < 44.0 {
        FocusContourSpec {
            half_extent_deg: 0.014,
            raster_size: 1792,
            interval_m: 1.0,
            simplify_step: 1,
            feature_budget: 1100,
            zoom_bucket: 9,
        }
    } else {
        FocusContourSpec {
            half_extent_deg: 0.006,
            raster_size: 1792,
            interval_m: 0.5,
            simplify_step: 1,
            feature_budget: 1300,
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
