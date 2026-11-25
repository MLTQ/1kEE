//! Select a station using the measured arrival time in the published origin.
use super::super::{Pick, StationId, day::Inventory};
use crate::model::GeoPoint;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

pub(crate) struct StationStop {
    pub event: Arc<str>,
    pub station: StationId,
    pub pick: Pick,
    pub point: GeoPoint,
}

type StationPicks = Vec<(StationId, Pick)>;

#[derive(Default)]
pub(crate) struct QuakeFollow {
    since: Option<i64>,
    pending: BTreeMap<(i64, Arc<str>), StationPicks>,
    visited: HashMap<Arc<str>, i64>,
}

impl QuakeFollow {
    pub fn set_enabled(&mut self, enabled: bool, now: i64) {
        if enabled {
            self.since.get_or_insert(now);
            let cutoff = now - 2 * 3_600_000;
            self.since = self.since.map(|since| since.max(cutoff));
            self.visited.retain(|_, origin| *origin >= cutoff);
            self.pending.retain(|(origin, _), _| *origin >= cutoff);
        } else if self.since.is_some() {
            *self = Self::default();
        }
    }

    pub(in crate::station_replay) fn observe(
        &mut self,
        event: &Arc<str>,
        origin: i64,
        picks: &[(StationId, Pick)],
    ) {
        if !self.since.is_some_and(|start| origin >= start) || self.visited.contains_key(event) {
            return;
        }
        self.pending.retain(|(_, id), _| id != event);
        self.pending.insert((origin, event.clone()), picks.to_vec());
    }

    pub fn next(&mut self, stations: &Inventory) -> Option<StationStop> {
        let (key, stop) = self.pending.iter().find_map(|(key, picks)| {
            let (id, pick, point) = picks
                .iter()
                .filter(|(_, p)| p.time_ms >= key.0)
                .filter_map(|(id, p)| Some((id, p, stations.get(id)?.location(p.time_ms)?)))
                .min_by(|(a, p, _), (b, q, _)| p.time_ms.cmp(&q.time_ms).then_with(|| a.cmp(b)))?;
            Some((
                key.clone(),
                StationStop {
                    event: key.1.clone(),
                    station: id.clone(),
                    pick: pick.clone(),
                    point,
                },
            ))
        })?;
        self.pending.remove(&key);
        self.visited.insert(key.1, key.0);
        Some(stop)
    }
}
