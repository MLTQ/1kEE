const PI: f32 = 3.14159265358979;
// Keep the shaded globe subordinate to native contours and map overlays.
// This is a linear-light exposure, independent of the theme's line palette.
const BACKDROP_EXPOSURE: f32 = 0.12;

// ── Terrain helpers ─────────────────────────────────────────────────────────

fn gauss(lat: f32, lon: f32, lat0: f32, lon0: f32, sigma: f32) -> f32 {
    let dlat = lat - lat0;
    var dlon = lon - lon0;
    if dlon > 180.0  { dlon = dlon - 360.0; }
    if dlon < -180.0 { dlon = dlon + 360.0; }
    return exp(-(dlat * dlat + dlon * dlon) / (2.0 * sigma * sigma));
}

/// Synthetic terrain elevation that replicates `terrain_field::elevation`.
/// Returns a value in [0, 1.6].
fn terrain_elev(lat_deg: f32, lon_deg: f32) -> f32 {
    let lat = lat_deg * (PI / 180.0);
    let lon = lon_deg * (PI / 180.0);

    let h = 0.42
        + 0.18 * sin(lat * 2.1) * cos(lon * 1.8)
        + 0.11 * sin(lat * 5.4 + lon * 1.3)
        + 0.08 * cos(lon * 3.7 - lat * 2.2)
        + 0.05 * sin(lon * 9.1 + lat * 7.3);

    var m = 0.0;
    m += gauss(lat_deg, lon_deg,  27.7,   86.9,  10.5) * 1.00; // Himalayas
    m += gauss(lat_deg, lon_deg, -32.7,  -70.1,  11.5) * 0.90; // Andes
    m += gauss(lat_deg, lon_deg,  46.5,   10.2,   6.5) * 0.55; // Alps
    m += gauss(lat_deg, lon_deg,  35.4,  138.7,   4.6) * 0.48; // Japan
    m += gauss(lat_deg, lon_deg,  61.0, -149.0,   7.2) * 0.52; // Alaska
    m += gauss(lat_deg, lon_deg, -43.6,  170.2,   5.6) * 0.42; // New Zealand

    return clamp(h + m, 0.0, 1.6);
}

/// Map normalised elevation [0..1] to a surface colour using the theme palette.
/// Blends ocean → land → mountain in linear space.
fn surface_color(e_norm: f32) -> vec3<f32> {
    let ocean = u.ocean_col.rgb;
    let land  = u.land_col.rgb;
    let mount = u.mount_col.rgb;

    if e_norm < 0.24 {
        // Deep ocean / below-sea-level
        return ocean;
    } else if e_norm < 0.46 {
        let t = (e_norm - 0.24) / 0.22;
        return mix(ocean, land, t);
    } else if e_norm < 0.72 {
        return land;
    } else {
        let t = clamp((e_norm - 0.72) / 0.28, 0.0, 1.0);
        return mix(land, mount, t);
    }
}

// ── Lunar terrain helpers ───────────────────────────────────────────────────

/// Synthetic lunar elevation for the globe backdrop — returns [0, 1].
/// Models major near-side mare (dark basaltic depressions) as negative
/// Gaussians subtracted from a uniform highland baseline.
fn lunar_terrain_elev(lat_deg: f32, lon_deg: f32) -> f32 {
    // Default: heavily-cratered highland at ~0.72 normalised height.
    var h = 0.72f;

    // Near-side maria — large basaltic plains (depressions relative to mean sphere)
    h -= gauss(lat_deg, lon_deg,  20.0,  -57.0, 22.0) * 0.52; // Oceanus Procellarum (huge)
    h -= gauss(lat_deg, lon_deg,  32.0,  -17.0,  9.5) * 0.46; // Mare Imbrium
    h -= gauss(lat_deg, lon_deg,   8.0,   30.0,  7.5) * 0.40; // Mare Tranquillitatis
    h -= gauss(lat_deg, lon_deg,  27.0,   19.0,  6.5) * 0.36; // Mare Serenitatis
    h -= gauss(lat_deg, lon_deg,  17.0,   59.0,  4.5) * 0.32; // Mare Crisium
    h -= gauss(lat_deg, lon_deg, -21.0,  -15.0,  5.5) * 0.30; // Mare Nubium
    h -= gauss(lat_deg, lon_deg,  -4.0,   52.0,  5.5) * 0.28; // Mare Fecunditatis
    h -= gauss(lat_deg, lon_deg, -24.0,  -38.0,  4.0) * 0.26; // Mare Humorum
    h -= gauss(lat_deg, lon_deg,  14.0,    3.0,  3.5) * 0.22; // Mare Vaporum
    h -= gauss(lat_deg, lon_deg,  45.0,  -30.0,  5.0) * 0.20; // Sinus Iridum area

    // South Pole-Aitken basin (far side, very large and deep)
    h -= gauss(lat_deg, lon_deg, -56.0,  180.0, 18.0) * 0.38;

    return clamp(h, 0.0, 1.0);
}

