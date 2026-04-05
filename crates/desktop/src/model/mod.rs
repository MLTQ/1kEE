mod arcgis;
mod cameras;
mod events;
mod flights;
mod geo;
mod geojson_layer;
mod kml_layer;
pub mod replay;
mod vessels;

pub use arcgis::*;
pub use cameras::*;
pub use events::*;
pub use flights::*;
pub use geo::*;
pub use geojson_layer::*;
pub use replay::{ActiveFlare, ReplayState};
pub use vessels::*;

use crate::city_catalog;
use crate::osm_ingest::{self, OsmInventory};
use crate::settings_store;
use crate::terrain_assets::{self, TerrainInventory};
use std::collections::BTreeSet;
use std::path::PathBuf;

pub struct AppModel {
    pub events: Vec<EventRecord>,
    pub cameras: Vec<CameraFeed>,
    /// Live AIS vessel positions; refreshed periodically by `moving_tracks`.
    pub tracks: Vec<MovingTrack>,
    /// Live ADS-B flight positions; refreshed periodically by `flight_tracks`.
    pub flights: Vec<FlightTrack>,
    pub selected_event_id: Option<String>,
    pub selected_camera_id: Option<String>,
    /// MMSI string of the currently-selected vessel (for detail panel).
    pub selected_track_mmsi: Option<u64>,
    /// ICAO24 hex of the currently-selected flight (for detail panel).
    pub selected_flight_icao24: Option<String>,
    pub globe_view: GlobeViewState,
    pub focused_city_id: Option<String>,
    pub cinematic_mode: bool,
    pub moon_mode: bool,
    pub map_theme: crate::theme::MapTheme,
    pub show_event_markers: bool,
    pub show_coastlines: bool,
    pub show_graticule: bool,
    pub show_reticle: bool,
    pub show_major_roads: bool,
    pub show_minor_roads: bool,
    pub show_water: bool,
    pub show_beam: bool,
    pub fill_elevation: bool,
    pub show_bathymetry: bool,
    pub show_contours: bool,
    pub show_trees: bool,
    pub show_buildings: bool,
    pub show_admin: bool,
    pub show_ships: bool,
    pub show_flights: bool,
    /// User-uploaded vector overlay layers (GeoJSON, KML, or KMZ).
    pub geojson_layers: Vec<GeoJsonLayer>,
    /// ArcGIS FeatureServer sources added by the user.
    pub arcgis_sources: Vec<ArcGisSourceRef>,
    /// Merged features from all enabled source/layer combos (refreshed each frame).
    pub arcgis_features: Vec<ArcGisFeature>,
    /// Selected feature for the detail panel: (source_url, object_id).
    pub selected_arcgis_feature: Option<(String, i64)>,
    pub selected_root: Option<PathBuf>,
    pub factal_settings_open: bool,
    pub factal_brief_open: bool,
    pub factal_api_key: String,
    pub windy_webcams_api_key: String,
    pub ny511_api_key: String,
    pub aisstream_api_key: String,
    pub settings_asset_root: String,
    pub settings_data_root: String,
    pub settings_derived_root: String,
    pub settings_srtm_root: String,
    pub settings_planet_path: String,
    pub settings_gdal_bin_dir: String,
    pub settings_osmium_bin_dir: String,
    pub settings_prefer_overpass: bool,
    pub terrain_library_open: bool,
    pub city_filter: String,
    pub selected_city_ids: BTreeSet<String>,
    pub activity_log: Vec<String>,
    pub log_collapsed: bool,
    pub factal_stream_status: String,
    pub camera_registry_status: String,
    // ── Replay mode ──────────────────────────────────────────────────────────
    pub replay_mode: bool,
    /// Start of the replay window (unix seconds).
    pub replay_from_unix: i64,
    /// End of the replay window (unix seconds, ≤ now).
    pub replay_to_unix: i64,
    /// Edit buffer for the "from" date text input (kept in sync with replay_from_unix).
    pub replay_from_str: String,
    /// Edit buffer for the "to" date text input (kept in sync with replay_to_unix).
    pub replay_to_str: String,
    /// Wall-clock duration of the full replay in seconds (60–300).
    pub replay_duration_secs: u32,
    pub replay_state: Option<ReplayState>,
    /// Human-readable status for the history backfill fetch.
    pub replay_history_status: String,
    pub terrain_inventory: TerrainInventory,
    pub osm_inventory: OsmInventory,
}

