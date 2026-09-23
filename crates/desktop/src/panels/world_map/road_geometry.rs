//! Build reusable, bounded GPU batches off the UI thread without dropping ways.
use super::super::local_contour_pass::{LocalBatchId, LocalSegmentInstance, LocalTileBatch};
use crate::model::GeoPoint;
use crate::osm_ingest::RoadPolyline;
use std::path::Path;
use std::sync::Arc;

const MAX_SOURCE_POINTS_PER_ROAD: usize = 192;
const SEGMENTS_PER_BATCH: usize = 65_536; // 2 MiB, at most 6 MiB uploaded per frame.

pub(super) struct RoadGeometry {
    pub major: Vec<LocalTileBatch>,
    pub minor: Vec<LocalTileBatch>,
}

impl RoadGeometry {
    pub fn build(
        roads: impl IntoIterator<Item = RoadPolyline>,
        root: Option<&Path>,
        version: u64,
        colors: [egui::Color32; 2],
    ) -> Self {
        let mut major = BatchBuilder::new(true, version, colors[0]);
        let mut minor = BatchBuilder::new(false, version, colors[1]);
        for road in roads {
            let points = crate::feature_heights::prepare(
                &road.points,
                road.elevations.as_deref(),
                MAX_SOURCE_POINTS_PER_ROAD,
                3.0,
                |pt| super::super::srtm_stream::sample_elevation_m(root, pt),
            );
            let builder = if matches!(
                road.road_class.as_str(),
                "motorway" | "trunk" | "primary" | "secondary"
            ) {
                &mut major
            } else {
                &mut minor
            };
            builder.push_line(&points);
        }
        Self {
            major: major.finish(),
            minor: minor.finish(),
        }
    }
}

struct BatchBuilder {
    batches: Vec<LocalTileBatch>,
    current: Vec<LocalSegmentInstance>,
    template: LocalSegmentInstance,
    major: bool,
    version: u64,
}

impl BatchBuilder {
    fn new(major: bool, version: u64, color: egui::Color32) -> Self {
        Self {
            batches: Vec::new(),
            current: Vec::new(),
            template: LocalSegmentInstance::line([0.0; 3], [0.0; 3], color, major),
            major,
            version,
        }
    }
    fn push_line(&mut self, points: &[(GeoPoint, f32)]) {
        // Split segments, not vertices: a chunk boundary never loses a join.
        for pair in points.windows(2) {
            let a = [pair[0].0.lon, pair[0].0.lat, pair[0].1];
            let b = [pair[1].0.lon, pair[1].0.lat, pair[1].1];
            if !a.iter().chain(&b).all(|v| v.is_finite()) {
                continue;
            }
            self.current.push(self.template.with_endpoints(a, b));
            if self.current.len() == SEGMENTS_PER_BATCH {
                self.flush();
            }
        }
    }
    fn flush(&mut self) {
        if self.current.is_empty() {
            return;
        }
        self.batches.push(LocalTileBatch {
            id: LocalBatchId::Road {
                major: self.major,
                chunk: self.batches.len(),
            },
            version: self.version,
            instances: Arc::new(std::mem::take(&mut self.current)),
        });
    }
    fn finish(mut self) -> Vec<LocalTileBatch> {
        self.flush();
        self.batches
    }
}

#[cfg(test)]
#[path = "road_geometry_tests.rs"]
mod tests;
