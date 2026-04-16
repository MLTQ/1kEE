use std::sync::{OnceLock, RwLock};

// ── Theme enum ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MapTheme {
    /// Perfect-black/sodium-amber/warm ivory.
    /// Evokes low-pressure street lamps, darkroom safelights, and amber HUD glass.
    Sodium,
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
    /// Space-black / regolith-grey / highland-white — achromatic monochrome keyed to
    /// actual lunar albedo.  Mare basalts are near-black; highlands bleach toward
    /// bone-white.  Designed for SLDEM2015 lunar topology data.
    Lunar,
    /// Hematite red / terra cotta orange / rusted black — warm dusty palette
    /// scaled explicitly matching MRO Mars Context Camera DTM mappings. 
    Mars,
    /// Deep crimson / dark iron / pitch black — high-contrast low-light Mars mode.
    MarsDark,
}

impl MapTheme {
    pub const ALL: &'static [MapTheme] = &[
        MapTheme::Topo,
        MapTheme::Phosphor,
        MapTheme::Thermal,
        MapTheme::Ghost,
        MapTheme::Akira,
        MapTheme::Sodium,
        MapTheme::Lunar,
        MapTheme::Mars,
        MapTheme::MarsDark,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Sodium => "SODIUM",
            Self::Topo => "TOPO",
            Self::Phosphor => "PHOSPHOR",
            Self::Thermal => "THERMAL",
            Self::Ghost => "GHOST",
            Self::Akira => "AKIRA",
            Self::Lunar => "LUNAR",
            Self::Mars => "MARS",
            Self::MarsDark => "MARS DARK",
        }
    }

    pub fn theory(self) -> &'static str {
        match self {
            Self::Sodium => "monochrome · sodium amber / lamp black",
            Self::Topo => "complementary · blue / orange",
            Self::Phosphor => "analogous · green / amber",
            Self::Thermal => "split-complementary · violet / gold",
            Self::Ghost => "achromatic · value contrast only",
            Self::Akira => "split-complementary · cherry-red / electric-cyan",
            Self::Lunar => "monochrome · regolith grey / space black",
            Self::Mars => "monochrome · terra cotta / rusted iron",
            Self::MarsDark => "high-contrast · deep crimson / pitch black",
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

    let (panel, faint, extreme, active, hovered, inactive, window, selection, hyper, text) =
        match theme {
            MapTheme::Sodium => (
                egui::Color32::from_rgb(0, 0, 0),
                egui::Color32::from_rgb(8, 5, 2),
                egui::Color32::from_rgb(0, 0, 0),
                egui::Color32::from_rgb(108, 61, 18),
                egui::Color32::from_rgb(78, 44, 13),
                egui::Color32::from_rgb(18, 10, 4),
                egui::Color32::from_rgb(3, 2, 0),
                egui::Color32::from_rgb(186, 111, 28),
                egui::Color32::from_rgb(255, 190, 110),
                Some(egui::Color32::from_rgb(255, 239, 210)),
            ),
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
                None,
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
                None,
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
                None,
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
                None,
            ),
            MapTheme::Akira => (
                egui::Color32::from_rgb(6, 3, 3), // panel: near-black with red tint
                egui::Color32::from_rgb(12, 5, 5),
                egui::Color32::from_rgb(2, 1, 1),
                egui::Color32::from_rgb(130, 15, 15), // active: dark blood-red
                egui::Color32::from_rgb(90, 10, 10),  // hovered
                egui::Color32::from_rgb(22, 6, 6),    // inactive
                egui::Color32::from_rgb(8, 3, 3),     // window
                egui::Color32::from_rgb(180, 20, 20), // selection: vivid red
                egui::Color32::from_rgb(0, 215, 255), // hyperlink: electric cyan
                None,
            ),
            MapTheme::Lunar => (
                egui::Color32::from_rgb(5, 5, 7), // panel: near-black space
                egui::Color32::from_rgb(9, 9, 12),
                egui::Color32::from_rgb(2, 2, 4),
                egui::Color32::from_rgb(58, 57, 68), // active: muted grey-blue
                egui::Color32::from_rgb(44, 43, 52), // hovered
                egui::Color32::from_rgb(15, 15, 20), // inactive
                egui::Color32::from_rgb(7, 7, 10),   // window
                egui::Color32::from_rgb(78, 82, 104), // selection: cool grey-blue
                egui::Color32::from_rgb(155, 200, 248), // hyperlink: pale ice-blue
                Some(egui::Color32::from_rgb(218, 214, 204)), // text: pale warm grey
            ),
            MapTheme::Mars => (
                egui::Color32::from_rgb(10, 4, 2), // panel: deep rusted iron
                egui::Color32::from_rgb(16, 7, 4),
                egui::Color32::from_rgb(5, 2, 1),
                egui::Color32::from_rgb(130, 48, 24), // active: terra cotta
                egui::Color32::from_rgb(100, 36, 18), // hovered
                egui::Color32::from_rgb(26, 10, 5),   // inactive
                egui::Color32::from_rgb(14, 5, 3),    // window
                egui::Color32::from_rgb(160, 60, 32), // selection: dusty orange
                egui::Color32::from_rgb(255, 150, 100), // hyperlink: vibrant salmon
                Some(egui::Color32::from_rgb(245, 200, 180)), // text: pale sepia
            ),
            MapTheme::MarsDark => (
                egui::Color32::from_rgb(5, 0, 0),
                egui::Color32::from_rgb(8, 0, 0),
                egui::Color32::from_rgb(3, 0, 0),
                egui::Color32::from_rgb(120, 15, 15),
                egui::Color32::from_rgb(90, 10, 10),
                egui::Color32::from_rgb(20, 0, 0),
                egui::Color32::from_rgb(10, 0, 0),
                egui::Color32::from_rgb(150, 20, 20),
                egui::Color32::from_rgb(255, 80, 80),
                Some(egui::Color32::from_rgb(230, 150, 150)),
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
    style.visuals.override_text_color = text;
    ctx.set_style(style);
}

/// Called once at startup to install the default theme.
pub fn install(ctx: &egui::Context) {
    apply_egui_style(ctx, current());
}

// ── Map palette ───────────────────────────────────────────────────────────────

pub fn canvas_background() -> egui::Color32 {
    match current() {
        MapTheme::Sodium => egui::Color32::from_rgb(0, 0, 0),
        MapTheme::Topo => egui::Color32::from_rgb(5, 15, 22),
        MapTheme::Phosphor => egui::Color32::from_rgb(3, 9, 3),
        MapTheme::Thermal => egui::Color32::from_rgb(8, 5, 18),
        MapTheme::Ghost => egui::Color32::from_rgb(10, 12, 14),
        MapTheme::Akira => egui::Color32::from_rgb(2, 0, 0),
        MapTheme::Lunar => egui::Color32::from_rgb(2, 2, 3),
        MapTheme::Mars => egui::Color32::from_rgb(3, 1, 0),
        MapTheme::MarsDark => egui::Color32::from_rgb(0, 0, 0),
    }
}

pub fn section_background() -> egui::Color32 {
    match current() {
        MapTheme::Sodium => egui::Color32::from_rgb(5, 3, 1),
        MapTheme::Topo => egui::Color32::from_rgb(10, 22, 31),
        MapTheme::Phosphor => egui::Color32::from_rgb(7, 18, 7),
        MapTheme::Thermal => egui::Color32::from_rgb(14, 9, 28),
        MapTheme::Ghost => egui::Color32::from_rgb(15, 18, 21),
        MapTheme::Akira => egui::Color32::from_rgb(14, 4, 4),
        MapTheme::Lunar => egui::Color32::from_rgb(9, 9, 13),
        MapTheme::Mars => egui::Color32::from_rgb(12, 5, 3),
        MapTheme::MarsDark => egui::Color32::from_rgb(5, 0, 0),
    }
}

pub fn grid_color() -> egui::Color32 {
    match current() {
        MapTheme::Sodium => egui::Color32::from_rgb(52, 28, 9),
        MapTheme::Topo => egui::Color32::from_rgb(30, 68, 82),
        MapTheme::Phosphor => egui::Color32::from_rgb(15, 52, 15),
        MapTheme::Thermal => egui::Color32::from_rgb(42, 22, 68),
        MapTheme::Ghost => egui::Color32::from_rgb(28, 34, 40),
        MapTheme::Akira => egui::Color32::from_rgb(55, 8, 8),
        MapTheme::Lunar => egui::Color32::from_rgb(46, 46, 54),
        MapTheme::Mars => egui::Color32::from_rgb(60, 20, 10),
        MapTheme::MarsDark => egui::Color32::from_rgb(40, 5, 5),
    }
}

pub fn camera_color() -> egui::Color32 {
    match current() {
        MapTheme::Sodium => egui::Color32::from_rgb(255, 196, 112),
        MapTheme::Topo => egui::Color32::from_rgb(126, 208, 229),
        MapTheme::Phosphor => egui::Color32::from_rgb(120, 210, 120),
        MapTheme::Thermal => egui::Color32::from_rgb(175, 120, 240),
        MapTheme::Ghost => egui::Color32::from_rgb(190, 205, 215),
        MapTheme::Akira => egui::Color32::from_rgb(0, 215, 255), // electric cyan
        MapTheme::Lunar => egui::Color32::from_rgb(155, 200, 248), // ice-blue mission control
        MapTheme::Mars => egui::Color32::from_rgb(200, 150, 100), 
        MapTheme::MarsDark => egui::Color32::from_rgb(255, 80, 80),
    }
}

pub fn text_muted() -> egui::Color32 {
    match current() {
        MapTheme::Sodium => egui::Color32::from_rgb(188, 152, 103),
        MapTheme::Topo => egui::Color32::from_rgb(154, 178, 189),
        MapTheme::Phosphor => egui::Color32::from_rgb(145, 178, 145),
        MapTheme::Thermal => egui::Color32::from_rgb(175, 155, 198),
        MapTheme::Ghost => egui::Color32::from_rgb(155, 168, 178),
        MapTheme::Akira => egui::Color32::from_rgb(185, 140, 140),
        MapTheme::Lunar => egui::Color32::from_rgb(155, 152, 142),
        MapTheme::Mars => egui::Color32::from_rgb(190, 145, 120),
        MapTheme::MarsDark => egui::Color32::from_rgb(190, 100, 100),
    }
}

pub fn wireframe_color() -> egui::Color32 {
    match current() {
        MapTheme::Sodium => egui::Color32::from_rgb(120, 70, 24),
        MapTheme::Topo => egui::Color32::from_rgb(66, 123, 143),
        MapTheme::Phosphor => egui::Color32::from_rgb(48, 115, 48),
        MapTheme::Thermal => egui::Color32::from_rgb(92, 55, 148),
        MapTheme::Ghost => egui::Color32::from_rgb(68, 82, 92),
        MapTheme::Akira => egui::Color32::from_rgb(100, 12, 12),
        MapTheme::Lunar => egui::Color32::from_rgb(68, 67, 78),
        MapTheme::Mars => egui::Color32::from_rgb(85, 30, 20),
        MapTheme::MarsDark => egui::Color32::from_rgb(80, 15, 15),
    }
}

pub fn topo_color() -> egui::Color32 {
    match current() {
        MapTheme::Sodium => egui::Color32::from_rgb(82, 48, 16),
        MapTheme::Topo => egui::Color32::from_rgb(39, 88, 105),
        MapTheme::Phosphor => egui::Color32::from_rgb(22, 70, 22),
        MapTheme::Thermal => egui::Color32::from_rgb(55, 30, 92),
        MapTheme::Ghost => egui::Color32::from_rgb(38, 46, 52),
        MapTheme::Akira => egui::Color32::from_rgb(75, 10, 10),
        MapTheme::Lunar => egui::Color32::from_rgb(54, 53, 62),
        MapTheme::Mars => egui::Color32::from_rgb(68, 25, 16),
        MapTheme::MarsDark => egui::Color32::from_rgb(45, 5, 5),
    }
}

pub fn hot_color() -> egui::Color32 {
    match current() {
        MapTheme::Sodium => egui::Color32::from_rgb(255, 170, 72),
        MapTheme::Topo => egui::Color32::from_rgb(245, 125, 78),
        MapTheme::Phosphor => egui::Color32::from_rgb(210, 162, 40),
        MapTheme::Thermal => egui::Color32::from_rgb(222, 188, 38),
        MapTheme::Ghost => egui::Color32::from_rgb(228, 238, 245),
        MapTheme::Akira => egui::Color32::from_rgb(210, 18, 35), // cherry red
        MapTheme::Lunar => egui::Color32::from_rgb(215, 210, 188), // sunlit regolith
        MapTheme::Mars => egui::Color32::from_rgb(255, 145, 80), 
        MapTheme::MarsDark => egui::Color32::from_rgb(255, 60, 60),
    }
}

pub fn contour_color() -> egui::Color32 {
    match current() {
        MapTheme::Sodium => egui::Color32::from_rgb(212, 128, 44),
        MapTheme::Topo => egui::Color32::from_rgb(96, 164, 181),
        MapTheme::Phosphor => egui::Color32::from_rgb(72, 180, 72),
        MapTheme::Thermal => egui::Color32::from_rgb(138, 82, 205),
        MapTheme::Ghost => egui::Color32::from_rgb(105, 122, 135),
        MapTheme::Akira => egui::Color32::from_rgb(0, 185, 220), // cold electric cyan
        MapTheme::Lunar => egui::Color32::from_rgb(138, 136, 126),
        MapTheme::Mars => egui::Color32::from_rgb(165, 85, 55),
        MapTheme::MarsDark => egui::Color32::from_rgb(160, 30, 30),
    }
}

// ── UI chrome palette ─────────────────────────────────────────────────────────

/// Semi-transparent fill for overlay panels (layer bar, focus card, footer).
pub fn panel_fill(alpha: u8) -> egui::Color32 {
    let (r, g, b) = match current() {
        MapTheme::Sodium => (4, 2, 0),
        MapTheme::Topo => (7, 18, 24),
        MapTheme::Phosphor => (5, 14, 5),
        MapTheme::Thermal => (11, 7, 22),
        MapTheme::Ghost => (13, 15, 18),
        MapTheme::Akira => (6, 3, 3),
        MapTheme::Lunar => (5, 5, 7),
        MapTheme::Mars => (6, 2, 1),
        MapTheme::MarsDark => (3, 1, 1),
    };
    egui::Color32::from_rgba_premultiplied(r, g, b, alpha)
}

/// Stroke/border color for overlay panels and chrome frames.
pub fn panel_stroke() -> egui::Color32 {
    match current() {
        MapTheme::Sodium => egui::Color32::from_rgb(84, 50, 18),
        MapTheme::Topo => egui::Color32::from_rgb(24, 63, 79),
        MapTheme::Phosphor => egui::Color32::from_rgb(18, 65, 18),
        MapTheme::Thermal => egui::Color32::from_rgb(55, 30, 90),
        MapTheme::Ghost => egui::Color32::from_rgb(38, 47, 55),
        MapTheme::Akira => egui::Color32::from_rgb(90, 10, 10),
        MapTheme::Lunar => egui::Color32::from_rgb(40, 40, 50),
        MapTheme::Mars => egui::Color32::from_rgb(60, 25, 15),
        MapTheme::MarsDark => egui::Color32::from_rgb(60, 10, 10),
    }
}

/// Fill for floating windows (Settings, Terrain Library).
pub fn window_fill() -> egui::Color32 {
    match current() {
        MapTheme::Sodium => egui::Color32::from_rgb(7, 4, 1),
        MapTheme::Topo => egui::Color32::from_rgb(14, 18, 23),
        MapTheme::Phosphor => egui::Color32::from_rgb(9, 16, 9),
        MapTheme::Thermal => egui::Color32::from_rgb(14, 9, 26),
        MapTheme::Ghost => egui::Color32::from_rgb(16, 19, 22),
        MapTheme::Akira => egui::Color32::from_rgb(10, 4, 4),
        MapTheme::Lunar => egui::Color32::from_rgb(8, 8, 12),
        MapTheme::Mars => egui::Color32::from_rgb(10, 4, 2),
        MapTheme::MarsDark => egui::Color32::from_rgb(5, 0, 0),
    }
}

/// Stroke for floating windows.
pub fn window_stroke() -> egui::Color32 {
    match current() {
        MapTheme::Sodium => egui::Color32::from_rgb(94, 58, 22),
        MapTheme::Topo => egui::Color32::from_rgb(43, 49, 58),
        MapTheme::Phosphor => egui::Color32::from_rgb(28, 48, 28),

        MapTheme::Thermal => egui::Color32::from_rgb(52, 28, 75),
        MapTheme::Ghost => egui::Color32::from_rgb(42, 50, 58),
        MapTheme::Akira => egui::Color32::from_rgb(75, 18, 18),
        MapTheme::Lunar => egui::Color32::from_rgb(38, 38, 50),
        MapTheme::Mars => egui::Color32::from_rgb(50, 20, 12),
        MapTheme::MarsDark => egui::Color32::from_rgb(45, 5, 5),
    }
}

/// Fill color for the active state of chrome toggle buttons (GLOBE/LOCAL etc.).
pub fn chrome_active_fill() -> egui::Color32 {
    match current() {
        MapTheme::Sodium => egui::Color32::from_rgb(96, 55, 18),
        MapTheme::Topo => egui::Color32::from_rgb(24, 63, 79),
        MapTheme::Phosphor => egui::Color32::from_rgb(18, 65, 18),
        MapTheme::Thermal => egui::Color32::from_rgb(55, 30, 90),
        MapTheme::Ghost => egui::Color32::from_rgb(38, 47, 55),
        MapTheme::Akira => egui::Color32::from_rgb(90, 10, 10),
        MapTheme::Lunar => egui::Color32::from_rgb(44, 43, 58),
        MapTheme::Mars => egui::Color32::from_rgb(72, 28, 14),
        MapTheme::MarsDark => egui::Color32::from_rgb(90, 15, 15),
    }
}

/// Text color for the active state of chrome toggle buttons — near-white with a theme tint.
pub fn chrome_active_text() -> egui::Color32 {
    match current() {
        MapTheme::Sodium => egui::Color32::from_rgb(255, 245, 224),
        MapTheme::Topo => egui::Color32::from_rgb(225, 245, 250), // near-white cyan
        MapTheme::Phosphor => egui::Color32::from_rgb(220, 245, 220), // near-white green
        MapTheme::Thermal => egui::Color32::from_rgb(235, 225, 248), // near-white lavender
        MapTheme::Ghost => egui::Color32::from_rgb(238, 240, 242), // near-white neutral
        MapTheme::Akira => egui::Color32::from_rgb(210, 245, 252), // near-white cyan
        MapTheme::Lunar => egui::Color32::from_rgb(218, 214, 204), // pale warm grey
        MapTheme::Mars => egui::Color32::from_rgb(240, 210, 195),
        MapTheme::MarsDark => egui::Color32::from_rgb(255, 190, 190),
    }
}

/// Fill for list/card items in side panels (terrain library, camera list).
pub fn item_fill() -> egui::Color32 {
    match current() {
        MapTheme::Sodium => egui::Color32::from_rgb(8, 5, 2),
        MapTheme::Topo => egui::Color32::from_rgb(15, 22, 28),
        MapTheme::Phosphor => egui::Color32::from_rgb(9, 18, 9),
        MapTheme::Thermal => egui::Color32::from_rgb(16, 10, 30),
        MapTheme::Ghost => egui::Color32::from_rgb(18, 21, 25),
        MapTheme::Akira => egui::Color32::from_rgb(12, 5, 5),
        MapTheme::Lunar => egui::Color32::from_rgb(11, 11, 15),
        MapTheme::Mars => egui::Color32::from_rgb(14, 6, 4),
        MapTheme::MarsDark => egui::Color32::from_rgb(8, 0, 0),
    }
}

/// Fill for the selected/highlighted item in a list.
pub fn selected_item_fill() -> egui::Color32 {
    match current() {
        MapTheme::Sodium => egui::Color32::from_rgb(36, 20, 8),
        MapTheme::Topo => egui::Color32::from_rgb(18, 44, 56),
        MapTheme::Phosphor => egui::Color32::from_rgb(14, 44, 14),
        MapTheme::Thermal => egui::Color32::from_rgb(40, 22, 62),
        MapTheme::Ghost => egui::Color32::from_rgb(28, 35, 42),
        MapTheme::Akira => egui::Color32::from_rgb(62, 10, 10),
        MapTheme::Lunar => egui::Color32::from_rgb(28, 28, 40),
        MapTheme::Mars => egui::Color32::from_rgb(45, 18, 10),
        MapTheme::MarsDark => egui::Color32::from_rgb(35, 5, 5),
    }
}

/// Near-opaque backdrop fill for the globe scene (space/void behind the sphere).
pub fn scene_backdrop() -> egui::Color32 {
    match current() {
        MapTheme::Sodium => egui::Color32::from_rgba_premultiplied(1, 0, 0, 252),
        MapTheme::Topo => egui::Color32::from_rgba_premultiplied(2, 6, 10, 252),
        MapTheme::Phosphor => egui::Color32::from_rgba_premultiplied(2, 6, 2, 252),
        MapTheme::Thermal => egui::Color32::from_rgba_premultiplied(4, 2, 10, 252),
        MapTheme::Ghost => egui::Color32::from_rgba_premultiplied(5, 6, 8, 252),
        MapTheme::Akira => egui::Color32::from_rgba_premultiplied(3, 0, 0, 252),
        MapTheme::Lunar => egui::Color32::from_rgba_premultiplied(2, 2, 3, 252),
        MapTheme::Mars => egui::Color32::from_rgba_premultiplied(4, 1, 0, 252),
        MapTheme::MarsDark => egui::Color32::from_rgba_premultiplied(3, 0, 0, 252),
    }
}

/// Warm glow halo behind event markers.
pub fn marker_glow_warm() -> egui::Color32 {
    match current() {
        MapTheme::Sodium => egui::Color32::from_rgba_premultiplied(255, 190, 96, 170),
        MapTheme::Topo => egui::Color32::from_rgba_premultiplied(255, 241, 212, 170),
        MapTheme::Phosphor => egui::Color32::from_rgba_premultiplied(240, 222, 155, 170),
        MapTheme::Thermal => egui::Color32::from_rgba_premultiplied(238, 218, 110, 170),
        MapTheme::Ghost => egui::Color32::from_rgba_premultiplied(245, 245, 235, 170),
        MapTheme::Akira => egui::Color32::from_rgba_premultiplied(255, 185, 155, 170),
        MapTheme::Lunar => egui::Color32::from_rgba_premultiplied(220, 215, 200, 170),
        MapTheme::Mars => egui::Color32::from_rgba_premultiplied(240, 160, 110, 170),
        MapTheme::MarsDark => egui::Color32::from_rgba_premultiplied(255, 60, 60, 170),
    }
}

/// Outer ring color for camera markers.
pub fn marker_camera_ring() -> egui::Color32 {
    match current() {
        MapTheme::Sodium => egui::Color32::from_rgb(255, 224, 176),
        MapTheme::Topo => egui::Color32::from_rgb(215, 245, 252),
        MapTheme::Phosphor => egui::Color32::from_rgb(195, 240, 195),
        MapTheme::Thermal => egui::Color32::from_rgb(230, 210, 252),
        MapTheme::Ghost => egui::Color32::from_rgb(230, 238, 245),
        MapTheme::Akira => egui::Color32::from_rgb(180, 245, 255),
        MapTheme::Lunar => egui::Color32::from_rgb(205, 220, 240),
        MapTheme::Mars => egui::Color32::from_rgb(245, 215, 180),
        MapTheme::MarsDark => egui::Color32::from_rgb(255, 100, 100),
    }
}

// ── ADS-B flight category colours ────────────────────────────────────────────
// Each category gets a visually distinct hue that reads on every dark background.

/// Scheduled passenger airline flights (sky-blue family).
pub fn flight_airline_color() -> egui::Color32 {
    match current() {
        MapTheme::Sodium => egui::Color32::from_rgb(255, 214, 150),
        MapTheme::Topo => egui::Color32::from_rgb(80, 185, 255),
        MapTheme::Phosphor => egui::Color32::from_rgb(60, 215, 205),
        MapTheme::Thermal => egui::Color32::from_rgb(130, 205, 255),
        MapTheme::Ghost => egui::Color32::from_rgb(160, 205, 235),
        MapTheme::Akira => egui::Color32::from_rgb(0, 215, 255),
        MapTheme::Lunar => egui::Color32::from_rgb(155, 200, 248),
        MapTheme::Mars => egui::Color32::from_rgb(120, 180, 240),
        MapTheme::MarsDark => egui::Color32::from_rgb(255, 120, 120),
    }
}

/// Cargo / freight operators (warm orange family).
pub fn flight_cargo_color() -> egui::Color32 {
    match current() {
        MapTheme::Sodium => egui::Color32::from_rgb(255, 166, 70),
        MapTheme::Topo => egui::Color32::from_rgb(255, 155, 40),
        MapTheme::Phosphor => egui::Color32::from_rgb(230, 180, 35),
        MapTheme::Thermal => egui::Color32::from_rgb(255, 175, 55),
        MapTheme::Ghost => egui::Color32::from_rgb(225, 175, 135),
        MapTheme::Akira => egui::Color32::from_rgb(255, 135, 30),
        MapTheme::Lunar => egui::Color32::from_rgb(215, 195, 155),
        MapTheme::Mars => egui::Color32::from_rgb(240, 185, 110),
        MapTheme::MarsDark => egui::Color32::from_rgb(255, 80, 40),
    }
}

/// Military / government callsigns (red family).
pub fn flight_military_color() -> egui::Color32 {
    match current() {
        MapTheme::Sodium => egui::Color32::from_rgb(220, 96, 44),
        MapTheme::Topo => egui::Color32::from_rgb(215, 65, 75),
        MapTheme::Phosphor => egui::Color32::from_rgb(205, 100, 50),
        MapTheme::Thermal => egui::Color32::from_rgb(215, 55, 90),
        MapTheme::Ghost => egui::Color32::from_rgb(200, 120, 130),
        MapTheme::Akira => egui::Color32::from_rgb(255, 38, 55),
        MapTheme::Lunar => egui::Color32::from_rgb(200, 100, 100),
        MapTheme::Mars => egui::Color32::from_rgb(230, 80, 80),
        MapTheme::MarsDark => egui::Color32::from_rgb(255, 0, 0),
    }
}

/// General aviation — private / training / recreational (lime-green family).
pub fn flight_ga_color() -> egui::Color32 {
    match current() {
        MapTheme::Sodium => egui::Color32::from_rgb(214, 184, 92),
        MapTheme::Topo => egui::Color32::from_rgb(155, 225, 70),
        MapTheme::Phosphor => egui::Color32::from_rgb(200, 230, 45),
        MapTheme::Thermal => egui::Color32::from_rgb(145, 235, 85),
        MapTheme::Ghost => egui::Color32::from_rgb(170, 215, 150),
        MapTheme::Akira => egui::Color32::from_rgb(180, 255, 55),
        MapTheme::Lunar => egui::Color32::from_rgb(185, 210, 178),
        MapTheme::Mars => egui::Color32::from_rgb(190, 220, 110),
        MapTheme::MarsDark => egui::Color32::from_rgb(255, 100, 80),
    }
}

/// No callsign / unrecognised pattern (muted amber).
pub fn flight_unknown_color() -> egui::Color32 {
    match current() {
        MapTheme::Sodium => egui::Color32::from_rgb(165, 126, 58),
        MapTheme::Topo => egui::Color32::from_rgb(195, 170, 60),
        MapTheme::Phosphor => egui::Color32::from_rgb(165, 160, 50),
        MapTheme::Thermal => egui::Color32::from_rgb(190, 175, 65),
        MapTheme::Ghost => egui::Color32::from_rgb(170, 170, 150),
        MapTheme::Akira => egui::Color32::from_rgb(195, 155, 45),
        MapTheme::Lunar => egui::Color32::from_rgb(165, 160, 145),
        MapTheme::Mars => egui::Color32::from_rgb(185, 165, 95),
        MapTheme::MarsDark => egui::Color32::from_rgb(160, 40, 40),
    }
}

/// Water features (rivers, lakes, streams).  Always blue-family but tuned
/// per theme so it reads clearly against each background palette.
pub fn water_color() -> egui::Color32 {
    match current() {
        MapTheme::Sodium => egui::Color32::from_rgb(145, 105, 52),
        MapTheme::Topo => egui::Color32::from_rgb(60, 145, 210),
        MapTheme::Phosphor => egui::Color32::from_rgb(50, 200, 160),
        MapTheme::Thermal => egui::Color32::from_rgb(90, 170, 255),
        MapTheme::Ghost => egui::Color32::from_rgb(120, 175, 220),
        MapTheme::Akira => egui::Color32::from_rgb(0, 185, 220),
        MapTheme::Lunar => egui::Color32::from_rgb(100, 140, 185),
        MapTheme::Mars => egui::Color32::from_rgb(70, 120, 175),
        MapTheme::MarsDark => egui::Color32::from_rgb(120, 20, 20),
    }
}

/// Major road overlay color.
pub fn road_major_color() -> egui::Color32 {
    match current() {
        MapTheme::Sodium => egui::Color32::from_rgb(255, 198, 100),
        MapTheme::Topo => egui::Color32::from_rgb(255, 210, 92),
        MapTheme::Phosphor => egui::Color32::from_rgb(230, 185, 70),
        MapTheme::Thermal => egui::Color32::from_rgb(255, 205, 110),
        MapTheme::Ghost => egui::Color32::from_rgb(212, 184, 150),
        MapTheme::Akira => egui::Color32::from_rgb(255, 150, 44),
        MapTheme::Lunar => egui::Color32::from_rgb(210, 200, 175),
        MapTheme::Mars => egui::Color32::from_rgb(230, 160, 120),
        MapTheme::MarsDark => egui::Color32::from_rgb(220, 40, 40),
    }
}

/// Waterway overlay color (rivers, streams, canals).
pub fn waterway_color() -> egui::Color32 {
    egui::Color32::from_rgb(40, 110, 180)
}

/// Tree / forest area fill color.
pub fn tree_color() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(30, 100, 40, 160)
}

