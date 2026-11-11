use super::*;
use reqwest::blocking::Client;
use serde_json::json;
use sha1::{Digest, Sha1};
use std::io::{BufRead, BufReader, Cursor, Write};
use std::net::TcpListener;

fn archive(label: &str) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("doc.kml", zip::write::SimpleFileOptions::default())
        .unwrap();
    write!(zip, r#"<kml xmlns="http://www.opengis.net/kml/2.2"><Document><Folder><name>Incidents</name>
        <Folder><name>Reports</name><Placemark><name>{label}</name><Point><coordinates>-75.5,41.2,0</coordinates></Point>
        </Placemark></Folder></Folder></Document></kml>"#).unwrap();
    zip.finish().unwrap().into_inner()
}

fn blob_sha(bytes: &[u8]) -> String {
    let mut hash = Sha1::new();
    write!(hash, "blob {}\0", bytes.len()).unwrap();
    hash.update(bytes);
    format!("{:x}", hash.finalize())
}

// Serve exactly one response and return the request for assertions. Cache-only
// calls below use an invalid URL so an accidental request fails the test.
fn serve(body: Vec<u8>, status: &str) -> (String, std::thread::JoinHandle<String>) {
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", socket.local_addr().unwrap());
    let status = status.to_owned();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = socket.accept().unwrap();
        let request = BufReader::new(&stream)
            .lines()
            .map(Result::unwrap)
            .take_while(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
            .to_lowercase();
        write!(stream, "HTTP/1.1 {status}\r\nETag: \"tree\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
        stream.write_all(&body).unwrap();
        request
    });
    (url, handle)
}

#[test]
fn daily_catalog_revalidation_survives_restart_and_keeps_last_complete_tree() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("cache.sqlite");
    let db = refresh::open(&file).unwrap();
    let client = Client::builder().no_proxy().build().unwrap();
    let tree = json!({"truncated": false, "tree": [
        {"type":"blob","path":LAST_30,"sha":"1".repeat(40)},
        {"type":"blob","path":"notes.pdf","sha":"2".repeat(40)}
    ]});
    let (url, server) = serve(tree.to_string().into_bytes(), "200 OK");
    let (catalog, warning) = refresh::catalog(&db, &client, &url).unwrap();
    assert!(warning.is_none());
    assert_eq!(catalog.files.len(), 1);
    assert!(
        server
            .join()
            .unwrap()
            .starts_with("get /trees/main?recursive=1 ")
    );
    drop(db);

    let db = refresh::open(&file).unwrap();
    let (catalog, warning) = refresh::catalog(&db, &client, "invalid-url").unwrap();
    assert!(warning.is_none() && catalog.remaining() > Duration::from_secs(86000));
    let expire = || {
        db.execute("UPDATE catalog SET json=json_set(json, '$.checked', 1)", [])
            .unwrap()
    };
    expire();
    let (url, server) = serve(Vec::new(), "304 Not Modified");
    let (confirmed, warning) = refresh::catalog(&db, &client, &url).unwrap();
    assert_eq!(confirmed.files, catalog.files);
    assert!(warning.is_none() && !confirmed.remaining().is_zero());
    assert!(server.join().unwrap().contains("if-none-match: \"tree\""));

    expire();
    let incomplete = json!({"truncated": true, "tree": []});
    let (url, server) = serve(incomplete.to_string().into_bytes(), "200 OK");
    let (stale, warning) = refresh::catalog(&db, &client, &url).unwrap();
    assert!(warning.unwrap().contains("incomplete"));
    assert_eq!(stale.files, confirmed.files);
    assert!(stale.remaining().is_zero());
    server.join().unwrap();
}

#[test]
fn only_changed_kmz_is_pulled_and_invalid_update_preserves_previous_layers() {
    let dir = tempfile::tempdir().unwrap();
    let db = refresh::open(&dir.path().join("layers.sqlite")).unwrap();
    let client = Client::builder().no_proxy().build().unwrap();
    let mut good_sha = String::new();
    for label in ["Initial report", "Updated report"] {
        let bytes = archive(label);
        good_sha = blob_sha(&bytes);
        let (url, server) = serve(bytes, "200 OK");
        let (product, warning) = refresh::product(&db, &client, &url, LAST_30, &good_sha).unwrap();
        let request = server.join().unwrap();
        assert!(request.starts_with(&format!("get /blobs/{good_sha} ")));
        assert!(request.contains("accept: application/vnd.github.raw+json"));
        assert!(warning.is_none());
        assert_eq!(product.layers[0].name, "Incidents / Reports");
        assert_eq!(product.layers[0].features[0].label.as_deref(), Some(label));
        let (_, warning) =
            refresh::product(&db, &client, "invalid-url", LAST_30, &good_sha).unwrap();
        assert!(warning.is_none());
    }
    for sha in ["f".repeat(40), blob_sha(b"broken zip")] {
        let (url, server) = serve(b"broken zip".to_vec(), "200 OK");
        let (previous, warning) = refresh::product(&db, &client, &url, LAST_30, &sha).unwrap();
        assert!(warning.is_some());
        assert_eq!(previous.sha, good_sha);
        server.join().unwrap();
    }
    let (_, warning) = refresh::product(&db, &client, "invalid-url", LAST_30, &good_sha).unwrap();
    assert!(warning.is_none());
}

