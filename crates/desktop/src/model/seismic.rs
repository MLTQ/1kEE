use super::{EventSeverity, GeoPoint};

const EARTH_RADIUS_KM: f64 = 6_371.0;
const MAX_LIVE_EPICENTERS: usize = 12;
const MAX_STREAM_QUAKES: usize = 256;

#[derive(Clone)]
pub(crate) struct Quake {
    pub id: String,
    pub title: String,
    pub epicenter: GeoPoint,
    pub origin_ms: i64,
    pub severity: EventSeverity,
    pub has_station_picks: bool,
}

impl Quake {
    fn is_recent(&self, now_ms: i64) -> bool {
        let minutes = match self.severity {
            EventSeverity::Critical => 90,
            EventSeverity::Elevated => 45,
            _ => 20,
        };
        (0..minutes * 60_000).contains(&now_ms.saturating_sub(self.origin_ms))
    }

    fn same_origin(&self, other: &Self) -> bool {
        if self.origin_ms.abs_diff(other.origin_ms) > 25_000 {
            return false;
        }
        // Catalogs assign different IDs and may revise the same origin. Match
        // nearby reports with a wrap-safe great-circle distance.
        let a = self.epicenter.lat.to_radians();
        let b = other.epicenter.lat.to_radians();
        let dlon = (self.epicenter.lon - other.epicenter.lon).to_radians();
        let haversine =
            ((a - b) * 0.5).sin().powi(2) + a.cos() * b.cos() * (dlon * 0.5).sin().powi(2);
        let limit = (75.0 / (2.0 * EARTH_RADIUS_KM)).sin().powi(2) as f32;
        haversine <= limit
    }
}

pub(crate) fn quake_severity(magnitude: Option<f64>) -> EventSeverity {
    match magnitude {
        Some(magnitude) if magnitude >= 6.0 => EventSeverity::Critical,
        Some(magnitude) if magnitude >= 4.5 => EventSeverity::Elevated,
        _ => EventSeverity::Advisory,
    }
}

#[derive(Default)]
pub(crate) struct SeismicLayer {
    pub enabled: bool,
    pub seismogram: crate::station_replay::waveform::Seismogram,
    pub live_stations: crate::station_replay::live::LiveStations,
    pub stream_status: StreamStatus,
    pub station_replay: crate::station_replay::StationReplay,
    quakes: Vec<Quake>,
    streamed: Vec<Quake>,
}

#[derive(Default)]
pub(crate) enum StreamStatus {
    #[default]
    Connecting,
    Live,
    Reconnecting,
}

impl StreamStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Live => "connected",
            Self::Reconnecting => "reconnecting",
        }
    }
}

impl SeismicLayer {
    pub fn replace_quakes(&mut self, mut quakes: Vec<Quake>) {
        quakes.sort_unstable_by(|a, b| b.origin_ms.cmp(&a.origin_ms).then(a.id.cmp(&b.id)));
        self.quakes = quakes;
    }

    pub fn update_streamed(&mut self, id: &str, quake: Option<Quake>) {
        self.streamed.retain(|existing| existing.id != id);
        if let Some(quake) = quake {
            let index = self
                .streamed
                .partition_point(|existing| existing.origin_ms >= quake.origin_ms);
            self.streamed.insert(index, quake);
            self.streamed.truncate(MAX_STREAM_QUAKES);
        }
    }

    pub fn station_replay_candidate(&self, selected_id: Option<&str>) -> Option<&Quake> {
        selected_id
            .and_then(|id| self.quakes.iter().find(|quake| quake.id == id))
            .or_else(|| self.quakes.iter().find(|quake| quake.has_station_picks))
            .or_else(|| self.quakes.first())
    }

    pub fn recent_epicenters(&self, now_ms: i64) -> impl Iterator<Item = &Quake> {
        let recent_streamed = self
            .streamed
            .iter()
            .filter(move |quake| quake.is_recent(now_ms));
        let fallback = self.quakes.iter().filter(move |quake| {
            quake.is_recent(now_ms)
                && !self
                    .streamed
                    .iter()
                    .any(|report| report.is_recent(now_ms) && report.same_origin(quake))
        });
        recent_streamed.chain(fallback).take(MAX_LIVE_EPICENTERS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quake(id: &str, origin_ms: i64) -> Quake {
        Quake {
            id: id.to_owned(),
            title: id.to_owned(),
            epicenter: GeoPoint { lat: 0.0, lon: 0.0 },
            origin_ms,
            severity: EventSeverity::Advisory,
            has_station_picks: false,
        }
    }

    #[test]
    fn epicenters_exclude_future_and_expired_reports() {
        let mut layer = SeismicLayer::default();
        layer.replace_quakes(vec![
            quake("old", 0),
            quake("active", 1_250_500),
            quake("future", 1_400_000),
        ]);
        let ids: Vec<_> = layer
            .recent_epicenters(1_350_500)
            .map(|q| q.id.as_str())
            .collect();
        assert_eq!(ids, ["active"]);
    }

    #[test]
    fn streamed_revisions_take_precedence_over_the_same_usgs_origin() {
        let mut layer = SeismicLayer::default();
        let mut usgs = quake("usgs", 1_000_000);
        usgs.epicenter.lon = 179.9;
        let mut streamed = quake("emsc", 1_010_000);
        streamed.epicenter.lon = -179.9;
        layer.replace_quakes(vec![usgs, quake("distinct", 1_020_000)]);
        layer.update_streamed("emsc", Some(streamed.clone()));
        streamed.origin_ms += 3_000;
        layer.update_streamed("emsc", Some(streamed));

        let ids: Vec<_> = layer
            .recent_epicenters(1_100_000)
            .map(|quake| quake.id.as_str())
            .collect();
        assert_eq!(ids, ["emsc", "distinct"]);
        assert_eq!(
            layer.recent_epicenters(1_100_000).next().unwrap().origin_ms,
            1_013_000
        );
        layer.update_streamed("emsc", None);
        assert_eq!(layer.recent_epicenters(1_100_000).count(), 2);
    }
}
