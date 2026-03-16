use crate::city_catalog;
use crate::terrain_assets::{self, TerrainInventory};
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeoPoint {
    pub lat: f32,
    pub lon: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct GlobeViewState {
    pub yaw: f32,
    pub pitch: f32,
    pub local_center: GeoPoint,
    pub local_yaw: f32,
    pub local_pitch: f32,
    pub local_layer_spread: f32,
    pub zoom: f32,
    pub auto_spin: bool,
}

impl GlobeViewState {
    pub fn from_focus(point: GeoPoint) -> Self {
        let mut state = Self {
            yaw: 0.0,
            pitch: 0.0,
            local_center: point,
            local_yaw: -0.65,
            local_pitch: 0.98,
            local_layer_spread: 0.85,
            zoom: 1.0,
            auto_spin: true,
        };
        state.focus_on(point);
        state
    }

    pub fn focus_on(&mut self, point: GeoPoint) {
        self.yaw = point.lon.to_radians() - std::f32::consts::FRAC_PI_2;
        self.pitch = point.lat.to_radians().clamp(-1.1, 1.1);
        self.local_center = point;
        self.reset_local_camera();
    }

    pub fn reset_local_camera(&mut self) {
        self.local_yaw = -0.65;
        self.local_pitch = 0.98;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventSeverity {
    Critical,
    Elevated,
    Advisory,
}

impl EventSeverity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Critical => "Critical",
            Self::Elevated => "Elevated",
            Self::Advisory => "Advisory",
        }
    }

    pub fn color(self) -> egui::Color32 {
        match self {
            Self::Critical => egui::Color32::from_rgb(242, 90, 74),
            Self::Elevated => egui::Color32::from_rgb(255, 186, 73),
            Self::Advisory => egui::Color32::from_rgb(126, 208, 229),
        }
    }
}

#[derive(Clone, Debug)]
pub struct EventRecord {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub severity: EventSeverity,
    pub location_name: String,
    pub location: GeoPoint,
    pub source: String,
    pub occurred_at: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraConnectionState {
    Idle,
    Attempted,
    Reachable,
    Unreachable,
}

impl CameraConnectionState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Attempted => "attempted",
            Self::Reachable => "reachable",
            Self::Unreachable => "unreachable",
        }
    }

    pub fn color(self) -> egui::Color32 {
        match self {
            Self::Idle => egui::Color32::from_gray(150),
            Self::Attempted => egui::Color32::from_rgb(126, 208, 229),
            Self::Reachable => egui::Color32::from_rgb(117, 201, 104),
            Self::Unreachable => egui::Color32::from_rgb(242, 90, 74),
        }
    }
}

#[derive(Clone, Debug)]
pub struct CameraFeed {
    pub id: String,
    pub label: String,
    pub provider: String,
    pub kind: String,
    pub location: GeoPoint,
    pub stream_url: String,
    pub last_seen: String,
    pub status: CameraConnectionState,
}

#[derive(Clone, Debug)]
pub struct NearbyCamera {
    pub id: String,
    pub label: String,
    pub provider: String,
    pub kind: String,
    pub stream_url: String,
    pub last_seen: String,
    pub status: CameraConnectionState,
    pub distance_km: f32,
    pub location: GeoPoint,
}

pub struct AppModel {
    pub events: Vec<EventRecord>,
    pub cameras: Vec<CameraFeed>,
    pub selected_event_id: Option<String>,
    pub selected_camera_id: Option<String>,
    pub globe_view: GlobeViewState,
    pub focused_city_id: Option<String>,
    pub selected_root: Option<PathBuf>,
    pub terrain_library_open: bool,
    pub city_filter: String,
    pub selected_city_ids: BTreeSet<String>,
    pub activity_log: Vec<String>,
    pub factal_stream_status: String,
    pub camera_registry_status: String,
    pub terrain_inventory: TerrainInventory,
}

