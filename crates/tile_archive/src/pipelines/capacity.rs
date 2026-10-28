//! Source-reported capacity, deliberately independent of measured throughput.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Capacity {
    pub value: f64,
    pub unit: String,
    pub source_release: String,
}

impl Capacity {
    pub fn valid(&self) -> bool {
        self.value.is_finite()
            && self.value >= 0.0
            && !self.unit.trim().is_empty()
            && !self.source_release.trim().is_empty()
    }
}
