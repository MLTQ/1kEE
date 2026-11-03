use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new(size: [usize; 2], values: &[f32]) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "1kee-gravity-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let folder = root.join("gravity/ncei/test");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("manifest.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema": 1, "id": "test", "units": "mGal", "size": size,
                "bounds": [-188.0, 49.0, -126.0, 72.0], "nodata": -999999.0,
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            folder.join("grid.bil"),
            values
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn physical_values_nodata_rows_and_unwrapped_coverage_survive() {
    let fixture = Fixture::new([3, 2], &[-12.345, 0.0, -999999.0, 1.234, 5.678, f32::NAN]);
    let image = read(&fixture.0, "test", 4096, 150.0).unwrap();
    assert_eq!(image.image.size, [3, 2]);
    assert_eq!(image.bounds, [-188.0, 49.0, -126.0, 72.0]);
    assert_eq!(image.image.pixels[0], color(-12.345, 150.0));
    assert_eq!(image.image.pixels[1], color(0.0, 150.0));
    assert_eq!(image.image.pixels[2], egui::Color32::TRANSPARENT);
    assert_eq!(image.image.pixels[5], egui::Color32::TRANSPARENT);
    assert_ne!(color(0.0, 150.0), egui::Color32::TRANSPARENT);
    assert_eq!(color(-150.0, 150.0), color(-500.0, 150.0));
}

#[test]
fn native_alternating_pixels_survive_and_gpu_limit_is_respected() {
    let values: Vec<_> = (0..2050)
        .map(|x| if x % 2 == 0 { -10.0 } else { 10.0 })
        .collect();
    let fixture = Fixture::new([2050, 1], &values);
    let image = read(&fixture.0, "test", 4096, 150.0).unwrap();
    assert_eq!(image.image.size, [2050, 1]);
    assert!(image.image.pixels.windows(2).all(|p| p[0] != p[1]));
    assert_eq!(
        read(&fixture.0, "test", 512, 150.0).unwrap().image.size,
        [512, 1]
    );
}

#[test]
fn wrong_product_units_bounds_and_truncated_payload_are_rejected() {
    let fixture = Fixture::new([3, 2], &[0.0; 6]);
    let folder = fixture.0.join("gravity/ncei/test");
    let path = folder.join("manifest.json");
    let original: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for (key, bad) in [
        ("id", serde_json::json!("other")),
        ("units", serde_json::json!("Eotvos")),
        ("bounds", serde_json::json!([0, 0, 0, 10])),
    ] {
        let mut meta = original.clone();
        meta[key] = bad;
        std::fs::write(&path, serde_json::to_vec(&meta).unwrap()).unwrap();
        assert!(read(&fixture.0, "test", 4096, 150.0).is_err());
    }
    std::fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
    std::fs::write(folder.join("grid.bil"), [0; 4]).unwrap();
    assert!(read(&fixture.0, "test", 4096, 150.0).is_err());
}

#[test]
#[ignore = "requires the prepared Hilbert NCEI collection"]
fn real_hilbert_maps_load_at_native_detail() {
    let root = std::path::Path::new("/Volumes/Hilbert/Derived");
    for map in crate::model::gravity::gravity_maps() {
        let image = read(root, &map.id, 4096, 150.0).unwrap();
        assert!(
            image
                .image
                .pixels
                .iter()
                .any(|p| *p != egui::Color32::TRANSPARENT)
        );
        assert!(image.image.size.iter().all(|edge| *edge <= 4096));
        eprintln!("{}: {:?} {:?}", map.id, image.image.size, image.bounds);
    }
}
