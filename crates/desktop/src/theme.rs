use std::sync::{OnceLock, RwLock};

// ── Theme enum ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MapTheme {
    /// Navy/teal/orange — classic complementary split (blue opposite orange).
    /// The most legible pairing for dark-room tactical use.
    #[default]
    Topo,
    /// Black/green/amber — analogous warm-green palette.
    /// Evokes phosphor CRT screens, radar scopes, and night-vision monoculars.
    Phosphor,
    /// Dark-violet/gold — split-complementary (violet + gold flank orange).
    /// High contrast without the familiar teal; reads as "threat/warning".
    Thermal,
    /// Charcoal/steel/white — pure achromatic value contrast.
    /// Closest to a printed topographic map; useful for export/print.
    Ghost,
    /// Jet-black/cherry-red/electric-cyan — split-complementary at maximum saturation.
    /// Inspired by the visual language of Katsuhiro Otomo's Akira: pure black voids,
    /// saturated danger-red, cold blue-white technology glow.
    Akira,
}

impl MapTheme {
    pub const ALL: &'static [MapTheme] =
        &[MapTheme::Topo, MapTheme::Phosphor, MapTheme::Thermal, MapTheme::Ghost, MapTheme::Akira];

    pub fn label(self) -> &'static str {
        match self {
            Self::Topo => "TOPO",
            Self::Phosphor => "PHOSPHOR",
            Self::Thermal => "THERMAL",
            Self::Ghost => "GHOST",
            Self::Akira => "AKIRA",
        }
    }

    pub fn theory(self) -> &'static str {
        match self {
            Self::Topo => "complementary · blue / orange",
            Self::Phosphor => "analogous · green / amber",
            Self::Thermal => "split-complementary · violet / gold",
            Self::Ghost => "achromatic · value contrast only",
            Self::Akira => "split-complementary · cherry-red / electric-cyan",
        }
    }
}

// ── Global active theme ───────────────────────────────────────────────────────

fn theme_lock() -> &'static RwLock<MapTheme> {
    static T: OnceLock<RwLock<MapTheme>> = OnceLock::new();
    T.get_or_init(|| RwLock::new(MapTheme::Topo))
}

fn current() -> MapTheme {
    *theme_lock().read().unwrap_or_else(|e| e.into_inner())
}

/// Apply a new theme.  Call this whenever the user changes the picker.
/// Updates both the internal palette and the egui widget style.
pub fn set_theme(ctx: &egui::Context, theme: MapTheme) {
    *theme_lock().write().unwrap_or_else(|e| e.into_inner()) = theme;
    apply_egui_style(ctx, theme);
}

// ── egui widget style ─────────────────────────────────────────────────────────