/// Building footprint fill color.
pub fn building_color() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(120, 100, 80, 140)
}

/// Minor road overlay color.
pub fn road_minor_color() -> egui::Color32 {
    match current() {
        MapTheme::Sodium => egui::Color32::from_rgb(128, 92, 54),
        MapTheme::Topo => egui::Color32::from_rgb(116, 132, 142),
        MapTheme::Phosphor => egui::Color32::from_rgb(98, 132, 88),
        MapTheme::Thermal => egui::Color32::from_rgb(126, 106, 136),
        MapTheme::Ghost => egui::Color32::from_rgb(116, 128, 136),
        MapTheme::Akira => egui::Color32::from_rgb(118, 72, 60),
        MapTheme::Lunar => egui::Color32::from_rgb(118, 116, 108),
        MapTheme::Mars => egui::Color32::from_rgb(125, 90, 80),
        MapTheme::MarsDark => egui::Color32::from_rgb(110, 20, 20),
    }
}

/// Administrative boundary line color, keyed by OSM admin_level.
/// Level 2 = country, 4 = state/province, 6 = county, 8 = municipality.
/// Colors are theme-neutral — fixed RGBA values chosen to read on any dark palette.
pub fn admin_color(level: u8) -> egui::Color32 {
    match level {
        2 => egui::Color32::from_rgba_unmultiplied(220, 200, 60, 200), // country — bright yellow
        4 => egui::Color32::from_rgba_unmultiplied(180, 140, 60, 160), // state — amber
        6 => egui::Color32::from_rgba_unmultiplied(140, 110, 60, 120), // county — muted
        _ => egui::Color32::from_rgba_unmultiplied(100, 80, 60, 80),   // municipality — very subtle
    }
}