impl AppModel {
    pub fn seed_demo() -> Self {
        let selected_root = std::env::current_dir().ok();
        let terrain_inventory = TerrainInventory::detect_from(selected_root.as_deref());

        let events = vec![
            EventRecord {
                id: "evt-sf".into(),
                title: "Utility outage near Twin Peaks".into(),
                summary: "Curated alert placeholder representing a live urban disruption with confirmed location metadata.".into(),
                severity: EventSeverity::Critical,
                location_name: "San Francisco, USA".into(),
                location: GeoPoint {
                    lat: 37.7544,
                    lon: -122.4477,
                },
                source: "Factal stream".into(),
                occurred_at: "2026-03-15 05:42 UTC".into(),
            },
            EventRecord {
                id: "evt-nyc".into(),
                title: "Large structure fire in lower Manhattan".into(),
                summary: "Mock incident record used to validate event pinning and nearby camera discovery.".into(),
                severity: EventSeverity::Elevated,
                location_name: "New York City, USA".into(),
                location: GeoPoint {
                    lat: 40.7128,
                    lon: -74.0060,
                },
                source: "Factal stream".into(),
                occurred_at: "2026-03-15 05:50 UTC".into(),
            },
            EventRecord {
                id: "evt-tokyo".into(),
                title: "Flooding reported across a rail corridor".into(),
                summary: "Mock event with lower urgency to test sorting, selection, and globe overlays.".into(),
                severity: EventSeverity::Advisory,
                location_name: "Tokyo, Japan".into(),
                location: GeoPoint {
                    lat: 35.6764,
                    lon: 139.6500,
                },
                source: "Factal stream".into(),
                occurred_at: "2026-03-15 05:57 UTC".into(),
            },
        ];

        let cameras = vec![
            CameraFeed {
                id: "cam-sf-01".into(),
                label: "Twin Peaks North".into(),
                provider: "OpenCity SF".into(),
                kind: "traffic".into(),
                location: GeoPoint {
                    lat: 37.7549,
                    lon: -122.4471,
                },
                stream_url: "https://example.invalid/sf/twin-peaks-north".into(),
                last_seen: "36s ago".into(),
                status: CameraConnectionState::Idle,
            },
            CameraFeed {
                id: "cam-sf-02".into(),
                label: "Market Ridge".into(),
                provider: "Bay Civic Feeds".into(),
                kind: "public square".into(),
                location: GeoPoint {
                    lat: 37.7620,
                    lon: -122.4347,
                },
                stream_url: "https://example.invalid/sf/market-ridge".into(),
                last_seen: "1m ago".into(),
                status: CameraConnectionState::Reachable,
            },
            CameraFeed {
                id: "cam-nyc-01".into(),
                label: "Broadway South".into(),
                provider: "OpenStreetCam NY".into(),
                kind: "street".into(),
                location: GeoPoint {
                    lat: 40.7102,
                    lon: -74.0086,
                },
                stream_url: "https://example.invalid/nyc/broadway".into(),
                last_seen: "14s ago".into(),
                status: CameraConnectionState::Idle,
            },
            CameraFeed {
                id: "cam-nyc-02".into(),
                label: "Battery Overlook".into(),
                provider: "Harbor Public View".into(),
                kind: "harbor".into(),
                location: GeoPoint {
                    lat: 40.7041,
                    lon: -74.0170,
                },
                stream_url: "https://example.invalid/nyc/battery".into(),
                last_seen: "49s ago".into(),
                status: CameraConnectionState::Unreachable,
            },
            CameraFeed {
                id: "cam-tokyo-01".into(),
                label: "Shinjuku Crossing".into(),
                provider: "Tokyo Mobility Cams".into(),
                kind: "traffic".into(),
                location: GeoPoint {
                    lat: 35.6897,
                    lon: 139.7004,
                },
                stream_url: "https://example.invalid/tokyo/shinjuku".into(),
                last_seen: "21s ago".into(),
                status: CameraConnectionState::Idle,
            },
            CameraFeed {
                id: "cam-tokyo-02".into(),
                label: "Tokyo Station North".into(),
                provider: "Transit Surface Network".into(),
                kind: "station".into(),
                location: GeoPoint {
                    lat: 35.6828,
                    lon: 139.7668,
                },
                stream_url: "https://example.invalid/tokyo/station".into(),
                last_seen: "2m ago".into(),
                status: CameraConnectionState::Attempted,
            },
        ];

        let mut model = Self {
            events,
            cameras,
            selected_event_id: Some("evt-sf".into()),
            selected_camera_id: None,
            globe_view: GlobeViewState::from_focus(GeoPoint {
                lat: 37.7544,
                lon: -122.4477,
            }),
            focused_city_id: None,
            selected_root,
            terrain_library_open: false,
            city_filter: String::new(),
            selected_city_ids: BTreeSet::new(),
            activity_log: {
                let mut lines = vec![
                    "Factal demo stream connected to placeholder source.".into(),
                    "Camera registry loaded from mock public-feed catalog.".into(),
                ];
                lines.extend(terrain_inventory.status_lines());
                lines
            },
            factal_stream_status: "connected".into(),
            camera_registry_status: "loaded".into(),
            terrain_inventory,
        };

        if let Some(camera) = model.nearby_cameras(250.0).first() {
            model.selected_camera_id = Some(camera.id.clone());
        }

        model
    }

