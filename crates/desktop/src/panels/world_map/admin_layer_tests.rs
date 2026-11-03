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

/// Exercise the normal camera detail selection and actual ready-to-draw segment batches.
#[test]
#[ignore = "read-only QA after prewarming Hilbert; set ONEKEE_ADMIN_REAL_CACHE"]
fn admin_lod_measures_globe_and_local_on_real_cache() {
    use super::super::{admin_globe, globe_scene, local_terrain_scene};
    use crate::model::GlobeViewState;
    let root = PathBuf::from(std::env::var_os("ONEKEE_ADMIN_REAL_CACHE").unwrap());
    let levels = [2, 4, 6, 8];
    let center = GeoPoint {
        lat: 52.52,
        lon: 13.4,
    };
    let view = GlobeViewState::from_focus(center);
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024., 768.));
    let layout = globe_scene::globe_layout(rect, &view);
    for shade in [false, true] {
        let globe = Request::globe(&layout, &view, rect, 2., shade);
        let start = std::time::Instant::now();
        let data = load_detail(std::slice::from_ref(&root), &levels, globe);
        assert!(data.missing.is_empty(), "{:?}", data.warnings);
        let warnings = data.warnings.len();
        let batch =
            admin_globe::prepare(Arc::new(data), crate::theme::MapTheme::Topo.admin_palette());
        assert_eq!(!batch.triangles.is_empty(), shade);
        assert!(!batch.lines.is_empty() && batch.lines.len() < 3_000_000);
        println!(
            "Globe 1024x768 at 2x, zoom {}: {globe:?}, {} submitted segments, {} fill triangles, {warnings} warnings, {:?} worker preparation",
            view.zoom,
            batch.lines.len(),
            batch.triangles.len(),
            start.elapsed()
        );
    }
    let half = local_terrain_scene::visual_half_extent_for_zoom(view.local_zoom);
    let layout = local_terrain_scene::LocalLayout {
        center: rect.center(),
        focus_center: rect.center() + egui::vec2(rect.width() * 0.02, 12.),
        width: rect.width() * 0.82,
        height: rect.height() * 0.74,
        horizontal_scale: rect.width() * 0.31,
    };
    let params = local_terrain_scene::projection::local_projection_params(
        &layout,
        &view,
        center,
        (half * 111.32 * center.lat.to_radians().cos()).max(1.),
        (half * 111.32).max(1.),
    );
    for shade in [false, true] {
        let local = Request::local(&params, rect, 2., shade);
        let start = std::time::Instant::now();
        let data = load_detail(std::slice::from_ref(&root), &levels, local);
        let edges: usize = data
            .regions
            .iter()
            .flat_map(|r| &r.rings)
            .map(|r| r.len() - 1)
            .sum();
        let tris: usize = data
            .regions
            .iter()
            .filter_map(|r| r.fill.as_ref())
            .map(|f| f.indices.len() / 3)
            .sum();
        assert!(data.missing.is_empty() && !data.regions.is_empty());
        assert!(
            data.regions
                .iter()
                .filter(|r| r.relation_id == 51477)
                .all(|r| r.rings.is_empty())
        );
        assert!(edges < 300_000 && (tris > 0) == shade);
        println!(
            "Local Berlin 1024x768 at 2x, zoom {}: {local:?}, {edges} candidate segments, {tris} fill triangles, {:?} cold worker preparation",
            view.local_zoom,
            start.elapsed()
        );
    }
}