/// Administrative boundary stroke width, keyed by OSM admin_level.
pub fn admin_stroke_width(level: u8) -> f32 {
    match level {
        2 => 1.8,
        4 => 1.2,
        6 => 0.8,
        _ => 0.5,
    }
}

// ── Infrastructure layer colors ───────────────────────────────────────────────

/// High-voltage power line (≥300 kV).
pub fn power_ultra_color() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(255, 220, 50, 220)
}

/// High-voltage power line (100–299 kV).
pub fn power_high_color() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(240, 180, 40, 190)
}

/// Medium-voltage power line (50–99 kV).
pub fn power_med_color() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(200, 140, 40, 160)
}

/// Low/distribution power line (<50 kV).
pub fn power_low_color() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(160, 110, 40, 120)
}

/// Minor/service power line.
pub fn power_minor_color() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(120, 90, 40, 90)
}

/// Power substation or plant area.
pub fn power_area_color() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(240, 200, 60, 80)
}

/// Railway line.
pub fn rail_color() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(180, 180, 200, 200)
}

/// Subway / metro line.
pub fn rail_metro_color() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(100, 180, 240, 200)
}

/// Tram / light rail line.
pub fn rail_tram_color() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(120, 200, 160, 180)
}

/// Disused / abandoned railway.
pub fn rail_disused_color() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(120, 120, 120, 100)
}

/// Gas pipeline.
pub fn pipeline_gas_color() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(255, 140, 50, 180)
}

/// Oil pipeline.
pub fn pipeline_oil_color() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(200, 80, 40, 180)
}

/// Water/sewer pipeline.
pub fn pipeline_water_color() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(60, 160, 220, 160)
}

/// Other/unknown pipeline.
pub fn pipeline_other_color() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(160, 140, 120, 140)
}

/// Airport / aerodrome area.
pub fn aeroway_color() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(80, 160, 200, 160)
}

/// Runway line.
pub fn runway_color() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(120, 190, 220, 200)
}

/// Military base / danger area.
pub fn military_color() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(200, 50, 50, 120)
}

/// Communication tower / antenna.
pub fn comm_color() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(140, 220, 180, 180)
}

/// Industrial area.
pub fn industrial_color() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(160, 120, 80, 120)
}

/// Port / harbour area.
pub fn port_color() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(60, 180, 200, 160)
}

/// Government facility.
pub fn government_color() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(200, 160, 220, 140)
}

/// Surveillance camera / station.
pub fn surveillance_color() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(220, 80, 140, 180)
}
