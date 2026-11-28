//! Multi-sensor spatial-temporal convergence detector and broadcast director scoring.
//!
//! Correlates live in-memory feeds (Factal, USGS/EMSC seismic, SeedLink triggers,
//! NASA FIRMS fires, GhostMaps, public cameras, and viewport-local ADS-B/AIS)
//! with zero extra network calls, grouping nearby reports into coherent stories
//! and computing dynamic camera framing & broadcast priority scores.

use super::{
    CameraConnectionState, CameraFeed, EventRecord, EventSeverity, FireDetection, FlightTrack,
    GeoPoint, MovingTrack, haversine_km,
};
use crate::fire_source::FireConfidence;
use crate::station_replay::live::LiveStations;
use std::collections::{HashMap, HashSet, VecDeque};

/// Spatial clustering radius (km) for merging related incident reports.
const CLUSTER_RADIUS_KM: f32 = 85.0;
/// Spatial correlation radius (km) for secondary sensors (fires, cameras, seismic triggers).
const SENSOR_CORRELATION_KM: f32 = 110.0;
/// Viewport-local track enrichment radius (km) around an active story.
const TRACK_ENRICH_KM: f32 = 120.0;
/// Maximum age (seconds) for two reports to reinforce the same story cluster.
const CLUSTER_WINDOW_SECS: i64 = 6 * 3600;
/// Fire grid cell resolution in degrees (0.5° ≈ 55 km at equator).
const FIRE_CELL_DEG: f32 = 0.5;
/// Minimum interval (wall seconds) between automatic convergence recomputations.
const RECOMPUTE_INTERVAL_SECS: f64 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SignalDomain {
    Factal,
    Seismic,
    ThermalFire,
    GhostMaps,
    LiveCameras,
    Airspace,
    Maritime,
}

impl SignalDomain {
    pub fn short_label(self) -> &'static str {
        match self {
            Self::Factal => "FACTAL",
            Self::Seismic => "SEISMIC",
            Self::ThermalFire => "FIRMS FIRE",
            Self::GhostMaps => "GHOSTMAPS",
            Self::LiveCameras => "CAMS",
            Self::Airspace => "ADS-B",
            Self::Maritime => "AIS",
        }
    }
}

/// Adaptive camera framing and dwell multiplier for a Follow Events stop.
#[derive(Clone, Copy, Debug)]
pub struct StopDirective {
    pub globe_zoom: f64,
    pub local_zoom: f64,
    pub local_pitch: f64,
    pub orbit_radius_deg: f64,
    pub dwell_multiplier: f64,
    /// Gentle signed log-zoom push per second during dwell (C¹-matched on arrival).
    pub dwell_zoom_drift: f64,
}

