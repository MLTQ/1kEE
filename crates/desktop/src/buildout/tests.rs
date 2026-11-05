use super::*;
use reqwest::blocking::Client;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

const CAMPUS: &str = r#"[{"id":"camp","name":"O'Brien campus","operator":"op","city":"City","country":"US","lat":21,"lng":-80,"precision":"city","status":"planned","confidence":"low","lastVerified":"2026.11.05","mw":{},"sources":[]}]"#;
const RULE: &str = r#"[{"id":"r","title":"Local rule","summary":"Land use","country":"US","level":"state","kind":"study","severity":1,"status":"proposed","date":"2026.11","sources":[],"lastVerified":"2026.11.05"}]"#;
const NEWS: &str = r#"[{id:"n",date:"2026.11.05",title:"Update",summary:"Brackets [{ and quotes \" inside text",weight:"day",sites:["camp"],rules:["r"],sources:[]}]"#;

fn bundle() -> String {
    format!(
        "a.exports=JSON.parse('{}');b.exports=JSON.parse('{RULE}');c.exports={NEWS}",
        CAMPUS.replace('\'', "\\'")
    )
}

#[test]
fn extracts_three_tables_without_evaluating_script() {
    let mut catalog = catalog::Catalog::default();
    assert!(catalog.ingest(&bundle()) && catalog.valid());
    let c = &catalog.campuses[0];
    assert_eq!(c.name, "O'Brien campus");
    assert_eq!(c.precision, "city");
    assert!(c.mw.operational.is_none());
    assert!(catalog.rules[0].lat.is_none());
    assert_eq!(catalog.news[0].sites[0], c.id);
    assert_eq!(catalog.news[0].rules[0], catalog.rules[0].id);
    for broken in [
        bundle().replace("b.exports", "b.other"),
        bundle().replace("\"lat\":21", "\"lat\":91"),
        bundle().replace("title:\"Update\"", "title:getTitle()"),
    ] {
        let mut incomplete = catalog::Catalog::default();
        incomplete.ingest(&broken);
        assert!(!incomplete.valid());
    }
}

#[test]
fn discovers_only_local_static_assets() {
    let page = r#"<script src="/_next/static/chunks/abc.js"></script><script src="//elsewhere.test/x.js"></script><script src="/_next/static/../bad.js"></script>"#;
    assert_eq!(catalog::assets(page), ["/_next/static/chunks/abc.js"]);
}

