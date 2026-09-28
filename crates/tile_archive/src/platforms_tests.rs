use super::*;

fn fixture() -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({"version":1,"manifest":{"version":1,"sources":[]},"platforms":[{
        "source":"bsee","source_id":"1","name":"Structure A","operator":"","country":"US",
        "product":"","kind":"Platform","function":"","status":"unknown","installed":"","removed":"",
        "water_depth_m":null,"historical":false,"planned":false,"support":false,"lat":28.0,"lon":-90.0
    }]})).unwrap()
}

#[test]
fn rejects_corrupt_positions_duplicates_and_versions() {
    for (field, value) in [
        ("lat", serde_json::json!(91)),
        ("lon", serde_json::json!(-181)),
        ("source_id", serde_json::json!("")),
    ] {
        let mut v: serde_json::Value = serde_json::from_slice(&fixture()).unwrap();
        v["platforms"][0][field] = value;
        assert!(decode(&serde_json::to_vec(&v).unwrap()).is_err());
    }
    let mut v: serde_json::Value = serde_json::from_slice(&fixture()).unwrap();
    let duplicate = v["platforms"][0].clone();
    v["platforms"].as_array_mut().unwrap().push(duplicate);
    assert!(decode(&serde_json::to_vec(&v).unwrap()).is_err());
    v["version"] = serde_json::json!(2);
    assert!(decode(&serde_json::to_vec(&v).unwrap()).is_err());
}

#[test]
fn archive_roundtrip_world_copy_and_filters() {
    let dir = std::env::temp_dir().join(format!(
        "1kee-platform-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&dir).unwrap();
    let input = dir.join("input.json");
    fs::write(&input, fixture()).unwrap();
    let archive = dir.join(FILE_NAME);
    assert_eq!(build(&input, &archive).unwrap(), 1);
    assert!(build(&input, &archive).is_err());
    let reader = Reader::open(&archive).unwrap();
    let mut p = load(&reader).unwrap().platforms.remove(0);
    assert!(Filter::default().accepts(&p));
    p.historical = true;
    assert!(!Filter::default().accepts(&p));
    assert!(
        Filter {
            historical: true,
            ..Filter::default()
        }
        .accepts(&p)
    );
    p.support = true;
    assert!(
        !Filter {
            historical: true,
            ..Filter::default()
        }
        .accepts(&p)
    );
    let world = dir.join("world.1ka");
    let mut writer = Writer::create(&world).unwrap();
    writer
        .put_batch(
            &[(
                crate::pipelines::key(1, 0, 0),
                b"preserve other namespace".to_vec(),
            )],
            None,
        )
        .unwrap();
    copy_into(&reader, &mut writer).unwrap();
    writer.finish().unwrap();
    let world_reader = Reader::open(&world).unwrap();
    assert_eq!(load(&world_reader).unwrap().platforms.len(), 1);
    assert_eq!(reader.get(KEY).unwrap(), world_reader.get(KEY).unwrap());
    assert_eq!(
        world_reader
            .get(crate::pipelines::key(1, 0, 0))
            .unwrap()
            .unwrap(),
        b"preserve other namespace"
    );
    assert!(
        !fs::read_dir(&dir).unwrap().any(|p| p
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".part"))
    );
    drop(world_reader);
    drop(reader);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
#[ignore = "requires installed real platform archive"]
fn real_platform_inventory() {
    let path = std::env::var_os("ONEKEE_PLATFORM_ARCHIVE").expect("archive path");
    let start = std::time::Instant::now();
    let inventory = load(&Reader::open(Path::new(&path)).unwrap()).unwrap();
    let mut sources = std::collections::BTreeMap::new();
    for p in &inventory.platforms {
        *sources.entry(&p.source).or_insert(0) += 1;
    }
    let visible = inventory
        .platforms
        .iter()
        .filter(|p| Filter::default().accepts(p))
        .count();
    eprintln!(
        "Loaded/validated {} platforms, {visible} default, {sources:?}, in {:?}",
        inventory.platforms.len(),
        start.elapsed()
    );
    for (source, count) in sources {
        assert_eq!(
            inventory.manifest["counts"][source].as_u64().unwrap(),
            count
        );
    }
    assert_eq!(
        inventory.manifest["visible_by_default"].as_u64().unwrap(),
        visible as u64
    );
}