impl AppModel {
    pub fn seed_demo() -> Self {
        let _ = settings_store::ensure_default_asset_layout();
        let app_settings = settings_store::load_app_settings();
        let selected_root = settings_store::effective_asset_root();
        let terrain_inventory = TerrainInventory::detect_from(selected_root.as_deref());
        let osm_runtime_store = osm_ingest::ensure_runtime_store(selected_root.as_deref());
        let osm_inventory = OsmInventory::detect_from(selected_root.as_deref());
        let factal_api_key = app_settings.factal_api_key.trim().to_owned();
        let windy_webcams_api_key = app_settings.windy_webcams_api_key.trim().to_owned();
        let ny511_api_key = app_settings.ny511_api_key.trim().to_owned();
        let aisstream_api_key = app_settings.aisstream_api_key.trim().to_owned();

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
                factal_brief: None,
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
                factal_brief: None,
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
                factal_brief: None,
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
            tracks: Vec::new(),
            flights: Vec::new(),
            selected_event_id: Some("evt-sf".into()),
            selected_camera_id: None,
            selected_track_mmsi: None,
            selected_flight_icao24: None,
            globe_view: GlobeViewState::from_focus(GeoPoint {
                lat: 37.7544,
                lon: -122.4477,
            }),
            focused_city_id: None,
            cinematic_mode: false,
            moon_mode: false,
            map_theme: crate::theme::MapTheme::Topo,
            show_event_markers: true,
            show_coastlines: true,
            show_graticule: true,
            show_reticle: true,
            show_major_roads: false,
            show_minor_roads: false,
            show_water: false,
            show_beam: true,
            fill_elevation: false,
            show_bathymetry: true,
            show_contours: true,
            show_trees: false,
            show_buildings: false,
            show_admin: false,
            show_ships: false,
            show_flights: false,
            geojson_layers: Vec::new(),
            arcgis_sources: Vec::new(),
            arcgis_features: Vec::new(),
            selected_arcgis_feature: None,
            selected_root,
            factal_settings_open: false,
            factal_brief_open: false,
            factal_api_key: factal_api_key.clone(),
            windy_webcams_api_key: windy_webcams_api_key.clone(),
            ny511_api_key: ny511_api_key.clone(),
            aisstream_api_key: aisstream_api_key.clone(),
            settings_asset_root: settings_store::effective_asset_root()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            settings_data_root: app_settings.data_root.unwrap_or_default(),
            settings_derived_root: app_settings.derived_root.unwrap_or_default(),
            settings_srtm_root: app_settings.srtm_root.unwrap_or_default(),
            settings_planet_path: app_settings.planet_path.unwrap_or_default(),
            settings_gdal_bin_dir: app_settings.gdal_bin_dir.unwrap_or_default(),
            settings_osmium_bin_dir: app_settings.osmium_bin_dir.unwrap_or_default(),
            settings_prefer_overpass: app_settings.prefer_overpass,
            terrain_library_open: false,
            city_filter: String::new(),
            selected_city_ids: BTreeSet::new(),
            activity_log: {
                let mut lines = vec![
                    if factal_api_key.is_empty() {
                        "Factal stream is in demo mode until an API key is configured.".into()
                    } else {
                        "Factal API key loaded from local settings; live polling is ready.".into()
                    },
                    if windy_webcams_api_key.is_empty() && ny511_api_key.is_empty() {
                        "Camera registry is in demo mode until a live source key is configured."
                            .into()
                    } else {
                        "Camera registry keys loaded; live camera sync is ready.".into()
                    },
                ];
                lines.extend(terrain_inventory.status_lines());
                lines.extend(osm_inventory.status_lines());
                if let Ok(runtime_store) = &osm_runtime_store {
                    lines.push(format!(
                        "OSM runtime store ready: {}",
                        runtime_store.display()
                    ));
                }
                lines
            },
            replay_mode: false,
            replay_from_unix: crate::event_store::now_unix() - 30 * 86_400,
            replay_to_unix: crate::event_store::now_unix(),
            replay_from_str: crate::event_store::unix_to_date_str(
                crate::event_store::now_unix() - 30 * 86_400,
            ),
            replay_to_str: crate::event_store::unix_to_date_str(crate::event_store::now_unix()),
            replay_duration_secs: 120,
            replay_state: None,
            replay_history_status: String::new(),
            log_collapsed: false,
            factal_stream_status: if factal_api_key.is_empty() {
                "demo".into()
            } else {
                "configured".into()
            },
            camera_registry_status: if windy_webcams_api_key.is_empty() && ny511_api_key.is_empty()
            {
                "demo".into()
            } else {
                "configured".into()
            },
            terrain_inventory,
            osm_inventory,
        };

        if let Some(camera) = model.nearby_cameras(250.0).first() {
            model.selected_camera_id = Some(camera.id.clone());
        }

