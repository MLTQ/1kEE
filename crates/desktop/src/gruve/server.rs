//! A tiny std-only HTTP/1.1 server that serves the companion web view and the
//! JSON API the web view polls. Zero extra dependencies — like the Gruve agent
//! itself, everything here is localhost traffic, so `std::net` is plenty.
//!
//! It serves two surfaces on one port (the contract's "one process, one port"
//! shape):
//!   * the UI    — `/`, `/app.js`, `/app.css`, `/gruve-sdk.js`
//!   * the `api` — `/state`, `/events`, `/cameras`, `/tracks`, `/flights`,
//!                 and `POST /command`
//!
//! When opened through a Gruve agent the UI lands at `/apps/1kee/` and the API at
//! `/apps/1kee/__gruve/api/…`; the agent strips both prefixes before proxying here,
//! so the routes below see plain paths either way.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::Sender;

use super::snapshot::{Command, Snapshot};
use crate::model::GeoPoint;

// Static web assets, embedded so the shipped binary needs no asset directory.
// The on-disk copies under `web/` are the `gruve doctor` target.
const INDEX_HTML: &str = include_str!("web/index.html");
const APP_JS: &str = include_str!("web/app.js");
const APP_CSS: &str = include_str!("web/app.css");
const GRUVE_SDK_JS: &str = include_str!("web/gruve-sdk.js");

pub(super) struct ServerShared {
    pub snapshot: Mutex<Snapshot>,
    pub commands: Sender<Command>,
    pub stop: AtomicBool,
}

/// Spawn the accept loop on a background thread. Never blocks or panics into the
/// host app; if the port is taken the caller treats the companion as disabled.
pub(super) fn serve(listener: TcpListener, shared: Arc<ServerShared>) {
    // Non-blocking accept + short poll so `stop` is honoured promptly on shutdown.
    listener
        .set_nonblocking(true)
        .expect("listener nonblocking");
    std::thread::Builder::new()
        .name("gruve-http".into())
        .spawn(move || loop {
            if shared.stop.load(Ordering::Relaxed) {
                return;
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    let s = shared.clone();
                    // One short-lived thread per connection. Traffic is a handful of
                    // viewers polling a few times a second — no pool needed.
                    std::thread::spawn(move || {
                        let _ = handle(stream, &s);
                    });
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(200)),
            }
        })
        .ok();
}

fn handle(mut stream: TcpStream, shared: &ServerShared) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_nonblocking(false)?;

    let (method, path, headers, leftover) = read_request_head(&mut stream)?;
    let (route, query) = match path.split_once('?') {
        Some((r, q)) => (r, q),
        None => (path.as_str(), ""),
    };

    if method == "OPTIONS" {
        return write_response(&mut stream, 204, "text/plain", b"");
    }

    if method == "POST" && route == "/command" {
        let body = read_body(&mut stream, &headers, leftover)?;
        let status = apply_command(&body, shared);
        return write_response(&mut stream, status, "application/json", br#"{"ok":true}"#);
    }

    if method != "GET" {
        return write_response(&mut stream, 405, "text/plain", b"method not allowed");
    }

    // Static UI assets.
    match route {
        "/" | "/index.html" => {
            return write_response(&mut stream, 200, "text/html; charset=utf-8", INDEX_HTML.as_bytes());
        }
        "/app.js" => {
            return write_response(&mut stream, 200, "text/javascript; charset=utf-8", APP_JS.as_bytes());
        }
        "/gruve-sdk.js" => {
            return write_response(&mut stream, 200, "text/javascript; charset=utf-8", GRUVE_SDK_JS.as_bytes());
        }
        "/app.css" => {
            return write_response(&mut stream, 200, "text/css; charset=utf-8", APP_CSS.as_bytes());
        }
        _ => {}
    }

    // JSON API. Each route's slice carries a generation counter (bumped by
    // `publish` only when the slice changes). A poller sends its last-seen value
    // as `?gen=<u64>`; when it matches we answer `304 Not Modified` with no body —
    // skipping serialization entirely. No `gen` param → always a full 200 body, so
    // older clients keep working unchanged.
    let client_gen = query_u64(query, "gen");
    let api = {
        let snap = shared.snapshot.lock().unwrap_or_else(|p| p.into_inner());
        let slice_gen = match route {
            "/state" => Some(snap.gens.state),
            "/events" => Some(snap.gens.events),
            "/cameras" => Some(snap.gens.cameras),
            "/tracks" => Some(snap.gens.tracks),
            "/flights" => Some(snap.gens.flights),
            _ => None,
        };
        slice_gen.map(|g| {
            if client_gen == Some(g) {
                (g, None) // poller is current — no body needed
            } else {
                let body = match route {
                    "/state" => snap.state_json(),
                    "/events" => snap.events_json(),
                    "/cameras" => snap.cameras_json(),
                    "/tracks" => snap.tracks_json(),
                    _ => snap.flights_json(),
                };
                (g, Some(body))
            }
        })
    };

    match api {
        Some((g, Some(body))) => write_api_response(&mut stream, 200, g, body.as_bytes()),
        Some((g, None)) => write_api_response(&mut stream, 304, g, b""),
        None => write_response(&mut stream, 404, "text/plain", b"not found"),
    }
}

