//! Session-only administrative overlay choices, independent of cache availability.
pub const LEVELS: [(u8, &str); 10] = [
    (2, "Countries"),
    (3, "Intermediate regions"),
    (4, "States / provinces"),
    (5, "Regions / districts"),
    (6, "Counties / districts"),
    (7, "Subdistricts"),
    (8, "Cities / municipalities"),
    (9, "Local subdivisions"),
    (10, "Neighborhoods"),
    (11, "Local areas"),
];

pub struct AdminOptions {
    pub cache_root: Option<std::path::PathBuf>,
    pub enabled: [bool; 10],
    pub shade: bool,
    pub opacity: f32,
}
impl Default for AdminOptions {
    fn default() -> Self {
        Self {
            cache_root: None,
            enabled: [
                true, false, true, false, true, false, true, false, false, false,
            ],
            shade: false,
            opacity: 0.16,
        }
    }
}
impl AdminOptions {
    pub fn levels(&self) -> Vec<u8> {
        LEVELS
            .iter()
            .zip(self.enabled)
            .filter_map(|((level, _), enabled)| enabled.then_some(*level))
            .collect()
    }
}