#[test]
fn listener_respects_selection_visibility_and_upstream_removals() {
    let mut source = Source::new(Settings::default(), PathBuf::new());
    source.tick();
    assert!(source.worker.is_none());
    let sha = "2".repeat(40);
    let mut catalog = Catalog::default();
    catalog.files.insert(LAST_30.into(), sha.clone());
    catalog.files.insert("other.kmz".into(), "9".repeat(40));
    source.finish(Job::Catalog, Ok((Data::Catalog(catalog), None)));
    source.catalog_due = Instant::now() + refresh::DAY;
    source.settings.enabled = true;
    source.settings.hidden_folders.insert(
        LAST_30.into(),
        BTreeSet::from(["Incidents / Reports".into()]),
    );
    let job = source.next_job(Instant::now()).unwrap();
    assert!(matches!(&job, Job::Export { path, .. } if path == LAST_30));
    let layers = GeoJsonLayer::kmz_folder_layers(LAST_30, &archive("Report")).unwrap();
    source.finish(job, Ok((Data::Export(Product { sha, layers }), None)));
    assert!(!source.layers().next().unwrap()[0].visible);
    assert!(source.next_job(Instant::now()).is_none());
    source.finish(Job::Catalog, Ok((Data::Catalog(Catalog::default()), None)));
    assert!(source.layers().next().is_none());
}

#[test]
#[ignore = "checks the publisher's current catalog and default export over the network"]
fn live_default_export() {
    let dir = tempfile::tempdir().unwrap();
    let db = refresh::open(&dir.path().join("cache.sqlite")).unwrap();
    let client = refresh::client().unwrap();
    let (catalog, warning) = refresh::catalog(&db, &client, refresh::API).unwrap();
    assert!(warning.is_none());
    let (product, warning) =
        refresh::product(&db, &client, refresh::API, LAST_30, &catalog.files[LAST_30]).unwrap();
    assert!(warning.is_none());
    eprintln!(
        "{} published KMZs; default export has {} layers and {} features",
        catalog.files.len(),
        product.layers.len(),
        product
            .layers
            .iter()
            .map(|l| l.features.len())
            .sum::<usize>()
    );
}

#[test]
#[ignore = "exercises the production map scenes with isolated renderer state"]
fn cached_event_reaches_both_map_views() {
    use crate::model::{AppModel, GeoPoint};
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("layers.sqlite");
    let bytes = archive("GhostMaps test event");
    let sha = blob_sha(&bytes);
    let mut catalog = Catalog::default();
    catalog.checked = std::time::UNIX_EPOCH.elapsed().unwrap().as_secs();
    catalog.files.insert(LAST_30.into(), sha.clone());
    let db = refresh::open(&file).unwrap();
    db.execute(
        "INSERT INTO catalog VALUES (1, ?1)",
        [serde_json::to_string(&catalog).unwrap()],
    )
    .unwrap();
    db.execute(
        "INSERT INTO exports VALUES (?1, ?2, ?3)",
        rusqlite::params![LAST_30, sha, bytes],
    )
    .unwrap();
    drop(db);
    let mut model = AppModel::new();
    model.ghostmaps = Source::new(
        Settings {
            enabled: true,
            labels: true,
            ..Default::default()
        },
        file,
    );
    let start = Instant::now();
    while model.ghostmaps.layers().next().is_none() {
        model.ghostmaps.tick();
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "fresh cache did not load"
        );
        std::thread::sleep(Duration::from_millis(5));
    }

    model.globe_view.focus_on(GeoPoint {
        lat: 41.2,
        lon: -75.5,
    });
    model.globe_view.stop_motion();
    model.obs_view = true;
    model.selected_root = Some(dir.path().into());
    model.show_contours = false;
    model.show_coastlines = false;
    model.show_bathymetry = false;
    let ctx = egui::Context::default();
    for local in [false, true] {
        model.globe_view.local_mode = local;
        for enabled in [true, false] {
            model.ghostmaps.settings.enabled = enabled;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 700.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default()
                        .show(ctx, |ui| crate::panels::render_world_map(ui, &mut model));
                },
            );
            assert_eq!(
                enabled,
                output.shapes.iter().any(|s| matches!(&s.shape,
                egui::Shape::Text(t) if t.galley.text() == "GhostMaps test event")),
                "local={local}"
            );
        }
    }
}