fn apply_egui_style(ctx: &egui::Context, theme: MapTheme) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(12.0, 8.0);
    style.spacing.window_margin = egui::Margin::same(14);
    style.visuals = egui::Visuals::dark();

    let (panel, faint, extreme, active, hovered, inactive, window, selection, hyper) = match theme {
        MapTheme::Topo => (
            egui::Color32::from_rgb(7, 16, 24),
            egui::Color32::from_rgb(12, 26, 37),
            egui::Color32::from_rgb(5, 12, 18),
            egui::Color32::from_rgb(31, 91, 110),
            egui::Color32::from_rgb(22, 64, 78),
            egui::Color32::from_rgb(11, 25, 35),
            egui::Color32::from_rgb(9, 18, 27),
            egui::Color32::from_rgb(22, 120, 146),
            egui::Color32::from_rgb(126, 208, 229),
        ),
        MapTheme::Phosphor => (
            egui::Color32::from_rgb(5, 14, 5),
            egui::Color32::from_rgb(9, 22, 9),
            egui::Color32::from_rgb(3, 9, 3),
            egui::Color32::from_rgb(25, 90, 25),
            egui::Color32::from_rgb(18, 65, 18),
            egui::Color32::from_rgb(9, 26, 9),
            egui::Color32::from_rgb(7, 16, 7),
            egui::Color32::from_rgb(20, 115, 20),
            egui::Color32::from_rgb(120, 210, 120),
        ),
        MapTheme::Thermal => (
            egui::Color32::from_rgb(11, 7, 22),
            egui::Color32::from_rgb(16, 10, 32),
            egui::Color32::from_rgb(6, 4, 14),
            egui::Color32::from_rgb(75, 40, 120),
            egui::Color32::from_rgb(55, 30, 90),
            egui::Color32::from_rgb(16, 10, 30),
            egui::Color32::from_rgb(9, 6, 20),
            egui::Color32::from_rgb(90, 50, 145),
            egui::Color32::from_rgb(175, 120, 240),
        ),
        MapTheme::Ghost => (
            egui::Color32::from_rgb(13, 15, 18),
            egui::Color32::from_rgb(18, 21, 25),
            egui::Color32::from_rgb(8, 10, 12),
            egui::Color32::from_rgb(50, 62, 72),
            egui::Color32::from_rgb(38, 47, 55),
            egui::Color32::from_rgb(20, 25, 30),
            egui::Color32::from_rgb(11, 13, 16),
            egui::Color32::from_rgb(70, 88, 102),
            egui::Color32::from_rgb(190, 205, 215),
        ),
        MapTheme::Akira => (
            egui::Color32::from_rgb(6, 3, 3),    // panel: near-black with red tint
            egui::Color32::from_rgb(12, 5, 5),
            egui::Color32::from_rgb(2, 1, 1),
            egui::Color32::from_rgb(130, 15, 15), // active: dark blood-red
            egui::Color32::from_rgb(90, 10, 10),  // hovered
            egui::Color32::from_rgb(22, 6, 6),    // inactive
            egui::Color32::from_rgb(8, 3, 3),     // window
            egui::Color32::from_rgb(180, 20, 20), // selection: vivid red
            egui::Color32::from_rgb(0, 215, 255), // hyperlink: electric cyan
        ),
    };

    style.visuals.panel_fill = panel;
    style.visuals.faint_bg_color = faint;
    style.visuals.extreme_bg_color = extreme;
    style.visuals.widgets.active.bg_fill = active;
    style.visuals.widgets.hovered.bg_fill = hovered;
    style.visuals.widgets.inactive.bg_fill = inactive;
    style.visuals.window_fill = window;
    style.visuals.selection.bg_fill = selection;
    style.visuals.hyperlink_color = hyper;
    ctx.set_style(style);
}

/// Called once at startup to install the default theme.
pub fn install(ctx: &egui::Context) {
    apply_egui_style(ctx, current());
}

// ── Map palette ───────────────────────────────────────────────────────────────

pub fn canvas_background() -> egui::Color32 {
    match current() {
        MapTheme::Topo     => egui::Color32::from_rgb(5, 15, 22),
        MapTheme::Phosphor => egui::Color32::from_rgb(3, 9, 3),
        MapTheme::Thermal  => egui::Color32::from_rgb(8, 5, 18),
        MapTheme::Ghost    => egui::Color32::from_rgb(10, 12, 14),
        MapTheme::Akira    => egui::Color32::from_rgb(2, 0, 0),
    }
}

pub fn section_background() -> egui::Color32 {
    match current() {
        MapTheme::Topo     => egui::Color32::from_rgb(10, 22, 31),
        MapTheme::Phosphor => egui::Color32::from_rgb(7, 18, 7),
        MapTheme::Thermal  => egui::Color32::from_rgb(14, 9, 28),
        MapTheme::Ghost    => egui::Color32::from_rgb(15, 18, 21),
        MapTheme::Akira    => egui::Color32::from_rgb(14, 4, 4),
    }
}

pub fn grid_color() -> egui::Color32 {
    match current() {
        MapTheme::Topo     => egui::Color32::from_rgb(30, 68, 82),
        MapTheme::Phosphor => egui::Color32::from_rgb(15, 52, 15),
        MapTheme::Thermal  => egui::Color32::from_rgb(42, 22, 68),
        MapTheme::Ghost    => egui::Color32::from_rgb(28, 34, 40),
        MapTheme::Akira    => egui::Color32::from_rgb(55, 8, 8),
    }
}

pub fn camera_color() -> egui::Color32 {
    match current() {
        MapTheme::Topo     => egui::Color32::from_rgb(126, 208, 229),
        MapTheme::Phosphor => egui::Color32::from_rgb(120, 210, 120),
        MapTheme::Thermal  => egui::Color32::from_rgb(175, 120, 240),
        MapTheme::Ghost    => egui::Color32::from_rgb(190, 205, 215),
        MapTheme::Akira    => egui::Color32::from_rgb(0, 215, 255),  // electric cyan
    }
}

