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

/// Envelope radius the 3DEP tiers use. Every one of their tiles is a network
/// request, so they take a 5x5 grid rather than the local SRTM tiers' 13x13;
/// their `half_extent_deg` is sized against this radius to still cover the
/// oblique viewport. Changing this without resizing the tiers shrinks what the
/// scene can draw.
pub const THREEDEP_PREFETCH_RADIUS: i32 = 2;

/// Fraction of `visual_half_extent_for_zoom` the oblique camera can actually
/// show, matching the local marker cull distance. A tier that covers less than
/// this leaves visible ground blank.
///
/// Only the coverage test reads this; it exists so the sizing rule the 3DEP
/// tiers were built against is stated once rather than duplicated as a magic
/// number in the spec comments and the test.
#[allow(dead_code)]
pub const OBLIQUE_VISIBLE_EXTENT_FACTOR: f32 = 2.5;

pub fn prefetch_radius_for_zoom(zoom: f32, requested: i32) -> i32 {
    if spec_for_zoom(zoom).zoom_bucket >= FIRST_THREEDEP_BUCKET {
        requested.min(THREEDEP_PREFETCH_RADIUS)
    } else {
        requested
    }
}

/// Half-width in degrees a region of `radius` tiles of `spec` actually covers.
/// Tiles sit `half_extent * 0.45` apart, so the envelope reaches the outermost
/// bucket centre plus that tile's own half extent.
///
/// Read only by the coverage test, for the same reason as the constant above.
#[allow(dead_code)]
pub fn region_coverage_half_extent_deg(spec: &FocusContourSpec, radius: i32) -> f32 {
    spec.half_extent_deg * (1.0 + 0.45 * radius as f32)
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
    //
    // These tiers carry far more geometry than the SRTM ones: a single
    // bucket-10 tile is ~4.3 M points, where a whole SRTM tile is a small
    // fraction of that. Two consequences.
    //
    // `feature_budget` is 7 500 across a 25-tile envelope, so 300 contours per
    // tile. That looks small next to the raw count, but the reader now keeps
    // the *longest* contours rather than every Nth: 300 longest hold roughly a
    // third of a tile's geometry, where 300 strided held about 3%. It is also
    // what keeps memory sane — the envelope holds ~150 MB of points at this
    // budget and ~400 MB at 2 500 per tile, to feed a renderer that draws at
    // most 1 M points a frame.
    //
    // `simplify_step` is 2 throughout because source vertex spacing is already
    // below one screen pixel at these scales, so halving the points costs
    // nothing visible.
    //
    // Sizing rule, enforced by `threedep_tiers_cover_the_oblique_viewport` in
    // `local_terrain_scene`: the oblique camera sees ground out to about 2.5x
    // `visual_half_extent_for_zoom` — the same distance the local marker cull
    // uses — so a tier must satisfy
    //
    //     half_extent * (1 + 0.45 * radius)  >=  2.5 * visual half extent
    //
    // at its opening zoom, where the visual extent is largest. Because every
    // one of these tiles costs a network request, the envelope stays at the
    // 5x5 grid `prefetch_radius_for_zoom` allows and the coverage comes from
    // larger tiles instead. Fewer, wider requests beat more, narrower ones:
    // radius 3 would need only slightly smaller tiles but twice the downloads.
    } else if zoom < 21.0 {
        FocusContourSpec {
            half_extent_deg: 0.210,
            raster_size: 2048,
            interval_m: 5.0,
            simplify_step: 2,
            feature_budget: 7_500,
            zoom_bucket: 7,
        }
    } else if zoom < 31.0 {
        FocusContourSpec {
            half_extent_deg: 0.067,
            raster_size: 2400,
            interval_m: 2.0,
            simplify_step: 2,
            feature_budget: 7_500,
            zoom_bucket: 8,
        }
    } else if zoom < 44.0 {
        FocusContourSpec {
            half_extent_deg: 0.0305,
            raster_size: 2400,
            interval_m: 1.0,
            simplify_step: 2,
            feature_budget: 7_500,
            zoom_bucket: 9,
        }
    } else {
        FocusContourSpec {
            half_extent_deg: 0.0148,
            raster_size: 2400,
            interval_m: 0.5,
            simplify_step: 2,
            feature_budget: 7_500,
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