/// Surface colour for the Moon: maps elevation [0,1] to a greyscale regolith palette.
fn surface_color_moon(e: f32) -> vec3<f32> {
    let mare     = u.ocean_col.rgb;    // dark basalt  (low elevation)
    let regolith = u.land_col.rgb;     // mid regolith
    let highland = u.mount_col.rgb;    // bright highland (high elevation)

    if e < 0.38 {
        let t = e / 0.38;
        return mix(mare, regolith, t);
    } else {
        let t = clamp((e - 0.38) / 0.62, 0.0, 1.0);
        return mix(regolith, highland, t);
    }
}

/// Mars synthetic terrain for the globe backdrop.
fn mars_terrain_elev(lat_deg: f32, lon_deg: f32) -> f32 {
    // Default base is intermediate between earth continents and moon.
    var h = 0.65f;

    // Valles Marineris and Hellas Planitia
    h -= gauss(lat_deg, lon_deg, -42.5,  70.5, 15.0) * 0.40; // Hellas basin
    h -= gauss(lat_deg, lon_deg, -10.0, -70.0,  8.0) * 0.35; // Valles Marineris (approx)

    // Tharsis ridge and Olympus Mons
    h += gauss(lat_deg, lon_deg,   0.0, -105.0, 18.0) * 0.45; // Tharsis bulge
    h += gauss(lat_deg, lon_deg,  18.6, -133.8,  5.0) * 0.35; // Olympus Mons

    return clamp(h, 0.0, 1.2);
}

/// Surface colour for Mars: maps elevation [0,1] to red/orange dusty palette.
fn surface_color_mars(e: f32) -> vec3<f32> {
    let basin = u.ocean_col.rgb;
    let plain = u.land_col.rgb;
    let rise  = u.mount_col.rgb;

    if e < 0.3 {
        let t = e / 0.3;
        return mix(basin, plain, t);
    } else {
        let t = clamp((e - 0.3) / 0.7, 0.0, 1.0);
        return mix(plain, rise, t);
    }
}

// ── Graticule ───────────────────────────────────────────────────────────────

/// α (0..1) for a graticule line centred at distance 0.
/// `half_w` is the half-width of the line in the same unit as `dist`.
fn line_alpha(dist: f32, half_w: f32) -> f32 {
    return 1.0 - smoothstep(0.0, half_w * 2.0, dist);
}

/// Distance to the nearest multiple of `step` (wrapping-aware for longitude).
fn deg_dist_to_step(val: f32, step: f32) -> f32 {
    let n = val / step;
    return abs(n - round(n)) * step;
}

/// Composite the graticule contribution into `base_rgb`.
fn apply_graticule(
    base: vec3<f32>,
    lat_deg: f32,
    lon_deg: f32,
    half_w_lat: f32,   // half-line-width in latitude degrees
    half_w_lon: f32,   // half-line-width in longitude degrees (already / cos(lat))
) -> vec3<f32> {
    var rgb = base;

    // ── Grid step based on zoom (radius as proxy) ──────────────────────────
    //   radius < 120 px → 30°,  < 300 px → 15°,  ≥ 300 px → 10°
    let gs = select(
        select(30.0f, 15.0f, u.radius >= 120.0),
        10.0f,
        u.radius >= 300.0
    );

    // ── Special latitude parallels (equator, tropics, polar circles) ───────
    let specials = array<f32, 5>(0.0, 23.44, -23.44, 66.56, -66.56);
    for (var i = 0u; i < 5u; i++) {
        let a = line_alpha(abs(lat_deg - specials[i]), half_w_lat * 1.6) * 0.78;
        rgb = mix(rgb, u.hot_col.rgb, a);
    }

    // ── Prime meridian ─────────────────────────────────────────────────────
    {
        let a = line_alpha(abs(lon_deg), half_w_lon * 1.6) * 0.78;
        rgb = mix(rgb, u.hot_col.rgb, a);
    }

    // ── Major lat/lon lines (30° grid) ─────────────────────────────────────
    {
        let major_col = u.grid_col.rgb * 1.8;
        let lat_a = line_alpha(deg_dist_to_step(lat_deg, 30.0), half_w_lat) * 0.55;
        rgb = mix(rgb, major_col, lat_a);

        let lon_a = line_alpha(deg_dist_to_step(lon_deg, 30.0), half_w_lon) * 0.55;
        rgb = mix(rgb, major_col, lon_a);
    }

    // ── Minor lat/lon lines (grid_step) ────────────────────────────────────
    if gs < 30.0 {
        let minor_col = u.grid_col.rgb;
        let lat_a = line_alpha(deg_dist_to_step(lat_deg, gs), half_w_lat) * 0.30;
        rgb = mix(rgb, minor_col, lat_a);

        let lon_a = line_alpha(deg_dist_to_step(lon_deg, gs), half_w_lon) * 0.30;
        rgb = mix(rgb, minor_col, lon_a);
    }

    return rgb;
}