    pub fn set_selected_root(&mut self, root: PathBuf) {
        self.selected_root = Some(root.clone());
        self.terrain_inventory = TerrainInventory::detect_from(Some(root.as_path()));
        self.push_log(format!("Asset root selected: {}", root.display()));
        if let Some(srtm_root) = terrain_assets::find_srtm_root(Some(root.as_path())) {
            self.push_log(format!("Detected SRTM root: {}", srtm_root.display()));
        }
        self.push_log(format!(
            "Terrain refresh: {}",
            self.terrain_inventory.status_summary()
        ));
    }

    pub fn selected_event(&self) -> Option<&EventRecord> {
        let selected_id = self.selected_event_id.as_deref()?;
        self.events.iter().find(|event| event.id == selected_id)
    }

    pub fn focused_city(&self) -> Option<&'static city_catalog::CityEntry> {
        city_catalog::by_id(self.focused_city_id.as_deref()?)
    }

    pub fn terrain_focus_location(&self) -> Option<GeoPoint> {
        self.focused_city()
            .map(|city| city.location)
            .or_else(|| self.selected_event().map(|event| event.location))
    }

    pub fn terrain_focus_title(&self) -> String {
        if let Some(city) = self.focused_city() {
            format!("City focus: {}", city.name)
        } else if let Some(event) = self.selected_event() {
            event.title.clone()
        } else {
            "No terrain focus".into()
        }
    }

    pub fn terrain_focus_location_name(&self) -> String {
        if let Some(city) = self.focused_city() {
            format!("{}, {}", city.name, city.country)
        } else if let Some(event) = self.selected_event() {
            event.location_name.clone()
        } else {
            "No focus selected".into()
        }
    }

    pub fn terrain_focus_source(&self) -> &'static str {
        if self.focused_city_id.is_some() {
            "City catalog"
        } else {
            "Factal stream"
        }
    }

    pub fn terrain_focus_severity(&self) -> Option<EventSeverity> {
        self.focused_city_id
            .is_none()
            .then(|| self.selected_event().map(|event| event.severity))
            .flatten()
    }

    pub fn selected_camera(&self) -> Option<&CameraFeed> {
        let selected_id = self.selected_camera_id.as_deref()?;
        self.cameras.iter().find(|camera| camera.id == selected_id)
    }

    pub fn select_event(&mut self, event_id: &str) {
        if self.selected_event_id.as_deref() == Some(event_id) && self.focused_city_id.is_none() {
            return;
        }

        self.focused_city_id = None;
        self.selected_event_id = Some(event_id.to_owned());
        self.selected_camera_id = self
            .nearby_cameras(250.0)
            .first()
            .map(|camera| camera.id.clone());

        if let Some((title, location_name, location)) = self.selected_event().map(|event| {
            (
                event.title.clone(),
                event.location_name.clone(),
                event.location,
            )
        }) {
            self.globe_view.focus_on(location);
            self.push_log(format!("Event selected: {} ({})", title, location_name));
        }
    }

    pub fn focus_city(&mut self, city_id: &str) {
        let Some(city) = city_catalog::by_id(city_id) else {
            return;
        };

        self.focused_city_id = Some(city.id.to_owned());
        self.globe_view.focus_on(city.location);
        self.push_log(format!(
            "City focus selected: {}, {}",
            city.name, city.country
        ));
    }

    pub fn clear_city_focus(&mut self) {
        if self.focused_city_id.take().is_some() {
            self.push_log("City focus cleared; returning to event-driven focus.".into());
            if let Some(event) = self.selected_event() {
                self.globe_view.focus_on(event.location);
            }
        }
    }

    pub fn select_camera(&mut self, camera_id: &str) {
        if self.selected_camera_id.as_deref() == Some(camera_id) {
            return;
        }

        self.selected_camera_id = Some(camera_id.to_owned());

        if let Some(camera) = self.selected_camera() {
            self.push_log(format!(
                "Camera selected: {} [{}]",
                camera.label, camera.provider
            ));
        }
    }

    pub fn attempt_connect(&mut self, camera_id: &str) {
        if let Some(camera) = self
            .cameras
            .iter_mut()
            .find(|camera| camera.id == camera_id)
        {
            camera.status = if camera.status == CameraConnectionState::Reachable {
                CameraConnectionState::Reachable
            } else {
                CameraConnectionState::Attempted
            };

            let provider = camera.provider.clone();
            let label = camera.label.clone();
            let status = camera.status.label();

            self.selected_camera_id = Some(camera_id.to_owned());
            self.push_log(format!(
                "Feed connection attempted: {} [{}] -> {}",
                label, provider, status
            ));
        }
    }

    pub fn nearby_cameras(&self, radius_km: f32) -> Vec<NearbyCamera> {
        let Some(event) = self.selected_event() else {
            return Vec::new();
        };

        let mut nearby: Vec<_> = self
            .cameras
            .iter()
            .filter_map(|camera| {
                let distance_km = haversine_km(event.location, camera.location);
                (distance_km <= radius_km).then(|| NearbyCamera {
                    id: camera.id.clone(),
                    label: camera.label.clone(),
                    provider: camera.provider.clone(),
                    kind: camera.kind.clone(),
                    stream_url: camera.stream_url.clone(),
                    last_seen: camera.last_seen.clone(),
                    status: camera.status,
                    distance_km,
                    location: camera.location,
                })
            })
            .collect();

        nearby.sort_by(|left, right| left.distance_km.total_cmp(&right.distance_km));
        nearby
    }

    pub fn push_log(&mut self, line: String) {
        self.activity_log.push(line);

        if self.activity_log.len() > 12 {
            let extra = self.activity_log.len() - 12;
            self.activity_log.drain(0..extra);
        }
    }
}

pub fn haversine_km(a: GeoPoint, b: GeoPoint) -> f32 {
    let earth_radius_km = 6_371.0_f32;
    let lat_delta = (b.lat - a.lat).to_radians();
    let lon_delta = (b.lon - a.lon).to_radians();
    let lat_a = a.lat.to_radians();
    let lat_b = b.lat.to_radians();

    let sin_lat = (lat_delta / 2.0).sin();
    let sin_lon = (lon_delta / 2.0).sin();

    let inner = sin_lat * sin_lat + lat_a.cos() * lat_b.cos() * sin_lon * sin_lon;
    let arc = 2.0 * inner.sqrt().atan2((1.0 - inner).sqrt());

    earth_radius_km * arc
}
