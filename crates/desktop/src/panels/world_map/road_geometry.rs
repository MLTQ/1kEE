//! Build reusable, bounded GPU batches off the UI thread without dropping ways.
use super::super::local_contour_pass::{LocalBatchId, LocalSegmentInstance, LocalTileBatch};
use super::super::projection::geo_to_ecef;
use super::super::unified_line_pass::{RtcLineBatch, RtcSegmentInstance};
use crate::model::{ActiveBody, GeoPoint};
use crate::osm_ingest::RoadPolyline;
use glam::DVec3;
use std::path::Path;
use std::sync::Arc;

const MAX_SOURCE_POINTS_PER_ROAD: usize = 192;
const SEGMENTS_PER_BATCH: usize = 65_536; // 2 MiB, at most 6 MiB uploaded per frame.

pub(super) struct RoadGeometry {
    pub major: Vec<LocalTileBatch>,
    pub minor: Vec<LocalTileBatch>,
    pub rtc_major: Vec<RtcLineBatch>,
    pub rtc_minor: Vec<RtcLineBatch>,
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
        let (major, rtc_major) = major.finish_with_rtc();
        let (minor, rtc_minor) = minor.finish_with_rtc();
        Self {
            major,
            minor,
            rtc_major,
            rtc_minor,
        }
    }
}

struct BatchBuilder {
    batches: Vec<LocalTileBatch>,
    rtc_batches: Vec<RtcLineBatch>,
    current: Vec<LocalSegmentInstance>,
    rtc_current: Vec<RtcSegmentInstance>,
    origin: Option<DVec3>,
    template: LocalSegmentInstance,
    color: egui::Color32,
    major: bool,
    version: u64,
}

impl BatchBuilder {
    fn new(major: bool, version: u64, color: egui::Color32) -> Self {
        Self {
            batches: Vec::new(),
            rtc_batches: Vec::new(),
            current: Vec::new(),
            rtc_current: Vec::new(),
            origin: None,
            template: LocalSegmentInstance::line([0.0; 3], [0.0; 3], color, major),
            color,
            major,
            version,
        }
    }
    fn push_line(&mut self, points: &[(GeoPoint, f32)]) {
        let r = ActiveBody::Earth.radius_m();
        // Split segments, not vertices: a chunk boundary never loses a join.
        for pair in points.windows(2) {
            let a = [pair[0].0.lon, pair[0].0.lat, pair[0].1];
            let b = [pair[1].0.lon, pair[1].0.lat, pair[1].1];
            if !a.iter().chain(&b).all(|v| v.is_finite()) {
                continue;
            }
            let origin = *self.origin.get_or_insert_with(|| {
                geo_to_ecef(pair[0].0.lat as f64, pair[0].0.lon as f64, 0.0, r)
            });
            self.current.push(self.template.with_endpoints(a, b));
            self.rtc_current.push(RtcSegmentInstance::from_geo(
                pair[0].0,
                pair[0].1,
                pair[1].0,
                pair[1].1,
                origin,
                r,
                self.color,
                self.major,
            ));
            if self.current.len() == SEGMENTS_PER_BATCH {
                self.flush();
            }
        }
    }
    fn flush(&mut self) {
        if self.current.is_empty() {
            return;
        }
        let chunk_idx = self.batches.len();
        self.batches.push(LocalTileBatch {
            occlusion: Default::default(),
            id: LocalBatchId::Road {
                major: self.major,
                chunk: chunk_idx,
            },
            version: self.version,
            instances: Arc::new(std::mem::take(&mut self.current)),
        });
        let key = 0x524F_4144_0000_0000_u64
            | ((u64::from(self.major)) << 31)
            | (chunk_idx as u64);
        self.rtc_batches.push(RtcLineBatch {
            key,
            version: self.version,
            origin_ecef: self.origin.take().unwrap_or(DVec3::X * ActiveBody::Earth.radius_m()),
            segments: Arc::new(std::mem::take(&mut self.rtc_current)),
            stroke_minor_px: 1.0,
            stroke_major_px: 1.8,
            alpha: 0.96,
            extra_radial_m: 1.5,
            depth_bias_ndc: 0.00025,
            horizon_cull: true,
            palette_override: None,
        });
    }
    fn finish_with_rtc(mut self) -> (Vec<LocalTileBatch>, Vec<RtcLineBatch>) {
        self.flush();
        (self.batches, self.rtc_batches)
    }
    #[cfg(test)]
    fn finish(self) -> Vec<LocalTileBatch> {
        self.finish_with_rtc().0
    }
}

#[cfg(test)]
#[path = "road_geometry_tests.rs"]
mod tests;