pub fn text_muted() -> egui::Color32 {
    match current() {
        MapTheme::Topo     => egui::Color32::from_rgb(154, 178, 189),
        MapTheme::Phosphor => egui::Color32::from_rgb(145, 178, 145),
        MapTheme::Thermal  => egui::Color32::from_rgb(175, 155, 198),
        MapTheme::Ghost    => egui::Color32::from_rgb(155, 168, 178),
        MapTheme::Akira    => egui::Color32::from_rgb(185, 140, 140),
    }
}

pub fn wireframe_color() -> egui::Color32 {
    match current() {
        MapTheme::Topo     => egui::Color32::from_rgb(66, 123, 143),
        MapTheme::Phosphor => egui::Color32::from_rgb(48, 115, 48),
        MapTheme::Thermal  => egui::Color32::from_rgb(92, 55, 148),
        MapTheme::Ghost    => egui::Color32::from_rgb(68, 82, 92),
        MapTheme::Akira    => egui::Color32::from_rgb(100, 12, 12),
    }
}

pub fn topo_color() -> egui::Color32 {
    match current() {
        MapTheme::Topo     => egui::Color32::from_rgb(39, 88, 105),
        MapTheme::Phosphor => egui::Color32::from_rgb(22, 70, 22),
        MapTheme::Thermal  => egui::Color32::from_rgb(55, 30, 92),
        MapTheme::Ghost    => egui::Color32::from_rgb(38, 46, 52),
        MapTheme::Akira    => egui::Color32::from_rgb(75, 10, 10),
    }
}

pub fn hot_color() -> egui::Color32 {
    match current() {
        MapTheme::Topo     => egui::Color32::from_rgb(245, 125, 78),
        MapTheme::Phosphor => egui::Color32::from_rgb(210, 162, 40),
        MapTheme::Thermal  => egui::Color32::from_rgb(222, 188, 38),
        MapTheme::Ghost    => egui::Color32::from_rgb(228, 238, 245),
        MapTheme::Akira    => egui::Color32::from_rgb(210, 18, 35),  // cherry red
    }
}

pub fn contour_color() -> egui::Color32 {
    match current() {
        MapTheme::Topo     => egui::Color32::from_rgb(96, 164, 181),
        MapTheme::Phosphor => egui::Color32::from_rgb(72, 180, 72),
        MapTheme::Thermal  => egui::Color32::from_rgb(138, 82, 205),
        MapTheme::Ghost    => egui::Color32::from_rgb(105, 122, 135),
        MapTheme::Akira    => egui::Color32::from_rgb(0, 185, 220),  // cold electric cyan
    }
}

// ── UI chrome palette ─────────────────────────────────────────────────────────

/// Semi-transparent fill for overlay panels (layer bar, focus card, footer).
pub fn panel_fill(alpha: u8) -> egui::Color32 {
    let (r, g, b) = match current() {
        MapTheme::Topo     => (7, 18, 24),
        MapTheme::Phosphor => (5, 14, 5),
        MapTheme::Thermal  => (11, 7, 22),
        MapTheme::Ghost    => (13, 15, 18),
        MapTheme::Akira    => (6, 3, 3),
    };
    egui::Color32::from_rgba_premultiplied(r, g, b, alpha)
}

/// Stroke/border color for overlay panels and chrome frames.
pub fn panel_stroke() -> egui::Color32 {
    match current() {
        MapTheme::Topo     => egui::Color32::from_rgb(24, 63, 79),
        MapTheme::Phosphor => egui::Color32::from_rgb(18, 65, 18),
        MapTheme::Thermal  => egui::Color32::from_rgb(55, 30, 90),
        MapTheme::Ghost    => egui::Color32::from_rgb(38, 47, 55),
        MapTheme::Akira    => egui::Color32::from_rgb(90, 10, 10),
    }
}

/// Fill for floating windows (Settings, Terrain Library).
pub fn window_fill() -> egui::Color32 {
    match current() {
        MapTheme::Topo     => egui::Color32::from_rgb(14, 18, 23),
        MapTheme::Phosphor => egui::Color32::from_rgb(9, 16, 9),
        MapTheme::Thermal  => egui::Color32::from_rgb(14, 9, 26),
        MapTheme::Ghost    => egui::Color32::from_rgb(16, 19, 22),
        MapTheme::Akira    => egui::Color32::from_rgb(10, 4, 4),
    }
}

