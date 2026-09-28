use super::*;
use crate::camera_directory_pipeline::{
    EyesOnPipelineConfig, EyesOnPipelineProgress, fetch_with_store,
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

struct Temp(std::path::PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "1kee-camera-cache-{}-{}-{}",
            std::process::id(),
            now(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn path(&self) -> std::path::PathBuf {
        self.0.join("endpoints.sqlite")
    }
    fn open(&self) -> Store {
        Store::open(&self.path()).unwrap()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn camera(id: &str, status: CameraConnectionState) -> CameraFeed {
    CameraFeed {
        id: format!("eyes-on-{id}"),
        label: format!("Camera {id}"),
        provider: "test".into(),
        kind: "snapshot".into(),
        location: GeoPoint { lat: 1.0, lon: 2.0 },
        stream_url: format!("http://8.8.8.8/{id}.jpg"),
        last_seen: "test".into(),
        status,
    }
}

#[test]
fn verified_endpoints_survive_restart_without_claiming_live_reachability() {
    let temp = Temp::new();
    let store = temp.open();
    let plan = store.plan("global").unwrap();
    assert_eq!(plan.action, Action::Discover);
    store
        .record(
            "global",
            plan.revision,
            &camera("failed", CameraConnectionState::Unreachable),
        )
        .unwrap();
    assert!(store.snapshot("global").unwrap().endpoints.is_empty());
    store
        .record(
            "global",
            plan.revision,
            &camera("working", CameraConnectionState::Reachable),
        )
        .unwrap();
    // Even a partially finished/cancelled first scan preserves its successes.
    drop(store);
    let store = temp.open();
    let plan = store.plan("global").unwrap();
    assert_eq!(plan.action, Action::Cached);
    assert_eq!(plan.snapshot.endpoints.len(), 1);
    assert_eq!(
        plan.snapshot.endpoints[0].camera.status,
        CameraConnectionState::Idle
    );
    assert!(
        plan.snapshot.endpoints[0]
            .camera
            .last_seen
            .contains("saved endpoint")
    );
}

#[test]
fn removing_last_endpoint_does_not_trigger_automatic_discovery() {
    let temp = Temp::new();
    let store = temp.open();
    let plan = store.plan("US").unwrap();
    let good = camera("1", CameraConnectionState::Reachable);
    store.record("US", plan.revision, &good).unwrap();
    assert_eq!(
        store.remove("US", Some(&good.stream_url)).unwrap(),
        vec![good.stream_url]
    );
    drop(store);
    let plan = temp.open().plan("US").unwrap();
    assert_eq!(plan.action, Action::Cached);
    assert!(plan.snapshot.endpoints.is_empty());
}

#[test]
fn removal_rejects_old_workers_and_preserves_other_scopes() {
    let temp = Temp::new();
    let worker = temp.open();
    let settings = temp.open();
    let old = worker.plan("global").unwrap();
    let us = worker.plan("US").unwrap();
    let good = camera("1", CameraConnectionState::Reachable);
    worker.record("global", old.revision, &good).unwrap();
    worker.record("US", us.revision, &good).unwrap();
    settings.remove("global", Some(&good.stream_url)).unwrap();
    assert!(!worker.record("global", old.revision, &good).unwrap());
    assert!(
        !worker
            .complete("global", old.revision, Action::Discover)
            .unwrap()
    );
    assert!(settings.snapshot("global").unwrap().endpoints.is_empty());
    assert_eq!(settings.snapshot("US").unwrap().endpoints.len(), 1);
}

#[test]
fn checks_and_failed_removal_leave_working_endpoints_intact() {
    let temp = Temp::new();
    let store = temp.open();
    let original = store.plan("global").unwrap();
    for id in ["1", "2"] {
        store
            .record(
                "global",
                original.revision,
                &camera(id, CameraConnectionState::Reachable),
            )
            .unwrap();
    }
    store
        .complete("global", original.revision, Action::Discover)
        .unwrap();
    store.request("global", Action::Check).unwrap();
    let check = store.plan("global").unwrap();
    assert_eq!(check.action, Action::Check);
    store
        .record(
            "global",
            check.revision,
            &camera("1", CameraConnectionState::Unreachable),
        )
        .unwrap();
    store
        .complete("global", check.revision, Action::Check)
        .unwrap();
    assert_eq!(store.snapshot("global").unwrap().endpoints.len(), 2);
    assert_eq!(
        store.remove("global", None).unwrap(),
        vec!["http://8.8.8.8/1.jpg"]
    );
    assert_eq!(
        store.snapshot("global").unwrap().endpoints[0].camera.id,
        "eyes-on-2"
    );
    assert_eq!(store.plan("global").unwrap().action, Action::Cached);
}

#[test]
fn explicit_refresh_survives_restart_without_deleting_the_last_good_cache() {
    let temp = Temp::new();
    let store = temp.open();
    let plan = store.plan("global").unwrap();
    store
        .record(
            "global",
            plan.revision,
            &camera("1", CameraConnectionState::Reachable),
        )
        .unwrap();
    store.request("global", Action::Discover).unwrap();
    assert!(
        !store
            .complete("global", plan.revision, Action::Discover)
            .unwrap()
    );
    drop(store);
    let store = temp.open();
    let refresh = store.plan("global").unwrap();
    assert_eq!(refresh.action, Action::Discover);
    assert_eq!(refresh.snapshot.endpoints.len(), 1);
    store
        .complete("global", refresh.revision, Action::Discover)
        .unwrap();
    assert_eq!(store.plan("global").unwrap().action, Action::Cached);
}

#[test]
fn saved_old_or_empty_scopes_return_without_any_network_requests() {
    let temp = Temp::new();
    let store = temp.open();
    let global = store.plan("global").unwrap();
    store
        .record(
            "global",
            global.revision,
            &camera("1", CameraConnectionState::Reachable),
        )
        .unwrap();
    store
        .connection
        .lock()
        .unwrap()
        .execute("UPDATE camera_endpoints SET verified_at=1,checked_at=1", [])
        .unwrap();
    let us = store.plan("US").unwrap();
    store.complete("US", us.revision, Action::Discover).unwrap();
    drop(store);
    let store = temp.open();
    // Any attempted HTTP request would fail through this unreachable proxy.
    let client = reqwest::blocking::Client::builder()
        .proxy(reqwest::Proxy::all("http://127.0.0.1:1").unwrap())
        .build()
        .unwrap();
    for (country, count) in [(None, 1), (Some("us".to_string()), 0)] {
        let progress = Mutex::new(Vec::new());
        let result = fetch_with_store(
            &client,
            &EyesOnPipelineConfig {
                country_code: country,
                requests_per_minute: 250,
            },
            &AtomicBool::new(false),
            |p| progress.lock().unwrap().push(p),
            &store,
        )
        .unwrap();
        assert_eq!(result.len(), count);
        assert_eq!(
            *progress.lock().unwrap(),
            vec![EyesOnPipelineProgress::SavedEndpoints { count }]
        );
    }
}

#[test]
fn unsafe_or_invalid_endpoints_are_not_persisted_or_loaded() {
    let temp = Temp::new();
    let store = temp.open();
    let plan = store.plan("global").unwrap();
    for url in [
        "http://127.0.0.1/",
        "http://10.0.0.1/",
        "http://user:secret@8.8.8.8/",
        "file:///tmp/image.jpg",
    ] {
        let mut bad = camera("bad", CameraConnectionState::Reachable);
        bad.stream_url = url.into();
        assert!(!store.record("global", plan.revision, &bad).unwrap());
    }
    let good = camera("1", CameraConnectionState::Reachable);
    store.record("global", plan.revision, &good).unwrap();
    store
        .connection
        .lock()
        .unwrap()
        .execute("UPDATE camera_endpoints SET lat=91", [])
        .unwrap();
    assert!(store.snapshot("global").unwrap().endpoints.is_empty());
}

#[test]
fn corrupt_cache_is_reported_without_overwriting_it() {
    let temp = Temp::new();
    let data = b"not a sqlite database";
    std::fs::write(temp.path(), data).unwrap();
    assert!(Store::open(&temp.path()).is_err());
    assert_eq!(std::fs::read(temp.path()).unwrap(), data);
}

#[test]
fn changed_feed_replaces_the_same_camera_instead_of_duplicating_it() {
    let temp = Temp::new();
    let store = temp.open();
    let plan = store.plan("global").unwrap();
    let mut good = camera("1", CameraConnectionState::Reachable);
    store.record("global", plan.revision, &good).unwrap();
    good.stream_url = "http://8.8.8.8/replacement.jpg".into();
    store.record("global", plan.revision, &good).unwrap();
    let snapshot = store.snapshot("global").unwrap();
    assert_eq!(snapshot.endpoints.len(), 1);
    assert_eq!(snapshot.endpoints[0].camera.stream_url, good.stream_url);
}

#[test]
fn discovery_reuse_recheck_removal_and_manual_rediscovery_use_only_expected_requests() {
    use std::io::{Read, Write};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let paths = Arc::new(Mutex::new(Vec::new()));
    let reachable = Arc::new(AtomicBool::new(true));
    let stop = Arc::new(AtomicBool::new(false));
    let server = {
        let paths = paths.clone();
        let reachable = reachable.clone();
        let stop = stop.clone();
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(15);
            while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
                let Ok((mut socket, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                socket
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 2048];
                while !request.windows(4).any(|p| p == b"\r\n\r\n") {
                    let count = socket.read(&mut buffer).unwrap();
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                }
                let request = String::from_utf8(request).unwrap();
                let path = request
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .to_owned();
                paths.lock().unwrap().push(path.clone());
                let (status, kind, body) = if path.contains("/byrating/") {
                    (
                        200,
                        "text/html",
                        r#"<a href="/en/view/42/"><img src="http://8.8.8.8/camera.jpg" title="Live camera Test in Town"></a><script>pagenavigator(1,1);</script>"#,
                    )
                } else if path.contains("/en/view/") {
                    (200, "text/html", "L.marker([1.0,2.0]);")
                } else {
                    (
                        if reachable.load(Ordering::Relaxed) {
                            200
                        } else {
                            503
                        },
                        "image/jpeg",
                        "",
                    )
                };
                write!(socket, "HTTP/1.1 {status} Test\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        })
    };
    let client = reqwest::blocking::Client::builder()
        .proxy(reqwest::Proxy::all(format!("http://{address}")).unwrap())
        .build()
        .unwrap();
    let temp = Temp::new();
    let store = temp.open();
    let config = EyesOnPipelineConfig {
        country_code: None,
        requests_per_minute: 3000,
    };
    let fetch =
        || fetch_with_store(&client, &config, &AtomicBool::new(false), |_| {}, &store).unwrap();
    assert_eq!(fetch().len(), 1);
    assert_eq!(paths.lock().unwrap().len(), 3); // listing, detail, feed
    assert_eq!(fetch().len(), 1);
    assert_eq!(paths.lock().unwrap().len(), 3); // no requests on normal poll
    reachable.store(false, Ordering::Relaxed);
    store.request("global", Action::Check).unwrap();
    assert_eq!(fetch()[0].status, CameraConnectionState::Unreachable);
    assert_eq!(paths.lock().unwrap().len(), 4); // feed only, no directory
    assert_eq!(store.remove("global", None).unwrap().len(), 1);
    assert!(fetch().is_empty());
    assert_eq!(paths.lock().unwrap().len(), 4);
    reachable.store(true, Ordering::Relaxed);
    store.request("global", Action::Discover).unwrap();
    assert_eq!(fetch().len(), 1);
    assert_eq!(paths.lock().unwrap().len(), 7); // full explicit rediscovery
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
}
