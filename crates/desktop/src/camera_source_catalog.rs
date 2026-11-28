use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PublicCameraSourceKind {
    JsonArray,
    GeoJson,
    ArcGisFeatureService,
    TflJamCam,
    StaticList,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StaticCameraEntry {
    pub id: String,
    pub label: String,
    pub latitude: f32,
    pub longitude: f32,
    pub stream_url: String,
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PublicCameraSource {
    pub name: String,
    pub provider: String,
    pub kind: PublicCameraSourceKind,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub array_field: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub feature_field: Option<String>,
    #[serde(default)]
    pub id_field: Option<String>,
    #[serde(default)]
    pub label_field: Option<String>,
    #[serde(default)]
    pub stream_url_field: Option<String>,
    #[serde(default)]
    pub stream_url_template: Option<String>,
    #[serde(default)]
    pub kind_value: Option<String>,
    #[serde(default)]
    pub latitude_field: Option<String>,
    #[serde(default)]
    pub longitude_field: Option<String>,
    #[serde(default)]
    pub geometry_x_field: Option<String>,
    #[serde(default)]
    pub geometry_y_field: Option<String>,
    #[serde(default)]
    pub static_cameras: Vec<StaticCameraEntry>,
    #[serde(default)]
    pub enabled: bool,
}

/// How long a parsed catalog is reused before re-reading from disk. The catalog
/// is a config file edited by hand, so a couple seconds of staleness is fine and
/// it spares us a disk read + JSON parse on every UI frame (this is called
/// several times per frame from `camera_registry::tick`).
const CATALOG_TTL: Duration = Duration::from_secs(2);

struct Cached {
    root_key: Option<PathBuf>,
    include_builtin: bool,
    loaded_at: Instant,
    sources: Vec<PublicCameraSource>,
}

fn cache() -> &'static Mutex<Option<Cached>> {
    static C: OnceLock<Mutex<Option<Cached>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

pub fn load_public_sources(selected_root: Option<&Path>) -> Vec<PublicCameraSource> {
    load_public_sources_with_builtin(selected_root, false)
}

pub fn load_public_sources_with_builtin(
    selected_root: Option<&Path>,
    include_builtin: bool,
) -> Vec<PublicCameraSource> {
    let root_key = selected_root.map(|p| p.to_path_buf());
    {
        let guard = cache().lock().unwrap();
        if let Some(c) = guard.as_ref() {
            if c.root_key == root_key
                && c.include_builtin == include_builtin
                && c.loaded_at.elapsed() < CATALOG_TTL
            {
                return c.sources.clone();
            }
        }
    }
    let sources = load_public_sources_uncached(selected_root, include_builtin);
    let mut guard = cache().lock().unwrap();
    *guard = Some(Cached {
        root_key,
        include_builtin,
        loaded_at: Instant::now(),
        sources: sources.clone(),
    });
    sources
}

fn load_public_sources_uncached(
    selected_root: Option<&Path>,
    include_builtin: bool,
) -> Vec<PublicCameraSource> {
    let mut disk_sources: Vec<PublicCameraSource> = source_catalog_paths(selected_root)
        .into_iter()
        .find_map(|path| {
            fs::read_to_string(&path)
                .ok()
                .and_then(|body| serde_json::from_str::<Vec<PublicCameraSource>>(&body).ok())
        })
        .unwrap_or_default();

    if disk_sources.is_empty() && include_builtin {
        disk_sources = builtin_public_sources();
    } else if include_builtin {
        // Merge any built-ins not explicitly listed (by name) in the user's JSON catalog.
        let existing: std::collections::HashSet<String> = disk_sources
            .iter()
            .map(|s| s.name.to_ascii_lowercase())
            .collect();
        for builtin in builtin_public_sources() {
            if !existing.contains(&builtin.name.to_ascii_lowercase()) {
                disk_sources.push(builtin);
            }
        }
    }

    disk_sources
        .into_iter()
        .filter(|source| source.enabled)
        .collect()
}

/// Curated allowlisted open public government, weather, transport & volcano webcam networks.
/// Every source is openly published for public consumption without credentials or scanning.
pub fn builtin_public_sources() -> Vec<PublicCameraSource> {
    vec![
        PublicCameraSource {
            name: "Caltrans D4 SF Bay Area CCTV".into(),
            provider: "Caltrans D4".into(),
            kind: PublicCameraSourceKind::JsonArray,
            endpoint: "https://cwwp2.dot.ca.gov/data/d4/cctv/cctvStatusD04.json".into(),
            description: Some("California DOT District 4 public highway cameras".into()),
            license: Some("Public Domain (CA Gov)".into()),
            homepage: Some("https://cwwp2.dot.ca.gov".into()),
            array_field: Some("data".into()),
            feature_field: None,
            id_field: Some("cctv.index".into()),
            label_field: Some("cctv.location.locationName".into()),
            stream_url_field: Some("cctv.imageData.static.currentImageURL".into()),
            stream_url_template: None,
            kind_value: Some("snapshot".into()),
            latitude_field: Some("cctv.location.latitude".into()),
            longitude_field: Some("cctv.location.longitude".into()),
            geometry_x_field: None,
            geometry_y_field: None,
            static_cameras: Vec::new(),
            enabled: true,
        },
        PublicCameraSource {
            name: "Caltrans D7 Los Angeles CCTV".into(),
            provider: "Caltrans D7".into(),
            kind: PublicCameraSourceKind::JsonArray,
            endpoint: "https://cwwp2.dot.ca.gov/data/d7/cctv/cctvStatusD07.json".into(),
            description: Some("California DOT District 7 public highway cameras".into()),
            license: Some("Public Domain (CA Gov)".into()),
            homepage: Some("https://cwwp2.dot.ca.gov".into()),
            array_field: Some("data".into()),
            feature_field: None,
            id_field: Some("cctv.index".into()),
            label_field: Some("cctv.location.locationName".into()),
            stream_url_field: Some("cctv.imageData.static.currentImageURL".into()),
            stream_url_template: None,
            kind_value: Some("snapshot".into()),
            latitude_field: Some("cctv.location.latitude".into()),
            longitude_field: Some("cctv.location.longitude".into()),
            geometry_x_field: None,
            geometry_y_field: None,
            static_cameras: Vec::new(),
            enabled: true,
        },
        PublicCameraSource {
            name: "Caltrans D11 San Diego CCTV".into(),
            provider: "Caltrans D11".into(),
            kind: PublicCameraSourceKind::JsonArray,
            endpoint: "https://cwwp2.dot.ca.gov/data/d11/cctv/cctvStatusD11.json".into(),
            description: Some("California DOT District 11 public highway cameras".into()),
            license: Some("Public Domain (CA Gov)".into()),
            homepage: Some("https://cwwp2.dot.ca.gov".into()),
            array_field: Some("data".into()),
            feature_field: None,
            id_field: Some("cctv.index".into()),
            label_field: Some("cctv.location.locationName".into()),
            stream_url_field: Some("cctv.imageData.static.currentImageURL".into()),
            stream_url_template: None,
            kind_value: Some("snapshot".into()),
            latitude_field: Some("cctv.location.latitude".into()),
            longitude_field: Some("cctv.location.longitude".into()),
            geometry_x_field: None,
            geometry_y_field: None,
            static_cameras: Vec::new(),
            enabled: true,
        },
        PublicCameraSource {
            name: "Caltrans D3 Sacramento & Sierra CCTV".into(),
            provider: "Caltrans D3".into(),
            kind: PublicCameraSourceKind::JsonArray,
            endpoint: "https://cwwp2.dot.ca.gov/data/d3/cctv/cctvStatusD03.json".into(),
            description: Some("California DOT District 3 public highway cameras".into()),
            license: Some("Public Domain (CA Gov)".into()),
            homepage: Some("https://cwwp2.dot.ca.gov".into()),
            array_field: Some("data".into()),
            feature_field: None,
            id_field: Some("cctv.index".into()),
            label_field: Some("cctv.location.locationName".into()),
            stream_url_field: Some("cctv.imageData.static.currentImageURL".into()),
            stream_url_template: None,
            kind_value: Some("snapshot".into()),
            latitude_field: Some("cctv.location.latitude".into()),
            longitude_field: Some("cctv.location.longitude".into()),
            geometry_x_field: None,
            geometry_y_field: None,
            static_cameras: Vec::new(),
            enabled: true,
        },
        PublicCameraSource {
            name: "Digitraffic Finland Weathercams".into(),
            provider: "Digitraffic Finland".into(),
            kind: PublicCameraSourceKind::GeoJson,
            endpoint: "https://tie.digitraffic.fi/api/weathercam/v1/stations".into(),
            description: Some("Fintraffic national road weather camera network".into()),
            license: Some("CC BY 4.0".into()),
            homepage: Some("https://www.digitraffic.fi/en/road-traffic/".into()),
            array_field: None,
            feature_field: None,
            id_field: Some("id".into()),
            label_field: Some("name".into()),
            stream_url_field: None,
            stream_url_template: Some("https://weathercam.digitraffic.fi/{id}01.jpg".into()),
            kind_value: Some("snapshot".into()),
            latitude_field: None,
            longitude_field: None,
            geometry_x_field: None,
            geometry_y_field: None,
            static_cameras: Vec::new(),
            enabled: true,
        },
        PublicCameraSource {
            name: "TfL London JamCams".into(),
            provider: "TfL London".into(),
            kind: PublicCameraSourceKind::TflJamCam,
            endpoint: "https://api.tfl.gov.uk/Place/Type/JamCam".into(),
            description: Some("Transport for London public JamCam traffic snapshots".into()),
            license: Some("Open Government Licence v2.0".into()),
            homepage: Some("https://api.tfl.gov.uk".into()),
            array_field: None,
            feature_field: None,
            id_field: Some("id".into()),
            label_field: Some("commonName".into()),
            stream_url_field: None,
            stream_url_template: None,
            kind_value: Some("snapshot".into()),
            latitude_field: Some("lat".into()),
            longitude_field: Some("lon".into()),
            geometry_x_field: None,
            geometry_y_field: None,
            static_cameras: Vec::new(),
            enabled: true,
        },
        PublicCameraSource {
            name: "USGS & GeoNet Volcano Observatories".into(),
            provider: "USGS / GeoNet Volcano".into(),
            kind: PublicCameraSourceKind::StaticList,
            endpoint: String::new(),
            description: Some("Public volcano observatory webcams (USGS HVO/CVO/AVO & GeoNet NZ)".into()),
            license: Some("US Gov Public Domain / CC BY 3.0 NZ".into()),
            homepage: Some("https://volcanoes.usgs.gov".into()),
            array_field: None,
            feature_field: None,
            id_field: None,
            label_field: None,
            stream_url_field: None,
            stream_url_template: None,
            kind_value: Some("snapshot".into()),
            latitude_field: None,
            longitude_field: None,
            geometry_x_field: None,
            geometry_y_field: None,
            static_cameras: vec![
                StaticCameraEntry {
                    id: "usgs-kilauea-kwcam".into(),
                    label: "Kilauea Halema'uma'u West Rim (KWcam)".into(),
                    latitude: 19.4069,
                    longitude: -155.2834,
                    stream_url: "https://volcanoes.usgs.gov/cams/KWcam/images/M.jpg".into(),
                    kind: Some("snapshot".into()),
                },
                StaticCameraEntry {
                    id: "usgs-kilauea-b1cam".into(),
                    label: "Kilauea Halema'uma'u East Rim (B1cam)".into(),
                    latitude: 19.4080,
                    longitude: -155.2680,
                    stream_url: "https://volcanoes.usgs.gov/cams/B1cam/images/M.jpg".into(),
                    kind: Some("snapshot".into()),
                },
                StaticCameraEntry {
                    id: "usgs-maunaloa-mlcam".into(),
                    label: "Mauna Loa Moku'aweoweo North Rim (MLcam)".into(),
                    latitude: 19.4795,
                    longitude: -155.5980,
                    stream_url: "https://volcanoes.usgs.gov/cams/MLcam/images/M.jpg".into(),
                    kind: Some("snapshot".into()),
                },
                StaticCameraEntry {
                    id: "usgs-msh-sugarbowl".into(),
                    label: "Mount St. Helens Sugar Bowl (SHcam)".into(),
                    latitude: 46.2004,
                    longitude: -122.1856,
                    stream_url: "https://volcanoes.usgs.gov/cams/SHcam/images/M.jpg".into(),
                    kind: Some("snapshot".into()),
                },
                StaticCameraEntry {
                    id: "usgs-yellowstone-oldfaithful".into(),
                    label: "Yellowstone Old Faithful Observatory".into(),
                    latitude: 44.4605,
                    longitude: -110.8281,
                    stream_url: "https://www.nps.gov/webcams-yell/oldfaithful.jpg".into(),
                    kind: Some("snapshot".into()),
                },
                StaticCameraEntry {
                    id: "geonet-ruapehu-north".into(),
                    label: "Mt. Ruapehu North (GeoNet NZ)".into(),
                    latitude: -39.2817,
                    longitude: 175.5685,
                    stream_url: "https://images.geonet.org.nz/volcano/cameras/latest/rimk.jpg".into(),
                    kind: Some("snapshot".into()),
                },
                StaticCameraEntry {
                    id: "geonet-whakaari".into(),
                    label: "Whakaari / White Island (GeoNet NZ)".into(),
                    latitude: -37.5210,
                    longitude: 177.1830,
                    stream_url: "https://images.geonet.org.nz/volcano/cameras/latest/wite.jpg".into(),
                    kind: Some("snapshot".into()),
                },
                StaticCameraEntry {
                    id: "geonet-tongariro".into(),
                    label: "Mt. Tongariro Te Maari (GeoNet NZ)".into(),
                    latitude: -39.1080,
                    longitude: 175.6430,
                    stream_url: "https://images.geonet.org.nz/volcano/cameras/latest/toma.jpg".into(),
                    kind: Some("snapshot".into()),
                },
                StaticCameraEntry {
                    id: "noaa-ndbc-46026".into(),
                    label: "NOAA BuoyCAM 46026 · San Francisco Approach".into(),
                    latitude: 37.759,
                    longitude: -122.833,
                    stream_url: "https://www.ndbc.noaa.gov/buoycam.php?station=46026".into(),
                    kind: Some("snapshot".into()),
                },
                StaticCameraEntry {
                    id: "noaa-ndbc-44025".into(),
                    label: "NOAA BuoyCAM 44025 · Long Island / NY Bight".into(),
                    latitude: 40.251,
                    longitude: -73.164,
                    stream_url: "https://www.ndbc.noaa.gov/buoycam.php?station=44025".into(),
                    kind: Some("snapshot".into()),
                },
            ],
            enabled: true,
        },
    ]
}

fn source_catalog_paths(selected_root: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(root) = selected_root {
        candidates.push(root.join("Data/camera_sources/public_sources.json"));
        candidates.push(root.join("Data/camera_sources/public_sources.jsonc"));
    }
    candidates.push(PathBuf::from("Data/camera_sources/public_sources.json"));
    candidates.push(PathBuf::from("Data/camera_sources/public_sources.jsonc"));
    candidates
}