/// Stroke for floating windows.
pub fn window_stroke() -> egui::Color32 {
    match current() {
        MapTheme::Topo     => egui::Color32::from_rgb(43, 49, 58),
        MapTheme::Phosphor => egui::Color32::from_rgb(28, 48, 28),
        MapTheme::Thermal  => egui::Color32::from_rgb(52, 28, 75),
        MapTheme::Ghost    => egui::Color32::from_rgb(42, 50, 58),
        MapTheme::Akira    => egui::Color32::from_rgb(75, 18, 18),
    }
}

/// Fill color for the active state of chrome toggle buttons (GLOBE/LOCAL etc.).
pub fn chrome_active_fill() -> egui::Color32 {
    match current() {
        MapTheme::Topo     => egui::Color32::from_rgb(24, 63, 79),
        MapTheme::Phosphor => egui::Color32::from_rgb(18, 65, 18),
        MapTheme::Thermal  => egui::Color32::from_rgb(55, 30, 90),
        MapTheme::Ghost    => egui::Color32::from_rgb(38, 47, 55),
        MapTheme::Akira    => egui::Color32::from_rgb(90, 10, 10),
    }
}

/// Text color for the active state of chrome toggle buttons — near-white with a theme tint.
pub fn chrome_active_text() -> egui::Color32 {
    match current() {
        MapTheme::Topo     => egui::Color32::from_rgb(225, 245, 250),  // near-white cyan
        MapTheme::Phosphor => egui::Color32::from_rgb(220, 245, 220),  // near-white green
        MapTheme::Thermal  => egui::Color32::from_rgb(235, 225, 248),  // near-white lavender
        MapTheme::Ghost    => egui::Color32::from_rgb(238, 240, 242),  // near-white neutral
        MapTheme::Akira    => egui::Color32::from_rgb(210, 245, 252),  // near-white cyan
    }
}

/// Fill for list/card items in side panels (terrain library, camera list).
pub fn item_fill() -> egui::Color32 {
    match current() {
        MapTheme::Topo     => egui::Color32::from_rgb(15, 22, 28),
        MapTheme::Phosphor => egui::Color32::from_rgb(9, 18, 9),
        MapTheme::Thermal  => egui::Color32::from_rgb(16, 10, 30),
        MapTheme::Ghost    => egui::Color32::from_rgb(18, 21, 25),
        MapTheme::Akira    => egui::Color32::from_rgb(12, 5, 5),
    }
}

/// Fill for the selected/highlighted item in a list.
pub fn selected_item_fill() -> egui::Color32 {
    match current() {
        MapTheme::Topo     => egui::Color32::from_rgb(18, 44, 56),
        MapTheme::Phosphor => egui::Color32::from_rgb(14, 44, 14),
        MapTheme::Thermal  => egui::Color32::from_rgb(40, 22, 62),
        MapTheme::Ghost    => egui::Color32::from_rgb(28, 35, 42),
        MapTheme::Akira    => egui::Color32::from_rgb(62, 10, 10),
    }
}

/// Near-opaque backdrop fill for the globe scene (space/void behind the sphere).
pub fn scene_backdrop() -> egui::Color32 {
    match current() {
        MapTheme::Topo     => egui::Color32::from_rgba_premultiplied(2, 6, 10, 252),
        MapTheme::Phosphor => egui::Color32::from_rgba_premultiplied(2, 6, 2, 252),
        MapTheme::Thermal  => egui::Color32::from_rgba_premultiplied(4, 2, 10, 252),
        MapTheme::Ghost    => egui::Color32::from_rgba_premultiplied(5, 6, 8, 252),
        MapTheme::Akira    => egui::Color32::from_rgba_premultiplied(3, 0, 0, 252),
    }
}

/// Warm glow halo behind event markers.
pub fn marker_glow_warm() -> egui::Color32 {
    match current() {
        MapTheme::Topo     => egui::Color32::from_rgba_premultiplied(255, 241, 212, 170),
        MapTheme::Phosphor => egui::Color32::from_rgba_premultiplied(240, 222, 155, 170),
        MapTheme::Thermal  => egui::Color32::from_rgba_premultiplied(238, 218, 110, 170),
        MapTheme::Ghost    => egui::Color32::from_rgba_premultiplied(245, 245, 235, 170),
        MapTheme::Akira    => egui::Color32::from_rgba_premultiplied(255, 185, 155, 170),
    }
}