#[test]
fn server_round_trip_cache_and_revalidation() {
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", socket.local_addr().unwrap());
    let page = format!("{origin}/buildout");
    let asset = "/_next/static/immutable/chunks/changed-hash.js";
    let next_asset = "/_next/static/immutable/chunks/new-hash.js";
    let server = std::thread::spawn(move || {
        for (expected, status, tag, body) in [
            (
                "/buildout",
                "200 OK",
                "first",
                format!("<script src=\"{asset}\"></script>"),
            ),
            (asset, "200 OK", "asset", bundle()),
            ("/buildout", "304 Not Modified", "first", String::new()),
            (
                "/buildout",
                "200 OK",
                "second",
                format!("<html><script src=\"{asset}\"></script>"),
            ),
            (
                "/buildout",
                "200 OK",
                "third",
                format!("<script src=\"{next_asset}\"></script>"),
            ),
            (
                next_asset,
                "200 OK",
                "new-asset",
                bundle().replace("planned", "operational"),
            ),
        ] {
            let (mut connection, _) = socket.accept().unwrap();
            let mut lines = BufReader::new(&connection).lines().map(Result::unwrap);
            assert!(
                lines
                    .next()
                    .unwrap()
                    .starts_with(&format!("GET {expected} "))
            );
            let headers = lines
                .take_while(|l| !l.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
                .to_lowercase();
            if status.starts_with("304") {
                assert!(headers.contains("if-none-match: \"first\""));
            }
            write!(connection, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\nETag: \"{tag}\"\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    let client = Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let Update::Changed(snapshot) = refresh::query(&client, &page, &origin, None).unwrap() else {
        panic!()
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("buildout.json");
    refresh::write_cache(&path, &snapshot).unwrap();
    let persisted = refresh::read_cache(&path).unwrap();
    let mut source = Source::new(false, path.clone());
    source.tick();
    assert!(source.worker.is_none());
    let (tx, _) = mpsc::channel();
    let (cached, _) = load(&path, None, &tx).unwrap();
    assert!(cached.remaining() > Duration::from_secs(86000));
    for expected in ["\"first\"", "\"second\""] {
        let Update::Unchanged(tag) = refresh::query(
            &client,
            &page,
            &origin,
            Some(refresh::validator(&persisted)),
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(tag.as_deref(), Some(expected));
    }
    let Update::Changed(next) = refresh::query(
        &client,
        &page,
        &origin,
        Some(refresh::validator(&persisted)),
    )
    .unwrap() else {
        panic!()
    };
    assert_eq!(next.catalog.campuses.len(), 1);
    assert_eq!(next.catalog.campuses[0].status, "operational");
    source.snapshot = Some(cached.clone());
    source.failed("bad schema".to_owned());
    assert!(source.due > Instant::now() && Arc::ptr_eq(&cached, source.snapshot.as_ref().unwrap()));
    server.join().unwrap();
}

#[test]
#[ignore = "checks the publisher's current bundle over the network"]
fn live_buildout() {
    let Update::Changed(s) = refresh::fetch(None).unwrap() else {
        panic!()
    };
    assert!(s.catalog.valid());
    eprintln!(
        "{} campuses, {} rules ({} geolocated), {} news",
        s.catalog.campuses.len(),
        s.catalog.rules.len(),
        s.catalog.rules.iter().filter(|r| r.lat.is_some()).count(),
        s.catalog.news.len()
    );
}

#[test]
fn earth_markers_open_sourced_details_and_unlocated_rules_remain_browsable() {
    use crate::model::ActiveBody;
    use egui::{Context, LayerId, Pos2, Rect, Shape};
    let mut data = catalog::Catalog::default();
    assert!(data.ingest(&bundle()));
    let snap: Snapshot = serde_json::from_value(serde_json::json!({
        "catalog": data, "checked": 0, "etag": null, "assets": []
    }))
    .unwrap();
    let mut source = Source::new(true, PathBuf::new());
    source.snapshot = Some(Arc::new(snap));
    let ctx = Context::default();
    let point = Pos2::new(50.0, 60.0);
    let _ = ctx.run(Default::default(), |ctx| {
        let painter = ctx
            .layer_painter(LayerId::background())
            .with_clip_rect(Rect::from_min_max(Pos2::ZERO, Pos2::new(200.0, 200.0)));
        let dots = markers::draw(&painter, &source, ActiveBody::Earth, |_| Some(point));
        assert_eq!(dots.len(), 1);
        assert!(markers::draw(&painter, &source, ActiveBody::Moon, |_| Some(point)).is_empty());
        markers::pick(&mut source, &dots, point);
    });
    let texts = |source: &mut Source| {
        let mut output = ctx.run(Default::default(), |ctx| view::window(ctx, source));
        // Floating windows finish their size calculation after the first frame.
        output.shapes.extend(
            ctx.run(Default::default(), |ctx| view::window(ctx, source))
                .shapes,
        );
        output
            .shapes
            .into_iter()
            .filter_map(|s| match s.shape {
                Shape::Text(t) => Some(t.galley.text().to_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let campus = texts(&mut source);
    assert!(campus.contains("O'Brien campus") && campus.contains("city-level location"));
    source.browser.select(view::Kind::Rule, "r");
    let rule = texts(&mut source);
    assert!(rule.contains("Local rule") && rule.contains("proposed"));
    source.browser.select(view::Kind::News, "n");
    let news = texts(&mut source);
    assert!(
        news.contains("Update") && news.contains("Local rule") && news.contains("O'Brien campus")
    );
}