// Shared by the desktop and OpenXR cameras. Return a linear-light color.
fn shade_globe(geo: vec3<f32>, facing: f32) -> vec3<f32> {
    let x2 = geo.x;
    let y2 = geo.y;
    let z2 = geo.z;
    // ── Geographic coordinates ─────────────────────────────────────────────
    let lat_rad = asin(clamp(y2, -1.0, 1.0));
    let lon_rad = atan2(z2, x2);
    let lat_deg = lat_rad * (180.0 / PI);
    let lon_deg = lon_rad * (180.0 / PI);

    // ── Terrain elevation & base colour ───────────────────────────────────
    var rgb: vec3<f32>;
    let rim = 1.0 - clamp(facing, 0.0, 1.0);
    let sun     = normalize(vec3<f32>(0.32, 0.55, 0.77));
    let shade   = clamp(dot(vec3<f32>(x2, y2, z2), sun), 0.0, 1.0);

    if u.active_body == 1u { // Moon
        let elev  = lunar_terrain_elev(lat_deg, lon_deg);
        rgb = surface_color_moon(elev);
        rgb = rgb * (0.08 + 0.92 * shade);
        rgb = rgb * (1.0 - pow(rim, 4.0) * 0.65);
    } else if u.active_body == 2u { // Mars
        let elev  = mars_terrain_elev(lat_deg, lon_deg);
        let e_norm = elev / 1.2;
        rgb = surface_color_mars(e_norm);
        rgb = rgb * (0.15 + 0.85 * shade);
        // Extremely thin / weak atmosphere edge.
        rgb = rgb * (1.0 - pow(rim, 4.0) * 0.40);
    } else {
        let elev   = terrain_elev(lat_deg, lon_deg);
        let e_norm = elev / 1.6;
        rgb = surface_color(e_norm);

        // ── Lambertian shading ─────────────────────────────────────────────
        rgb = rgb * (0.30 + 0.70 * shade);

        // ── Rim atmosphere (view-space fresnel) ───────────────────────────
        let atm = pow(rim, 3.0) * 0.55;
        rgb = mix(rgb, u.ocean_col.rgb * 0.4, atm);
    }

    // ── Graticule ──────────────────────────────────────────────────────────
    if u.show_graticule != 0u {
        // Approximate degrees-per-pixel at this lat/lon.
        // At the globe centre, 1 logical pixel ≈ (180/π) / radius degrees.
        let deg_per_px  = (180.0 / PI) / u.radius;
        let cos_lat     = max(cos(lat_rad), 0.05); // avoid /0 near poles
        let half_w_lat  = deg_per_px * 1.1;
        let half_w_lon  = deg_per_px * 1.1 / cos_lat;

        rgb = apply_graticule(rgb, lat_deg, lon_deg, half_w_lat, half_w_lon);
    }

    // ── Sphere-edge stroke (replaces circle_stroke in draw_backdrop) ───────
    // Thin brightening at the very limb.
    let edge_a = smoothstep(0.92, 1.0, rim) * 0.55;
    rgb = mix(rgb, u.mount_col.rgb * 0.85, edge_a);

    // Fully opaque inside the sphere (premultiplied: colour*1, alpha=1).
    return rgb * BACKDROP_EXPOSURE;
}