/// Outer ring color for camera markers.
pub fn marker_camera_ring() -> egui::Color32 {
    match current() {
        MapTheme::Topo     => egui::Color32::from_rgb(215, 245, 252),
        MapTheme::Phosphor => egui::Color32::from_rgb(195, 240, 195),
        MapTheme::Thermal  => egui::Color32::from_rgb(230, 210, 252),
        MapTheme::Ghost    => egui::Color32::from_rgb(230, 238, 245),
        MapTheme::Akira    => egui::Color32::from_rgb(180, 245, 255),
    }
}

// ── ADS-B flight category colours ────────────────────────────────────────────
// Each category gets a visually distinct hue that reads on every dark background.

/// Scheduled passenger airline flights (sky-blue family).
pub fn flight_airline_color() -> egui::Color32 {
    match current() {
        MapTheme::Topo     => egui::Color32::from_rgb( 80, 185, 255),
        MapTheme::Phosphor => egui::Color32::from_rgb( 60, 215, 205),
        MapTheme::Thermal  => egui::Color32::from_rgb(130, 205, 255),
        MapTheme::Ghost    => egui::Color32::from_rgb(160, 205, 235),
        MapTheme::Akira    => egui::Color32::from_rgb(  0, 215, 255),
    }
}

/// Cargo / freight operators (warm orange family).
pub fn flight_cargo_color() -> egui::Color32 {
    match current() {
        MapTheme::Topo     => egui::Color32::from_rgb(255, 155,  40),
        MapTheme::Phosphor => egui::Color32::from_rgb(230, 180,  35),
        MapTheme::Thermal  => egui::Color32::from_rgb(255, 175,  55),
        MapTheme::Ghost    => egui::Color32::from_rgb(225, 175, 135),
        MapTheme::Akira    => egui::Color32::from_rgb(255, 135,  30),
    }
}

/// Military / government callsigns (red family).
pub fn flight_military_color() -> egui::Color32 {
    match current() {
        MapTheme::Topo     => egui::Color32::from_rgb(215,  65,  75),
        MapTheme::Phosphor => egui::Color32::from_rgb(205, 100,  50),
        MapTheme::Thermal  => egui::Color32::from_rgb(215,  55,  90),
        MapTheme::Ghost    => egui::Color32::from_rgb(200, 120, 130),
        MapTheme::Akira    => egui::Color32::from_rgb(255,  38,  55),
    }
}

/// General aviation — private / training / recreational (lime-green family).
pub fn flight_ga_color() -> egui::Color32 {
    match current() {
        MapTheme::Topo     => egui::Color32::from_rgb(155, 225,  70),
        MapTheme::Phosphor => egui::Color32::from_rgb(200, 230,  45),
        MapTheme::Thermal  => egui::Color32::from_rgb(145, 235,  85),
        MapTheme::Ghost    => egui::Color32::from_rgb(170, 215, 150),
        MapTheme::Akira    => egui::Color32::from_rgb(180, 255,  55),
    }
}

/// No callsign / unrecognised pattern (muted amber).
pub fn flight_unknown_color() -> egui::Color32 {
    match current() {
        MapTheme::Topo     => egui::Color32::from_rgb(195, 170,  60),
        MapTheme::Phosphor => egui::Color32::from_rgb(165, 160,  50),
        MapTheme::Thermal  => egui::Color32::from_rgb(190, 175,  65),
        MapTheme::Ghost    => egui::Color32::from_rgb(170, 170, 150),
        MapTheme::Akira    => egui::Color32::from_rgb(195, 155,  45),
    }
}

/// Water features (rivers, lakes, streams).  Always blue-family but tuned
/// per theme so it reads clearly against each background palette.
pub fn water_color() -> egui::Color32 {
    match current() {
        MapTheme::Topo     => egui::Color32::from_rgb(60,  145, 210),
        MapTheme::Phosphor => egui::Color32::from_rgb(50,  200, 160),
        MapTheme::Thermal  => egui::Color32::from_rgb(90,  170, 255),
        MapTheme::Ghost    => egui::Color32::from_rgb(120, 175, 220),
        MapTheme::Akira    => egui::Color32::from_rgb(0,   185, 220),
    }
}
