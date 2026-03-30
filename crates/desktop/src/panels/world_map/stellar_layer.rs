/// Stellar Correspondence Layer
///
/// Projects each star from the celestial sphere onto Earth's surface using the
/// direct geocentric mapping:
///
///   Earth latitude  = star declination
///   Earth longitude = star right ascension (0–180° stays as-is; 180–360° wraps to −180–0°)
///
/// From the geocentric frame, stars sit on a surrounding shell.  Contracting
/// that shell to the Earth's surface gives each star a unique ground point.
use crate::model::{GeoPoint, GlobeViewState};
use crate::stellar_catalog;

use super::globe_scene::{project_geo, GlobeLayout};

pub(super) fn draw_stellar_correspondence(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
) {
    for star in stellar_catalog::STARS {
        let lon = if star.ra_deg <= 180.0 {
            star.ra_deg
        } else {
            star.ra_deg - 360.0
        };
        let location = GeoPoint {
            lat: star.dec_deg,
            lon,
        };

        let Some(proj) = project_geo(layout, view, location, 0.0) else {
            continue;
        };
        if !proj.front_facing {
            continue;
        }

        // Radius: bright stars are larger.  Clamp so even the faintest star is
        // still a visible pixel and the brightest doesn't overwhelm the map.
        let radius = ((3.0 - star.mag) * 0.55).clamp(0.9, 4.5);

        // Soft bloom halo for the ten brightest stars
        if star.mag < 1.0 {
            let bloom_r = radius * 2.8;
            let bloom_alpha = (((1.0 - star.mag) / 2.0) * 30.0).clamp(10.0, 30.0) as u8;
            painter.circle_filled(
                proj.pos,
                bloom_r,
                egui::Color32::from_rgba_unmultiplied(180, 210, 255, bloom_alpha),
            );
        }

        // Core dot — blue-white stellar colour
        let core_alpha = (((3.5 - star.mag) / 5.5) * 220.0).clamp(80.0, 220.0) as u8;
        let core = egui::Color32::from_rgba_unmultiplied(215, 228, 255, core_alpha);
        painter.circle_filled(proj.pos, radius, core);

        // Bright-centre sparkle for mag < 2.5
        if star.mag < 2.5 {
            painter.circle_filled(
                proj.pos,
                (radius * 0.35).max(0.6),
                egui::Color32::from_rgba_unmultiplied(240, 246, 255, 240),
            );
        }

        // Name label for bright named stars only (avoids clutter)
        if !star.name.is_empty() && star.mag < 2.2 {
            painter.text(
                proj.pos + egui::vec2(radius + 3.0, 0.0),
                egui::Align2::LEFT_CENTER,
                star.name,
                egui::FontId::monospace(8.5),
                egui::Color32::from_rgba_unmultiplied(185, 208, 248, 155),
            );
        }
    }
}
