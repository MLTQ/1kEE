use super::*;
use cell_format::write::write_cell;
pub(super) struct Scratch(pub(super) PathBuf);
impl Scratch {
    pub(super) fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "1kee-admin-load-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
pub(super) fn square(id: i64, level: u8, low: f32, high: f32) -> CellFeature {
    CellFeature {
        way_id: id,
        class: level,
        name: None,
        is_polygon: true,
        elevations: None,
        points: [
            (low, low),
            (high, low),
            (high, high),
            (low, high),
            (low, low),
        ]
        .map(|(lon, lat)| CellPoint { lon, lat })
        .to_vec(),
    }
}
#[test]
fn admin_loads_derived_cache_filters_levels_and_preserves_holes() {
    let scratch = Scratch::new();
    let root = scratch.0.join("Derived/osm");
    fs::create_dir_all(root.join("admin_cells")).unwrap();
    fs::write(
        root.join("admin_cells/admin_level_4.1kc"),
        write_cell(
            0,
            0,
            &[(TAG_ADMN, &[square(10, 4, 0., 4.), square(10, 4, 1., 2.)])],
        ),
    )
    .unwrap();
    let dirs = super::super::cell_loader::osm_cache_dir_candidates(&scratch.0);
    let data = load_from_dirs(&dirs, &[4]);
    assert!(data.missing.is_empty());
    assert_eq!(data.regions.len(), 1);
    assert_eq!(data.regions[0].rings.len(), 2);
    assert!(data.regions[0].fill.is_some());
    let data = load_from_dirs(&dirs, &[2]);
    assert!(data.regions.is_empty());
    assert_eq!(data.missing, [2]);
}
#[test]
fn admin_corrupt_binary_falls_back_and_legacy_lines_remain_outlines() {
    let scratch = Scratch::new();
    let dir = scratch.0.join("admin_cells");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(admin_filename(2)), b"broken").unwrap();
    fs::write(dir.join("admin_level_2.geojson"),r#"{"type":"FeatureCollection","features":[{"properties":{"relation_id":1},"geometry":{"type":"LineString","coordinates":[[0,0],[2,0],[2,2],[0,0]]}}]}"#).unwrap();
    let data = load_from_dirs(std::slice::from_ref(&scratch.0), &[2]);
    assert_eq!(data.regions.len(), 1);
    assert!(data.regions[0].fill.is_none());
    assert!(!data.warnings.is_empty());
}
#[test]
fn admin_geojson_polygon_supports_holes_and_rejects_bad_coordinates() {
    let bytes=br#"{"features":[{"properties":{"relation_id":3},"geometry":{"type":"Polygon","coordinates":[[[0,0],[4,0],[4,4],[0,0]],[[1,1],[2,1],[2,2],[1,1]]]}}]}"#;
    let features = parse_geojson(bytes, 3).unwrap();
    assert_eq!(features.len(), 2);
    assert!(
        features
            .iter()
            .all(|f| f.is_polygon && f.class == 3 && f.way_id == 3)
    );
    assert!(
        parse_geojson(
            br#"{"features":[{"geometry":{"type":"LineString","coordinates":[[null,2]]}}]}"#,
            2
        )
        .is_err()
    );
}