/// Pull a `u64` value out of a query string (`a=1&b=2`). No percent-decoding —
/// the only consumer is the numeric `gen` param.
fn query_u64(query: &str, name: &str) -> Option<u64> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        if k == name {
            v.parse().ok()
        } else {
            None
        }
    })
}

/// Parse a `{ "type": ... }` command from a web viewer and forward it to the UI
/// thread. Returns the HTTP status to report.
fn apply_command(body: &[u8], shared: &ServerShared) -> u16 {
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return 400,
    };
    let cmd = match v.get("type").and_then(|t| t.as_str()) {
        Some("focus") => {
            let lat = v.get("lat").and_then(|x| x.as_f64());
            let lon = v.get("lon").and_then(|x| x.as_f64());
            match (lat, lon) {
                (Some(lat), Some(lon)) if lat.is_finite() && lon.is_finite() => Command::Focus {
                    point: GeoPoint {
                        lat: lat.clamp(-90.0, 90.0) as f32,
                        lon: lon.clamp(-180.0, 180.0) as f32,
                    },
                },
                _ => return 400,
            }
        }
        Some("select_event") => match v.get("id").and_then(|x| x.as_str()) {
            Some(id) if !id.is_empty() => Command::SelectEvent { id: id.to_string() },
            _ => return 400,
        },
        _ => return 400,
    };
    // Drop = the UI thread isn't draining; not fatal, just nothing happens.
    let _ = shared.commands.try_send(cmd);
    200
}

// ── Minimal HTTP/1.1 plumbing ──────────────────────────────────────────────

/// Read request bytes until the end of headers; return (method, path, raw header
/// block, any body bytes already read past the header terminator).
fn read_request_head(
    stream: &mut TcpStream,
) -> std::io::Result<(String, String, String, Vec<u8>)> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..pos]).to_string();
            let leftover = buf[pos + 4..].to_vec();
            let mut lines = head.lines();
            let first = lines.next().unwrap_or("");
            let mut parts = first.split_whitespace();
            let method = parts.next().unwrap_or("").to_string();
            let path = parts.next().unwrap_or("/").to_string();
            return Ok((method, path, head, leftover));
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            // Connection closed before full headers.
            return Ok(("".into(), "/".into(), String::new(), Vec::new()));
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > 64 * 1024 {
            // Guard against an unbounded header block.
            return Ok(("".into(), "/".into(), String::new(), Vec::new()));
        }
    }
}

fn read_body(
    stream: &mut TcpStream,
    headers: &str,
    leftover: Vec<u8>,
) -> std::io::Result<Vec<u8>> {
    let len = header_value(headers, "content-length")
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0)
        .min(256 * 1024); // command bodies are tiny; cap defensively
    let mut body = leftover;
    while body.len() < len {
        let mut chunk = [0u8; 4096];
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(len);
    Ok(body)
}

fn header_value<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
    headers.lines().skip(1).find_map(|line| {
        let (k, v) = line.split_once(':')?;
        if k.trim().eq_ignore_ascii_case(name) {
            Some(v.trim())
        } else {
            None
        }
    })
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    write_response_with(stream, status, content_type, "", body)
}

/// JSON API responses carry the slice's generation counter in `X-Gen` (exposed
/// through CORS) so pollers can send it back as `?gen=` and get 304s.
fn write_api_response(
    stream: &mut TcpStream,
    status: u16,
    slice_gen: u64,
    body: &[u8],
) -> std::io::Result<()> {
    let extra = format!("X-Gen: {slice_gen}\r\nAccess-Control-Expose-Headers: X-Gen\r\n");
    write_response_with(stream, status, "application/json", &extra, body)
}

fn write_response_with(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    extra_headers: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        304 => "Not Modified",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "OK",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         {extra_headers}\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
         Access-Control-Allow-Headers: content-type\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}
