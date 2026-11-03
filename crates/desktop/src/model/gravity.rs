//! Session-only selection of physically distinct gravity anomaly products.
use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GravityKind {
    FreeAir,
    Bouguer,
    Isostatic,
}
impl GravityKind {
    pub const ALL: [Self; 3] = [Self::FreeAir, Self::Bouguer, Self::Isostatic];
    pub fn label(self) -> &'static str {
        match self {
            Self::FreeAir => "Free-air",
            Self::Bouguer => "Bouguer",
            Self::Isostatic => "Isostatic residual",
        }
    }
    pub fn color_limit(self) -> f32 {
        match self {
            Self::FreeAir => 150.0,
            Self::Bouguer => 350.0,
            Self::Isostatic => 100.0,
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::FreeAir => "Gravity corrected for observation height.",
            Self::Bouguer => "Removes modeled terrain mass to reveal density contrasts.",
            Self::Isostatic => "Also removes modeled deep crustal compensation.",
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct GravityMap {
    pub id: String,
    pub kind: GravityKind,
    pub label: String,
    pub spacing: String,
    pub note: String,
}

pub fn gravity_maps() -> &'static [GravityMap] {
    static MAPS: OnceLock<Vec<GravityMap>> = OnceLock::new();
    MAPS.get_or_init(|| {
        serde_json::from_str(include_str!("gravity_maps.json"))
            .expect("bundled gravity map catalog must be valid")
    })
}

#[derive(Default)]
pub struct GravityState {
    pub enabled: bool,
    selected: usize,
}
impl GravityState {
    pub fn map(&self) -> &'static GravityMap {
        &gravity_maps()[self.selected]
    }
    pub fn select_kind(&mut self, kind: GravityKind) {
        if self.map().kind != kind {
            self.selected = gravity_maps().iter().position(|m| m.kind == kind).unwrap();
        }
    }
    pub fn select_map(&mut self, index: usize) {
        if index < gravity_maps().len() {
            self.selected = index;
        }
    }
    pub fn selected(&self) -> usize {
        self.selected
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_families_have_unique_real_products_and_selection_stays_valid() {
        let maps = gravity_maps();
        let ids: std::collections::HashSet<_> = maps.iter().map(|m| &m.id).collect();
        assert_eq!(ids.len(), maps.len());
        let mut state = GravityState::default();
        assert!(!state.enabled);
        for kind in GravityKind::ALL {
            state.select_kind(kind);
            assert_eq!(state.map().kind, kind);
            let before = state.selected();
            state.select_map(usize::MAX);
            assert_eq!(state.selected(), before);
        }
    }
}
