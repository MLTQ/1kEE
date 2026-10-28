//! Shared capacity colors, legend and wording for globe/local pipeline overlays.
use egui::Color32;
use tile_archive::pipelines::{Capacity, Info};

pub(super) const UNKNOWN: Color32 = Color32::from_rgb(100, 106, 115);
const ZERO: Color32 = Color32::from_rgb(177, 151, 203);
const GAS: [Color32; 4] = [
    Color32::from_rgb(65, 100, 151),
    Color32::from_rgb(44, 143, 201),
    Color32::from_rgb(65, 199, 227),
    Color32::from_rgb(180, 242, 255),
];
const OIL: [Color32; 4] = [
    Color32::from_rgb(145, 89, 56),
    Color32::from_rgb(202, 124, 46),
    Color32::from_rgb(247, 181, 67),
    Color32::from_rgb(255, 234, 169),
];

fn capacity_color(info: &Info) -> Color32 {
    let Some(capacity) = info.capacity.as_ref().filter(|c| c.valid()) else {
        return UNKNOWN;
    };
    let (palette, thresholds) = match (info.product.as_str(), capacity.unit.as_str()) {
        ("gas", "bcm/y of gas") => (GAS, [1.0, 10.0, 50.0]),
        ("oil" | "liquids" | "mixed", "boe/d") => (OIL, [100_000.0, 500_000.0, 1_000_000.0]),
        _ => return UNKNOWN,
    };
    if capacity.value == 0.0 {
        ZERO
    } else {
        palette[thresholds.iter().filter(|&&t| capacity.value >= t).count()]
    }
}

pub(super) fn color(info: &Info, enabled: bool, product_colors: [Color32; 3]) -> Color32 {
    let color = if enabled {
        capacity_color(info)
    } else {
        product_colors[match info.product.as_str() {
            "gas" => 0,
            "oil" => 1,
            _ => 2,
        }]
    };
    if info.historical {
        color.gamma_multiply(0.35)
    } else if info.planned {
        color.gamma_multiply(0.60)
    } else {
        color
    }
}

pub(super) fn label(capacity: Option<&Capacity>) -> String {
    match capacity.filter(|c| c.valid()) {
        Some(c) => format!("{} {} (source-reported; not actual flow)", c.value, c.unit),
        None => "Not reported in available data".into(),
    }
}

pub(super) fn map_label(painter: &egui::Painter) {
    let text = painter.layout_no_wrap(
        "Pipeline capacity · not actual flow".into(),
        egui::FontId::proportional(11.0),
        Color32::WHITE,
    );
    let pos = painter.clip_rect().right_top() + egui::vec2(-text.size().x - 16.0, 12.0);
    painter.rect_filled(
        egui::Rect::from_min_size(pos, text.size()).expand(5.0),
        3.0,
        Color32::from_black_alpha(210),
    );
    painter.galley(pos, text, Color32::WHITE);
}

fn swatch(ui: &mut egui::Ui, color: Color32, text: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(22.0, 10.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 2.0, color);
        ui.small(text);
    });
}

pub(super) fn legend(ui: &mut egui::Ui) {
    ui.strong("Pipeline capacity · not actual flow");
    ui.small("Brighter = greater reported capacity. Separate scales:");
    ui.columns(2, |cols| {
        cols[0].small("Gas · bcm/year");
        for (color, text) in GAS
            .into_iter()
            .zip([">0 to <1", "1 to <10", "10 to <50", "50+"])
        {
            swatch(&mut cols[0], color, text);
        }
        cols[1].small("Oil / liquids · boe/day");
        for (color, text) in
            OIL.into_iter()
                .zip([">0 to <100k", "100k to <500k", "500k to <1m", "1m+"])
        {
            swatch(&mut cols[1], color, text);
        }
    });
    swatch(ui, ZERO, "Reported zero capacity");
    swatch(
        ui,
        UNKNOWN,
        "Capacity unavailable / units not on these scales",
    );
    ui.small("bcm = billion cubic metres; boe = barrels of oil equivalent. Source units retained.");
    ui.small("Planned and historical routes remain dimmed. Hover for value and source release.");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(product: &str, value: Option<f64>, unit: &str) -> Info {
        serde_json::from_value(serde_json::json!({
            "source":"gem-gas","source_id":"P1","name":"Test","operator":"","owner":"",
            "product":product,"status":"operating","accuracy":"unknown","historical":false,"planned":false,
            "capacity":value.map(|v| serde_json::json!({"value":v,"unit":unit,"source_release":"Test release"}))
        })).unwrap()
    }

    #[test]
    fn unknown_zero_and_incompatible_units_are_not_conflated() {
        let unknown = info("gas", None, "bcm/y of gas");
        let zero = info("gas", Some(0.0), "bcm/y of gas");
        let positive = info("gas", Some(0.1), "bcm/y of gas");
        assert_eq!(capacity_color(&unknown), UNKNOWN);
        assert_eq!(capacity_color(&zero), ZERO);
        assert_ne!(capacity_color(&positive), ZERO);
        assert_ne!(capacity_color(&positive), UNKNOWN);
        assert_eq!(capacity_color(&info("gas", Some(10.0), "boe/d")), UNKNOWN);
        assert_eq!(
            capacity_color(&info("oil", Some(10.0), "bcm/y of gas")),
            UNKNOWN
        );
        assert_eq!(
            capacity_color(&info("gas", Some(-1.0), "bcm/y of gas")),
            UNKNOWN
        );
        assert!(label(zero.capacity.as_ref()).starts_with("0 bcm/y of gas"));
        assert!(!label(unknown.capacity.as_ref()).contains('0'));
    }

    #[test]
    fn separate_fixed_scales_keep_their_thresholds_and_status_filters() {
        for (product, unit, values, palette) in [
            ("gas", "bcm/y of gas", [0.1, 1.0, 10.0, 50.0], GAS),
            (
                "oil",
                "boe/d",
                [1.0, 100_000.0, 500_000.0, 1_000_000.0],
                OIL,
            ),
        ] {
            for (value, expected) in values.into_iter().zip(palette) {
                let mut record = info(product, Some(value), unit);
                assert_eq!(color(&record, true, [Color32::WHITE; 3]), expected);
                assert_eq!(color(&record, false, [Color32::WHITE; 3]), Color32::WHITE);
                record.planned = true;
                assert_eq!(
                    color(&record, true, [Color32::WHITE; 3]),
                    expected.gamma_multiply(0.60)
                );
                assert!(!tile_archive::pipelines::Filter::default().accepts(&record));
            }
        }
    }
}