impl Default for StopDirective {
    fn default() -> Self {
        Self {
            globe_zoom: 35.0,
            local_zoom: 9.5,
            local_pitch: 0.90,
            orbit_radius_deg: 0.8,
            dwell_multiplier: 1.0,
            dwell_zoom_drift: 0.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ConvergenceCluster {
    pub anchor_event_id: String,
    pub anchor_event: EventRecord,
    pub member_event_ids: Vec<String>,
    pub center: GeoPoint,
    pub footprint_km: f32,
    pub domains: Vec<SignalDomain>,
    pub factal_count: usize,
    pub quake_max_mag: Option<f32>,
    pub fire_count: u32,
    pub fire_max_frp: f32,
    pub camera_count: usize,
    pub seismic_triggers: usize,
    pub air_tracks: usize,
    pub sea_tracks: usize,
    pub latest_unix: i64,
    pub age_hours: f32,
    pub priority_score: f32,
}

impl ConvergenceCluster {
    /// True when two or more independent sensor domains corroborate this location.
    pub fn is_convergence(&self) -> bool {
        self.domains.len() >= 2
    }

    /// Concise broadcast badge for map cards, chyron, and OBS overlays.
    pub fn badge_text(&self) -> Option<String> {
        if self.domains.len() >= 2 {
            let mut parts: Vec<String> = Vec::with_capacity(self.domains.len());
            for &domain in &self.domains {
                match domain {
                    SignalDomain::Seismic => {
                        if let Some(mag) = self.quake_max_mag {
                            parts.push(format!("M{mag:.1} SEISMIC"));
                        } else if self.seismic_triggers > 0 {
                            parts.push(format!("{} SEISMIC", self.seismic_triggers));
                        } else {
                            parts.push("SEISMIC".into());
                        }
                    }
                    SignalDomain::ThermalFire => {
                        parts.push(format!("{} FIRES", self.fire_count));
                    }
                    SignalDomain::LiveCameras => {
                        parts.push(format!(
                            "{} {}",
                            self.camera_count,
                            if self.camera_count == 1 { "CAM" } else { "CAMS" }
                        ));
                    }
                    SignalDomain::Airspace => {
                        parts.push(format!("{} AIR", self.air_tracks));
                    }
                    SignalDomain::Maritime => {
                        parts.push(format!("{} SEA", self.sea_tracks));
                    }
                    other => parts.push(other.short_label().into()),
                }
            }
            return Some(format!("CONVERGENCE · {}", parts.join(" + ")));
        }
        if self.factal_count >= 2 {
            return Some(format!("DEVELOPING · {} REPORTS", self.factal_count));
        }
        None
    }

    /// Derive adaptive camera framing & dwell pacing from spatial footprint and sensor richness.
    pub fn directive(&self, adaptive_framing: bool) -> StopDirective {
        if !adaptive_framing {
            return StopDirective::default();
        }
        let wide = self.footprint_km > 60.0 || self.quake_max_mag.is_some_and(|m| m >= 5.5);
        let urban_cams = self.camera_count >= 2 && self.footprint_km <= 35.0;

        let (globe_zoom, local_zoom, local_pitch, orbit_radius_deg) = if wide {
            let spread = (self.footprint_km / 180.0).clamp(0.0, 1.0) as f64;
            (
                (28.0 - 10.0 * spread).clamp(16.0, 30.0),
                (7.8 - 2.3 * spread).clamp(5.0, 8.5),
                0.82,
                (0.95 + 0.45 * spread).clamp(0.85, 1.45),
            )
        } else if urban_cams {
            (41.0, 11.8, 0.98, 0.58)
        } else {
            (35.0, 9.5, 0.90, 0.80)
        };

        // Longer dwell for multi-sensor convergences with cameras; brisker for thin advisories.
        let mut dwell_multiplier = 1.0_f64;
        if self.domains.len() >= 3 {
            dwell_multiplier = 1.45;
        } else if self.domains.len() == 2 {
            dwell_multiplier = 1.25;
        } else if self.priority_score < 2.2 {
            dwell_multiplier = 0.75;
        }
        if self.camera_count >= 2 {
            dwell_multiplier = (dwell_multiplier + 0.10).min(1.55);
        }

        // Subtle cinematic push-in during dwell (clamped well inside zoom bounds).
        let dwell_zoom_drift = if urban_cams { 0.0045 } else { 0.0025 };

        StopDirective {
            globe_zoom,
            local_zoom,
            local_pitch,
            orbit_radius_deg,
            dwell_multiplier,
            dwell_zoom_drift,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct FireCell {
    count: u32,
    high_conf: u32,
    max_frp_mw: f32,
    sum_lat: f64,
    sum_lon: f64,
}

impl FireCell {
    fn centroid(self) -> GeoPoint {
        let denom = self.count.max(1) as f64;
        GeoPoint {
            lat: (self.sum_lat / denom) as f32,
            lon: (self.sum_lon / denom) as f32,
        }
    }
}

#[derive(Default)]
struct FireGrid {
    revision: u64,
    cells: HashMap<(i32, i32), FireCell>,
}

impl FireGrid {
    fn sync(&mut self, revision: u64, fires: &[FireDetection]) {
        if self.revision == revision && (!fires.is_empty() || self.cells.is_empty()) {
            return;
        }
        self.revision = revision;
        self.cells.clear();
        if fires.is_empty() {
            return;
        }
        self.cells.reserve((fires.len() / 8).min(8192));
        for fire in fires {
            if !fire.location.lat.is_finite() || !fire.location.lon.is_finite() {
                continue;
            }
            let key = fire_cell_key(fire.location);
            let cell = self.cells.entry(key).or_default();
            cell.count += 1;
            if fire.confidence == FireConfidence::High {
                cell.high_conf += 1;
            }
            cell.max_frp_mw = cell.max_frp_mw.max(fire.frp_mw);
            cell.sum_lat += f64::from(fire.location.lat);
            cell.sum_lon += f64::from(fire.location.lon);
        }
    }

    fn nearby(&self, center: GeoPoint) -> (u32, f32) {
        if self.cells.is_empty() {
            return (0, 0.0);
        }
        let (cy, cx) = fire_cell_key(center);
        let mut total = 0u32;
        let mut max_frp = 0.0_f32;
        for dy in -2..=2 {
            for dx in -2..=2 {
                if let Some(cell) = self.cells.get(&(cy + dy, cx + dx))
                    && haversine_km(center, cell.centroid()) <= SENSOR_CORRELATION_KM
                {
                    total = total.saturating_add(cell.count);
                    max_frp = max_frp.max(cell.max_frp_mw);
                }
            }
        }
        (total, max_frp)
    }

    /// Top standalone wildfire complexes (high count or extreme FRP) even without Factal coverage.
    fn major_hotspots(&self, limit: usize) -> Vec<((i32, i32), FireCell)> {
        let mut list: Vec<_> = self
            .cells
            .iter()
            .filter(|(_, cell)| cell.count >= 12 || (cell.count >= 4 && cell.max_frp_mw >= 120.0))
            .map(|(&k, &c)| (k, c))
            .collect();
        list.sort_by(|a, b| {
            let sa = a.1.count as f32 + a.1.max_frp_mw * 0.05;
            let sb = b.1.count as f32 + b.1.max_frp_mw * 0.05;
            sb.total_cmp(&sa)
        });
        list.truncate(limit);
        list
    }
}

fn fire_cell_key(loc: GeoPoint) -> (i32, i32) {
    (
        (loc.lat / FIRE_CELL_DEG).floor() as i32,
        (loc.lon / FIRE_CELL_DEG).floor() as i32,
    )
}

#[derive(Default)]
pub struct ConvergenceState {
    clusters: Vec<ConvergenceCluster>,
    event_to_cluster: HashMap<String, usize>,
    fire_grid: FireGrid,
    last_computed: f64,
    recent_anchors: VecDeque<String>,
    interrupted_anchors: HashSet<String>,
}

impl ConvergenceState {
    pub fn clusters(&self) -> &[ConvergenceCluster] {
        &self.clusters
    }

    pub fn cluster_for_event(&self, event_id: &str) -> Option<&ConvergenceCluster> {
        let &idx = self.event_to_cluster.get(event_id)?;
        self.clusters.get(idx)
    }

    pub fn badge_for_event(&self, event_id: &str) -> Option<String> {
        self.cluster_for_event(event_id)?.badge_text()
    }

    pub fn reset_tour_history(&mut self) {
        self.recent_anchors.clear();
        self.interrupted_anchors.clear();
    }

    pub fn record_visit(&mut self, event_id: &str) {
        let anchor = self
            .cluster_for_event(event_id)
            .map(|c| c.anchor_event_id.clone())
            .unwrap_or_else(|| event_id.to_owned());
        self.recent_anchors.retain(|id| id != &anchor);
        self.recent_anchors.push_back(anchor);
        while self.recent_anchors.len() > 12 {
            self.recent_anchors.pop_front();
        }
    }

    /// Recompute convergence clusters if due (or forced).
    #[allow(clippy::too_many_arguments)]
    pub fn refresh(
        &mut self,
        now_anim: f64,
        now_unix: i64,
        force: bool,
        events: &[EventRecord],
        cameras: &[CameraFeed],
        fires: &[FireDetection],
        fire_revision: u64,
        live_stations: &LiveStations,
        flights: &[FlightTrack],
        vessels: &[MovingTrack],
    ) {
        if !force && self.last_computed > 0.0 && now_anim - self.last_computed < RECOMPUTE_INTERVAL_SECS
        {
            return;
        }
        self.last_computed = now_anim;
        self.fire_grid.sync(fire_revision, fires);

        // Gather active SeedLink STA/LTA trigger points (within last 10 minutes).
        let now_ms = now_unix * 1000;
        let trigger_points: Vec<GeoPoint> = live_stations
            .activity
            .iter()
            .filter_map(|(id, act)| {
                let trig = act.motion.as_ref()?;
                if now_ms.saturating_sub(trig.received_ms) > 10 * 60_000 {
                    return None;
                }
                live_stations.stations.get(id)?.location(now_ms)
            })
            .collect();

        // Filter candidate anchor events: Factal, significant USGS quakes (M3.5+), GhostMaps.
        let mut eligible: Vec<(&EventRecord, i64, f32, SignalDomain, Option<f32>)> = Vec::new();
        for event in events {
            if !valid_location(event.location) {
                continue;
            }
            if event.id.starts_with("factal-") && event.factal_brief.is_some() {
                let ts = event_unix(event, now_unix);
                let sev = factal_severity_base(event);
                eligible.push((event, ts, sev, SignalDomain::Factal, None));
            } else if event.id.starts_with("usgs-") {
                let mag = parse_quake_magnitude(&event.title).unwrap_or(2.5);
                if mag >= 3.5 {
                    let ts = event_unix(event, now_unix);
                    let base = quake_base_score(mag);
                    eligible.push((event, ts, base, SignalDomain::Seismic, Some(mag)));
                }
            } else if event.is_ghostmaps() {
                let ts = event_unix(event, now_unix);
                eligible.push((event, ts, 2.0, SignalDomain::GhostMaps, None));
            }
        }

        // Sort strongest/newest first so cluster anchors are always the highest-signal event.
        eligible.sort_by(|a, b| {
            b.2.total_cmp(&a.2)
                .then_with(|| b.1.cmp(&a.1))
                .then_with(|| a.0.id.cmp(&b.0.id))
        });

        let mut clusters: Vec<ConvergenceCluster> = Vec::new();
        for (event, ts, base, domain, mag) in eligible {
            // Try merging into an existing nearby cluster within the time window.
            let mut merged_into = None;
            for (idx, cluster) in clusters.iter_mut().enumerate() {
                let dist = haversine_km(cluster.center, event.location);
                if dist <= CLUSTER_RADIUS_KM
                    && (cluster.latest_unix - ts).abs() <= CLUSTER_WINDOW_SECS
                {
                    cluster.member_event_ids.push(event.id.clone());
                    cluster.footprint_km = cluster.footprint_km.max(dist);
                    if !cluster.domains.contains(&domain) {
                        cluster.domains.push(domain);
                    }
                    if domain == SignalDomain::Factal {
                        cluster.factal_count += 1;
                    }
                    if let Some(m) = mag {
                        cluster.quake_max_mag = Some(cluster.quake_max_mag.map_or(m, |cur| cur.max(m)));
                    }
                    cluster.latest_unix = cluster.latest_unix.max(ts);
                    merged_into = Some(idx);
                    break;
                }
            }
            if merged_into.is_none() {
                clusters.push(ConvergenceCluster {
                    anchor_event_id: event.id.clone(),
                    anchor_event: event.clone(),
                    member_event_ids: vec![event.id.clone()],
                    center: event.location,
                    footprint_km: 12.0,
                    domains: vec![domain],
                    factal_count: usize::from(domain == SignalDomain::Factal),
                    quake_max_mag: mag,
                    fire_count: 0,
                    fire_max_frp: 0.0,
                    camera_count: 0,
                    seismic_triggers: 0,
                    air_tracks: 0,
                    sea_tracks: 0,
                    latest_unix: ts,
                    age_hours: 0.0,
                    priority_score: base,
                });
            }
        }

        // Also promote major standalone wildfire complexes if not already covered by an event cluster.
        for ((cy, cx), cell) in self.fire_grid.major_hotspots(8) {
            let center = cell.centroid();
            if clusters
                .iter()
                .any(|c| haversine_km(c.center, center) <= SENSOR_CORRELATION_KM)
            {
                continue;
            }
            let id = format!("convergence-fire-{cy}_{cx}");
            let event = EventRecord {
                id: id.clone(),
                title: format!(
                    "Wildfire thermal complex · {} VIIRS detections",
                    cell.count
                ),
                summary: format!(
                    "{} active NASA FIRMS detections (peak {:.0} MW FRP)",
                    cell.count, cell.max_frp_mw
                ),
                occurred_at: "Last 24h".into(),
                severity: if cell.max_frp_mw >= 200.0 || cell.count >= 40 {
                    EventSeverity::Elevated
                } else {
                    EventSeverity::Advisory
                },
                location_name: format!("{:.2}°, {:.2}°", center.lat, center.lon),
                location: center,
                source: "NASA FIRMS".into(),
                factal_brief: None,
            };
            clusters.push(ConvergenceCluster {
                anchor_event_id: id.clone(),
                anchor_event: event,
                member_event_ids: vec![id],
                center,
                footprint_km: 45.0,
                domains: vec![SignalDomain::ThermalFire],
                factal_count: 0,
                quake_max_mag: None,
                fire_count: cell.count,
                fire_max_frp: cell.max_frp_mw,
                camera_count: 0,
                seismic_triggers: 0,
                air_tracks: 0,
                sea_tracks: 0,
                latest_unix: now_unix.saturating_sub(1800),
                age_hours: 0.5,
                priority_score: 2.8,
            });
        }

        // Pre-filter previewable live cameras once.
        let live_cams: Vec<GeoPoint> = cameras
            .iter()
            .filter(|c| {
                c.status != CameraConnectionState::Unreachable
                    && valid_location(c.location)
                    && !c.stream_url.trim().is_empty()
            })
            .map(|c| c.location)
            .collect();

        // Correlate secondary sensor domains and compute final composite Broadcast Priority Score.
        for cluster in &mut clusters {
            // Fires
            let (fires_near, max_frp) = self.fire_grid.nearby(cluster.center);
            if fires_near >= 3 || max_frp >= 45.0 {
                cluster.fire_count = cluster.fire_count.max(fires_near);
                cluster.fire_max_frp = cluster.fire_max_frp.max(max_frp);
                if !cluster.domains.contains(&SignalDomain::ThermalFire) {
                    cluster.domains.push(SignalDomain::ThermalFire);
                }
            }

            // Live SeedLink tremor triggers
            let trig_count = trigger_points
                .iter()
                .filter(|&&pt| haversine_km(cluster.center, pt) <= SENSOR_CORRELATION_KM * 1.5)
                .count();
            if trig_count > 0 {
                cluster.seismic_triggers = trig_count;
                if !cluster.domains.contains(&SignalDomain::Seismic) {
                    cluster.domains.push(SignalDomain::Seismic);
                }
            }

            // Reachable nearby cameras
            let cam_count = live_cams
                .iter()
                .filter(|&&pt| haversine_km(cluster.center, pt) <= SENSOR_CORRELATION_KM)
                .count();
            cluster.camera_count = cam_count;
            if cam_count > 0 && !cluster.domains.contains(&SignalDomain::LiveCameras) {
                cluster.domains.push(SignalDomain::LiveCameras);
            }

            // On-arrival / viewport-local ADS-B & AIS enrichment
            let air_count = flights
                .iter()
                .filter(|f| {
                    !f.on_ground && haversine_km(cluster.center, f.location) <= TRACK_ENRICH_KM
                })
                .count();
            cluster.air_tracks = air_count;
            if air_count >= 5 && !cluster.domains.contains(&SignalDomain::Airspace) {
                cluster.domains.push(SignalDomain::Airspace);
            }

            let sea_count = vessels
                .iter()
                .filter(|v| haversine_km(cluster.center, v.location) <= TRACK_ENRICH_KM)
                .count();
            cluster.sea_tracks = sea_count;
            if sea_count >= 5 && !cluster.domains.contains(&SignalDomain::Maritime) {
                cluster.domains.push(SignalDomain::Maritime);
            }

            // Compute composite priority score.
            let age_secs = now_unix.saturating_sub(cluster.latest_unix).max(0);
            let age_hours = age_secs as f32 / 3600.0;
            cluster.age_hours = age_hours;

            let base = if cluster.anchor_event.id.starts_with("factal-") {
                factal_severity_base(&cluster.anchor_event)
            } else if let Some(mag) = cluster.quake_max_mag {
                quake_base_score(mag)
            } else {
                cluster.priority_score
            };

            // Recency multiplier: fresh (<1h) stories get a breaking boost; stale (>18h) fade.
            let recency = if age_hours <= 1.0 {
                1.35
            } else {
                (-age_hours / 8.0).exp().clamp(0.25, 1.25)
            };

            // Count distinct non-camera event/hazard domains + camera bonus.
            let convergence_mult =
                1.0 + 0.55 * (cluster.domains.len().saturating_sub(1).min(4) as f32);
            let cluster_bonus = 1.0 + 0.20 * (cluster.factal_count.saturating_sub(1).min(4) as f32);
            let cam_bonus = 1.0 + 0.25 * (cluster.camera_count.min(4) as f32 / 4.0);

            cluster.priority_score = base * recency * convergence_mult * cluster_bonus * cam_bonus;
        }

        clusters.sort_by(|a, b| {
            b.priority_score
                .total_cmp(&a.priority_score)
                .then_with(|| b.latest_unix.cmp(&a.latest_unix))
                .then_with(|| a.anchor_event_id.cmp(&b.anchor_event_id))
        });

        let mut map = HashMap::with_capacity(clusters.len() * 2);
        for (idx, cluster) in clusters.iter().enumerate() {
            for member_id in &cluster.member_event_ids {
                map.insert(member_id.clone(), idx);
            }
        }
        self.clusters = clusters;
        self.event_to_cluster = map;
    }

    /// Choose the next Follow Events stop using convergence priority + regional theater continuity.
    pub fn select_next_stop(
        &mut self,
        current_id: Option<&str>,
        current_loc: Option<GeoPoint>,
        adaptive_framing: bool,
    ) -> Option<(EventRecord, StopDirective)> {
        if self.clusters.is_empty() {
            return None;
        }
        let current_anchor = current_id
            .and_then(|id| self.cluster_for_event(id))
            .map(|c| c.anchor_event_id.as_str())
            .or(current_id);

        let cooldown = 6usize.min(self.clusters.len().saturating_sub(1) / 2);
        let mut best_idx = None;
        let mut best_score = f32::NEG_INFINITY;

        for (idx, cluster) in self.clusters.iter().enumerate() {
            if Some(cluster.anchor_event_id.as_str()) == current_anchor {
                continue;
            }
            if self
                .recent_anchors
                .iter()
                .rev()
                .take(cooldown)
                .any(|id| id == &cluster.anchor_event_id)
            {
                continue;
            }
            // Regional theater continuity: gently prefer nearby stories before trans-oceanic jumps.
            let proximity_mult = match current_loc {
                Some(loc) => {
                    let km = haversine_km(loc, cluster.center);
                    if km <= 1_200.0 {
                        1.28
                    } else if km <= 2_500.0 {
                        1.12
                    } else {
                        1.0
                    }
                }
                None => 1.0,
            };
            let score = cluster.priority_score * proximity_mult;
            if score > best_score {
                best_score = score;
                best_idx = Some(idx);
            }
        }

        let idx = best_idx?;
        let cluster = &self.clusters[idx];
        let event = cluster.anchor_event.clone();
        let directive = cluster.directive(adaptive_framing);
        let anchor = cluster.anchor_event_id.clone();
        self.record_visit(&anchor);
        Some((event, directive))
    }

    /// Check whether a brand-new, high-priority breaking story or multi-sensor convergence
    /// should preempt a lower-priority dwell (with an anti-thrash minimum dwell guard).
    pub fn breaking_interrupt(
        &mut self,
        current_id: Option<&str>,
        dwell_elapsed: f64,
        min_dwell_before_interrupt: f64,
        adaptive_framing: bool,
    ) -> Option<(EventRecord, StopDirective)> {
        if dwell_elapsed < min_dwell_before_interrupt {
            return None;
        }
        let current_score = current_id
            .and_then(|id| self.cluster_for_event(id))
            .map_or(0.0, |c| c.priority_score);
        let current_anchor = current_id
            .and_then(|id| self.cluster_for_event(id))
            .map(|c| c.anchor_event_id.as_str())
            .or(current_id);

        let candidate = self.clusters.iter().find(|c| {
            Some(c.anchor_event_id.as_str()) != current_anchor
                && !self.interrupted_anchors.contains(&c.anchor_event_id)
                && c.age_hours <= 0.25 // Breaking: < 15 minutes old
                && c.priority_score >= 10.0
                && c.priority_score >= current_score * 1.75
        })?;

        let event = candidate.anchor_event.clone();
        let directive = candidate.directive(adaptive_framing);
        let anchor = candidate.anchor_event_id.clone();
        self.interrupted_anchors.insert(anchor.clone());
        self.record_visit(&anchor);
        Some((event, directive))
    }
}

/// Extract earthquake magnitude from a USGS event title such as `"M4.8 - 12 km N of ..."`.
pub fn parse_quake_magnitude(title: &str) -> Option<f32> {
    let rest = title.trim().strip_prefix('M')?;
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    rest[..end].parse::<f32>().ok().filter(|v| v.is_finite())
}

fn valid_location(loc: GeoPoint) -> bool {
    loc.lat.is_finite() && loc.lon.is_finite() && loc.lat.abs() <= 90.0 && loc.lon.abs() <= 180.0
}

fn factal_severity_base(event: &EventRecord) -> f32 {
    let sev = event
        .factal_brief
        .as_ref()
        .and_then(|b| b.severity_value)
        .unwrap_or(match event.severity {
            EventSeverity::Critical => 4,
            EventSeverity::Elevated => 2,
            EventSeverity::Advisory => 1,
            EventSeverity::Unrated => 0,
        });
    match sev {
        i64::MIN..=1 => 1.2,
        2 => 1.8,
        3 => 3.8,
        4 => 6.8,
        _ => 10.5,
    }
}

fn quake_base_score(mag: f32) -> f32 {
    if mag >= 6.5 {
        13.0
    } else if mag >= 5.5 {
        8.5
    } else if mag >= 4.5 {
        4.5
    } else {
        2.0
    }
}

fn event_unix(event: &EventRecord, fallback_now: i64) -> i64 {
    if let Some(raw) = event
        .factal_brief
        .as_ref()
        .and_then(|b| b.occurred_at_raw.as_deref())
        && let Some(ts) = crate::event_store::parse_iso_to_unix(raw)
    {
        return ts;
    }
    crate::event_store::parse_iso_to_unix(&event.occurred_at).unwrap_or(fallback_now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FactalBrief;

    fn factal(id: &str, lat: f32, lon: f32, sev: i64, iso: &str) -> EventRecord {
        EventRecord {
            id: id.into(),
            title: format!("Factal {id}"),
            summary: String::new(),
            occurred_at: iso.into(),
            severity: if sev >= 4 {
                EventSeverity::Critical
            } else if sev >= 3 {
                EventSeverity::Elevated
            } else {
                EventSeverity::Advisory
            },
            location_name: "Test".into(),
            location: GeoPoint { lat, lon },
            source: "Factal".into(),
            factal_brief: Some(FactalBrief {
                factal_id: id.into(),
                severity_value: Some(sev),
                occurred_at_raw: Some(iso.into()),
                point_wkt: None,
                vertical: None,
                subvertical: None,
                topics: vec![],
                content: None,
                raw_json_pretty: String::new(),
            }),
        }
    }

    fn quake(id: &str, lat: f32, lon: f32, mag: f32, iso: &str) -> EventRecord {
        EventRecord {
            id: format!("usgs-{id}"),
            title: format!("M{mag:.1} - Near Test"),
            summary: String::new(),
            occurred_at: iso.into(),
            severity: if mag >= 6.0 {
                EventSeverity::Critical
            } else {
                EventSeverity::Elevated
            },
            location_name: "Test".into(),
            location: GeoPoint { lat, lon },
            source: "USGS".into(),
            factal_brief: None,
        }
    }

    #[test]
    fn clusters_multi_sensor_signals_and_elevates_convergences() {
        let now = 1_750_000_000_i64;
        let fresh_iso = chrono::DateTime::from_timestamp(now - 300, 0)
            .unwrap()
            .to_rfc3339();
        let stale_iso = chrono::DateTime::from_timestamp(now - 20 * 3600, 0)
            .unwrap()
            .to_rfc3339();

        let events = vec![
            // Stale standalone sev-3 event far away
            factal("factal-stale", 51.5, -0.1, 3, &stale_iso),
            // Fresh sev-3 Factal + M5.2 quake + nearby camera = 3-domain convergence
            factal("factal-hot", 35.68, 139.76, 3, &fresh_iso),
            quake("hot", 35.72, 139.80, 5.2, &fresh_iso),
        ];
        let cameras = vec![CameraFeed {
            id: "cam-1".into(),
            label: "Tokyo Cam".into(),
            provider: "Public".into(),
            kind: "Webcam".into(),
            location: GeoPoint {
                lat: 35.69,
                lon: 139.75,
            },
            stream_url: "https://example.com/cam.jpg".into(),
            status: CameraConnectionState::Reachable,
            last_seen: "now".into(),
        }];
        let mut state = ConvergenceState::default();
        state.refresh(
            1.0,
            now,
            true,
            &events,
            &cameras,
            &[],
            0,
            &LiveStations::default(),
            &[],
            &[],
        );

        assert_eq!(state.clusters().len(), 2);
        let top = &state.clusters()[0];
        assert_eq!(top.anchor_event_id, "usgs-hot");
        assert!(top.is_convergence());
        assert_eq!(top.domains.len(), 3);
        let badge = top.badge_text().unwrap();
        assert!(badge.contains("CONVERGENCE"));
        assert!(badge.contains("M5.2 SEISMIC"));
        assert!(badge.contains("FACTAL"));
        assert!(badge.contains("1 CAM"));
        assert!(top.priority_score > state.clusters()[1].priority_score * 3.0);
    }

    #[test]
    fn regional_continuity_prefers_nearby_story_over_distant_tie() {
        let now = 1_750_000_000_i64;
        let iso = chrono::DateTime::from_timestamp(now - 600, 0)
            .unwrap()
            .to_rfc3339();
        let events = vec![
            factal("factal-a", 34.0, -118.2, 3, &iso), // LA (current)
            factal("factal-b", 37.7, -122.4, 3, &iso), // SF (~560 km)
            factal("factal-c", 48.8, 2.35, 3, &iso),   // Paris (~9100 km)
        ];
        let mut state = ConvergenceState::default();
        state.refresh(
            1.0,
            now,
            true,
            &events,
            &[],
            &[],
            0,
            &LiveStations::default(),
            &[],
            &[],
        );
        let (next, _) = state
            .select_next_stop(
                Some("factal-a"),
                Some(GeoPoint {
                    lat: 34.0,
                    lon: -118.2,
                }),
                true,
            )
            .unwrap();
        assert_eq!(next.id, "factal-b");
    }

    #[test]
    fn parse_quake_magnitude_handles_usgs_titles() {
        assert_eq!(parse_quake_magnitude("M5.4 - 12 km SW of Ridgecrest"), Some(5.4));
        assert_eq!(parse_quake_magnitude("M2.5 - Alaska"), Some(2.5));
        assert_eq!(parse_quake_magnitude("Wildfire near Napa"), None);
    }
}