        model
    }

    pub fn has_factal_api_key(&self) -> bool {
        !self.factal_api_key.trim().is_empty()
    }

    pub fn has_camera_source_keys(&self) -> bool {
        !self.windy_webcams_api_key.trim().is_empty() || !self.ny511_api_key.trim().is_empty()
    }

    pub fn set_selected_root(&mut self, root: PathBuf) {
        self.selected_root = Some(root.clone());
        self.settings_asset_root = root.display().to_string();
        let _ = self.save_settings();
        let _ = settings_store::ensure_default_asset_layout();
        self.terrain_inventory = TerrainInventory::detect_from(Some(root.as_path()));
        let osm_runtime_store = osm_ingest::ensure_runtime_store(Some(root.as_path()));
        self.osm_inventory = OsmInventory::detect_from(Some(root.as_path()));
        self.push_log(format!("Asset root selected: {}", root.display()));
        if let Some(srtm_root) = terrain_assets::find_srtm_root(Some(root.as_path())) {
            self.push_log(format!("Detected SRTM root: {}", srtm_root.display()));
        }
        self.push_log(format!(
            "Terrain refresh: {}",
            self.terrain_inventory.status_summary()
        ));
        if let Some(planet) = &self.osm_inventory.planet_path {
            self.push_log(format!("Detected OSM planet source: {}", planet.display()));
        }
        if let Ok(runtime_store) = osm_runtime_store {
            self.push_log(format!(
                "OSM runtime store ready: {}",
                runtime_store.display()
            ));
        }
        self.push_log(format!(
            "OSM refresh: {}",
            self.osm_inventory.status_summary()
        ));
    }

    pub fn save_settings(&mut self) -> std::io::Result<()> {
        let settings = settings_store::AppSettings {
            factal_api_key: self.factal_api_key.trim().to_owned(),
            windy_webcams_api_key: self.windy_webcams_api_key.trim().to_owned(),
            ny511_api_key: self.ny511_api_key.trim().to_owned(),
            aisstream_api_key: self.aisstream_api_key.trim().to_owned(),
            asset_root: optional_path_field(&self.settings_asset_root),
            data_root: optional_path_field(&self.settings_data_root),
            derived_root: optional_path_field(&self.settings_derived_root),
            srtm_root: optional_path_field(&self.settings_srtm_root),
            planet_path: optional_path_field(&self.settings_planet_path),
            gdal_bin_dir: optional_path_field(&self.settings_gdal_bin_dir),
            osmium_bin_dir: optional_path_field(&self.settings_osmium_bin_dir),
            prefer_overpass: self.settings_prefer_overpass,
        };
        settings_store::save_app_settings(&settings)
    }

    pub fn apply_saved_settings(&mut self) {
        let _ = settings_store::ensure_default_asset_layout();
        let settings = settings_store::load_app_settings();
        self.selected_root = settings_store::effective_asset_root();
        self.settings_asset_root = self
            .selected_root
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        self.settings_data_root = settings.data_root.unwrap_or_default();
        self.settings_derived_root = settings.derived_root.unwrap_or_default();
        self.settings_srtm_root = settings.srtm_root.unwrap_or_default();
        self.settings_planet_path = settings.planet_path.unwrap_or_default();
        self.settings_gdal_bin_dir = settings.gdal_bin_dir.unwrap_or_default();
        self.settings_osmium_bin_dir = settings.osmium_bin_dir.unwrap_or_default();
        self.settings_prefer_overpass = settings.prefer_overpass;
        self.windy_webcams_api_key = settings.windy_webcams_api_key.trim().to_owned();
        self.ny511_api_key = settings.ny511_api_key.trim().to_owned();
        self.aisstream_api_key = settings.aisstream_api_key.trim().to_owned();

        self.terrain_inventory = TerrainInventory::detect_from(self.selected_root.as_deref());
        let osm_runtime_store = osm_ingest::ensure_runtime_store(self.selected_root.as_deref());
        self.osm_inventory = OsmInventory::detect_from(self.selected_root.as_deref());
        self.camera_registry_status = if self.has_camera_source_keys() {
            "configured".into()
        } else {
            "demo".into()
        };

        if let Some(root) = self.selected_root.clone() {
            self.push_log(format!(
                "Settings applied with asset root: {}",
                root.display()
            ));
            if let Some(srtm_root) = terrain_assets::find_srtm_root(Some(root.as_path())) {
                self.push_log(format!("Detected SRTM root: {}", srtm_root.display()));
            }
        }
        if let Some(planet) = &self.osm_inventory.planet_path {
            self.push_log(format!("Detected OSM planet source: {}", planet.display()));
        }
        if let Ok(runtime_store) = osm_runtime_store {
            self.push_log(format!(
                "OSM runtime store ready: {}",
                runtime_store.display()
            ));
        }
        self.push_log(format!(
            "Terrain refresh: {}",
            self.terrain_inventory.status_summary()
        ));
        self.push_log(format!(
            "OSM refresh: {}",
            self.osm_inventory.status_summary()
        ));
    }

    pub fn selected_event(&self) -> Option<&EventRecord> {
        let selected_id = self.selected_event_id.as_deref()?;
        self.events.iter().find(|event| event.id == selected_id)
    }

    pub fn selected_event_has_factal_brief(&self) -> bool {
        self.selected_event()
            .and_then(|event| event.factal_brief.as_ref())
            .is_some()
    }

    pub fn focused_city(&self) -> Option<city_catalog::CityEntry> {
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
            city.location_label()
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
        self.push_log(format!("City focus selected: {}", city.location_label()));
    }

    pub fn clear_city_focus(&mut self) {
        if self.focused_city_id.take().is_some() {
            self.push_log("City focus cleared; returning to event-driven focus.".into());
            if let Some(event) = self.selected_event() {
                self.globe_view.focus_on(event.location);
            }
        }
    }

    pub fn replace_factal_events(&mut self, events: Vec<EventRecord>) {
        let previous_selected = self.selected_event_id.clone();
        self.events = events;

        if self.events.is_empty() {
            self.selected_event_id = None;
            self.selected_camera_id = None;
            return;
        }

        let retained_selection = previous_selected
            .as_deref()
            .filter(|selected_id| self.events.iter().any(|event| event.id == *selected_id))
            .map(str::to_owned);

        self.selected_event_id =
            retained_selection.or_else(|| self.events.first().map(|event| event.id.clone()));
        self.selected_camera_id = self
            .nearby_cameras(250.0)
            .first()
            .map(|camera| camera.id.clone());
    }

    pub fn replace_camera_registry(&mut self, cameras: Vec<CameraFeed>, source_label: &str) {
        let previous_selected = self.selected_camera_id.clone();
        self.cameras = cameras;

        if self.cameras.is_empty() {
            self.selected_camera_id = None;
            self.camera_registry_status = "empty".into();
            self.push_log(format!(
                "Camera registry sync from {source_label} returned no cameras."
            ));
            return;
        }

        let retained_selection = previous_selected
            .as_deref()
            .filter(|selected_id| self.cameras.iter().any(|camera| camera.id == *selected_id))
            .map(str::to_owned);

        self.selected_camera_id = retained_selection.or_else(|| {
            self.nearby_cameras(250.0)
                .first()
                .map(|camera| camera.id.clone())
                .or_else(|| self.cameras.first().map(|camera| camera.id.clone()))
        });
        self.camera_registry_status = "live".into();
        self.push_log(format!(
            "Camera registry sync loaded {} camera(s) from {source_label}.",
            self.cameras.len()
        ));
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

    /// Enter or exit replay mode.  On enter: loads history from the local
    /// store for the configured window and starts playback.  If the store is
    /// empty and an API key is set, triggers a background history fetch.
    pub fn toggle_replay(&mut self) {
        if self.replay_mode {
            // Exit replay
            self.replay_mode = false;
            self.replay_state = None;
            return;
        }
        // Enter replay
        self.replay_mode = true;
        // Fetch history only when we actually have a coverage gap.
        // We consider coverage adequate when:
        //   (a) oldest stored event ≤ replay_from_unix  (data goes back far enough)
        //   (b) newest stored event ≥ now − 1 day       (data is fresh)
        if self.has_factal_api_key() && !crate::factal_stream::is_history_fetching() {
            let now = crate::event_store::now_unix();
            let oldest = crate::event_store::oldest_event_unix().unwrap_or(i64::MAX);
            let newest = crate::event_store::newest_event_unix().unwrap_or(0);
            if oldest > self.replay_from_unix || newest < now - 86_400 {
                crate::factal_stream::trigger_history_fetch(self.factal_api_key.clone(), 365);
                self.replay_history_status = "Fetching history…".into();
                self.push_log("Replay: fetching 1 year of Factal event history…".into());
            }
        }
        self.rebuild_replay_state();
    }

    /// Reload the replay state from the event store with current settings.
    pub fn rebuild_replay_state(&mut self) {
        let events =
            crate::event_store::load_events_in_range(self.replay_from_unix, self.replay_to_unix);
        let wall_duration = self.replay_duration_secs as f64;
        if events.is_empty() {
            self.replay_state = None;
        } else {
            self.replay_state = Some(ReplayState::new(
                events,
                self.replay_from_unix,
                self.replay_to_unix,
                wall_duration,
            ));
        }
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

fn optional_path_field(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}
