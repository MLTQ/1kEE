use super::{EventSeverity, GeoPoint};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const SURFACE_SPEED_KM_S: f64 = 3.5;
const EARTH_RADIUS_KM: f64 = 6_371.0;
pub(crate) const REPLAY_SPEED: f64 = 120.0;
const MAX_LIVE_FRONTS: usize = 12;
const MAX_STREAM_QUAKES: usize = 256;

#[derive(Clone, Copy)]
pub(crate) enum Provider {
    Usgs,
    Emsc,
}

impl Provider {
    pub fn label(self) -> &'static str {
        match self {
            Self::Usgs => "USGS",
            Self::Emsc => "EMSC",
        }
    }
}

#[derive(Clone)]
pub(crate) struct Quake {
    pub id: String,
    pub title: String,
    pub epicenter: GeoPoint,
    pub origin_ms: i64,
    pub severity: EventSeverity,
    pub provider: Provider,
    pub has_station_picks: bool,
}

impl Quake {
    fn display_duration(&self) -> f64 {
        // Screen-time limits keep minor events from filling the globe. They
        // describe how long we draw the front, independent of shaking reach.
        match self.severity {
            EventSeverity::Critical => 90.0 * 60.0,
            EventSeverity::Elevated => 45.0 * 60.0,
            _ => 20.0 * 60.0,
        }
    }

    fn wave(&self, age: f64) -> Option<Wave<'_>> {
        let duration = self.display_duration();
        (0.0..duration).contains(&age).then(|| Wave {
            quake: self,
            age,
            opacity: ((duration - age) / 120.0).min(1.0) as f32,
        })
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

pub(crate) struct Wave<'a> {
    pub quake: &'a Quake,
    pub age: f64,
    pub opacity: f32,
}

impl Wave<'_> {
    pub fn angular_radius(&self, lag: f64) -> Option<f32> {
        let angle = (self.age - lag) * SURFACE_SPEED_KM_S / EARTH_RADIUS_KM;
        (0.0..std::f64::consts::PI)
            .contains(&angle)
            .then_some(angle as f32)
    }
}

struct Replay {
    quake: Quake,
    started: f64,
}

#[derive(Default)]
pub(crate) struct SeismicLayer {
    pub enabled: bool,
    pub live_stations: crate::station_replay::live::LiveStations,
    pub stream_status: StreamStatus,
    pub station_replay: crate::station_replay::StationReplay,
    quakes: Vec<Quake>,
    streamed: Vec<Quake>,
    replay: Option<Replay>,
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

    pub fn replay_candidate(&self, selected_id: Option<&str>) -> Option<&Quake> {
        selected_id
            .and_then(|id| self.quakes.iter().find(|quake| quake.id == id))
            .or_else(|| {
                self.quakes
                    .first()
                    .into_iter()
                    .chain(self.streamed.first())
                    .max_by_key(|quake| quake.origin_ms)
            })
    }

    pub fn station_replay_candidate(&self, selected_id: Option<&str>) -> Option<&Quake> {
        selected_id
            .and_then(|id| self.quakes.iter().find(|quake| quake.id == id))
            .or_else(|| self.quakes.iter().find(|quake| quake.has_station_picks))
            .or_else(|| self.quakes.first())
    }

    pub fn start_replay(&mut self, quake: Quake, time: f64) {
        self.replay = Some(Replay {
            quake,
            started: time,
        });
    }

    pub fn stop_replay(&mut self) {
        self.replay = None;
    }

    pub fn replay_wave(&self, time: f64) -> Option<Wave<'_>> {
        let replay = self.replay.as_ref()?;
        replay.quake.wave((time - replay.started) * REPLAY_SPEED)
    }

    pub fn live_waves(&self, unix_secs: f64) -> impl Iterator<Item = Wave<'_>> {
        let active = move |quake: &Quake| unix_secs - quake.origin_ms as f64 / 1000.0;
        let streamed = self
            .streamed
            .iter()
            .filter_map(move |quake| quake.wave(active(quake)));
        let fallback = self.quakes.iter().filter_map(move |quake| {
            let wave = quake.wave(active(quake))?;
            let covered = self.streamed.iter().any(|streamed| {
                streamed.same_origin(quake) && streamed.wave(active(streamed)).is_some()
            });
            (!covered).then_some(wave)
        });
        streamed.chain(fallback).take(MAX_LIVE_FRONTS)
    }
}

pub(crate) fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
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
            provider: Provider::Usgs,
            has_station_picks: false,
        }
    }

    #[test]
    fn live_fronts_use_origin_time_and_exclude_future_or_expired_events() {
        let mut layer = SeismicLayer::default();
        layer.replace_quakes(vec![
            quake("old", 0),
            quake("active", 1_250_500),
            quake("future", 1_400_000),
        ]);
        let mut live = layer.live_waves(1_350.5);
        let wave = live.next().unwrap();
        assert_eq!(wave.quake.id, "active");
        let distance = wave.angular_radius(0.0).unwrap() as f64 * EARTH_RADIUS_KM;
        assert!((distance - 350.0).abs() < 0.001);
        assert!(live.next().is_none());
    }

    #[test]
    fn replay_uses_selected_quake_and_ends_on_its_own_clock() {
        let mut layer = SeismicLayer::default();
        layer.replace_quakes(vec![quake("old", 10_000), quake("latest", 30_000)]);
        assert_eq!(layer.replay_candidate(None).unwrap().id, "latest");
        let selected = layer.replay_candidate(Some("old")).unwrap().clone();
        layer.start_replay(selected, 500.0);
        layer.replace_quakes(vec![quake("latest", 30_000)]);

        let wave = layer.replay_wave(501.0).unwrap();
        assert_eq!(wave.quake.id, "old");
        assert_eq!(wave.age, 120.0);
        assert!(wave.angular_radius(150.0).is_none());
        assert!(layer.replay_wave(510.0).is_none());
        layer.stop_replay();
        assert!(layer.replay_wave(501.0).is_none());
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
            .live_waves(1_100.0)
            .map(|wave| wave.quake.id.as_str())
            .collect();
        assert_eq!(ids, ["emsc", "distinct"]);
        assert_eq!(layer.live_waves(1_100.0).next().unwrap().age, 87.0);
        layer.update_streamed("emsc", None);
        assert_eq!(layer.live_waves(1_100.0).count(), 2);
    }
}
