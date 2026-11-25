use crate::model::GeoPoint;
use crate::model::seismic::{Quake, SeismicLayer, StreamStatus, quake_severity};
use crossbeam_channel::{Receiver, Sender, TryRecvError, bounded};
use serde_json::Value;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Message, WebSocket};

pub(crate) const SOURCE_URL: &str = "https://www.seismicportal.eu/realtime.html";
const SOCKET_URL: &str = "wss://www.seismicportal.eu/standing_order/websocket";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(1);
const RETRY_DELAY: Duration = Duration::from_secs(30);
const PING_INTERVAL: Duration = Duration::from_secs(15);
const STALE_AFTER: Duration = Duration::from_secs(45);

#[derive(Default)]
pub(crate) struct Stream {
    connection: Option<Connection>,
    retry_at: Option<Instant>,
}

struct Connection {
    inbox: Receiver<Update>,
    // Dropping the sender stops the receiver loop within one read timeout.
    _stop: Sender<()>,
}

enum Update {
    Connected,
    Quake { id: String, quake: Option<Quake> },
}

impl Stream {
    pub fn tick(&mut self, ctx: &egui::Context, layer: &mut SeismicLayer) {
        if !layer.enabled {
            self.connection = None;
            self.retry_at = None;
            layer.stream_status = StreamStatus::Connecting;
            return;
        }

        if let Some(connection) = &self.connection {
            loop {
                match connection.inbox.try_recv() {
                    Ok(Update::Connected) => layer.stream_status = StreamStatus::Live,
                    Ok(Update::Quake { id, quake }) => layer.update_streamed(&id, quake),
                    Err(TryRecvError::Empty) => return,
                    Err(TryRecvError::Disconnected) => break,
                }
            }
            self.connection = None;
            self.retry_at = Some(Instant::now() + RETRY_DELAY);
            layer.stream_status = StreamStatus::Reconnecting;
        }

        if self
            .retry_at
            .is_none_or(|retry_at| Instant::now() >= retry_at)
        {
            let (outbox, inbox) = bounded(128);
            let (stop, stopped) = bounded(0);
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                connect_and_read(&outbox, &stopped, &ctx);
                drop(outbox);
                ctx.request_repaint();
            });
            self.connection = Some(Connection { inbox, _stop: stop });
            layer.stream_status = StreamStatus::Connecting;
        }
    }
}

fn connect_and_read(outbox: &Sender<Update>, stop: &Receiver<()>, ctx: &egui::Context) {
    let Ok(addresses) = ("www.seismicportal.eu", 443).to_socket_addrs() else {
        return;
    };
    let Some(tcp) = addresses
        .take(6)
        .take_while(|_| matches!(stop.try_recv(), Err(TryRecvError::Empty)))
        .find_map(|addr| TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).ok())
    else {
        return;
    };
    if tcp.set_read_timeout(Some(CONNECT_TIMEOUT)).is_err()
        || tcp.set_write_timeout(Some(CONNECT_TIMEOUT)).is_err()
    {
        return;
    }
    let config = WebSocketConfig {
        max_message_size: Some(64 * 1024),
        ..Default::default()
    };
    let Ok((mut socket, _)) =
        tungstenite::client_tls_with_config(SOCKET_URL, tcp, Some(config), None)
    else {
        return;
    };
    let tcp = match socket.get_ref() {
        tungstenite::stream::MaybeTlsStream::Rustls(tls) => tls.get_ref(),
        tungstenite::stream::MaybeTlsStream::Plain(tcp) => tcp,
        _ => return,
    };
    if tcp.set_read_timeout(Some(READ_TIMEOUT)).is_ok() {
        read_updates(&mut socket, outbox, stop, ctx);
    }
}

fn read_updates(
    socket: &mut WebSocket<impl Read + Write>,
    outbox: &Sender<Update>,
    stop: &Receiver<()>,
    ctx: &egui::Context,
) {
    if outbox.try_send(Update::Connected).is_err() {
        return;
    }
    ctx.request_repaint();

    let mut last_frame = Instant::now();
    let mut last_ping = last_frame;
    while matches!(stop.try_recv(), Err(TryRecvError::Empty)) && last_frame.elapsed() < STALE_AFTER
    {
        if last_ping.elapsed() >= PING_INTERVAL {
            if socket.send(Message::Ping(Vec::new())).is_err() {
                break;
            }
            last_ping = Instant::now();
        }
        let frame = match socket.read() {
            Ok(frame) => frame,
            Err(tungstenite::Error::Io(error))
                if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) =>
            {
                continue;
            }
            Err(_) => break,
        };
        last_frame = Instant::now();
        match frame {
            Message::Text(text) => {
                if let Some(update) = parse_update(&text) {
                    if outbox.try_send(update).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    let _ = socket.close(None);
}

fn parse_update(text: &str) -> Option<Update> {
    let root: Value = serde_json::from_str(text).ok()?;
    let data = root.get("data")?;
    let id = data.get("id")?.as_str()?.trim();
    if id.is_empty() {
        return None;
    }
    let id = format!("emsc-{id}");
    match root.get("action")?.as_str()? {
        "delete" => return Some(Update::Quake { id, quake: None }),
        "create" | "update" => {}
        _ => return None,
    }

    let properties = data.get("properties")?;
    let magnitude = properties.get("mag")?.as_f64()?;
    let event_type = properties.get("evtype")?.as_str()?;
    if event_type != "ke" || magnitude < 2.5 {
        return Some(Update::Quake { id, quake: None });
    }
    let geometry = data.get("geometry")?;
    if geometry.get("type")?.as_str()? != "Point" {
        return None;
    }
    let coordinates = geometry.get("coordinates")?.as_array()?;
    let lon = coordinates.first()?.as_f64()?;
    let lat = coordinates.get(1)?.as_f64()?;
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }
    let origin = chrono::DateTime::parse_from_rfc3339(properties.get("time")?.as_str()?).ok()?;
    let region = properties
        .get("flynn_region")
        .and_then(Value::as_str)
        .unwrap_or("Earthquake");
    let quake = Quake {
        id: id.clone(),
        title: format!("M{magnitude:.1} · {region}"),
        epicenter: GeoPoint {
            lat: lat as f32,
            lon: lon as f32,
        },
        origin_ms: origin.timestamp_millis(),
        severity: quake_severity(Some(magnitude)),
        has_station_picks: false,
    };
    Some(Update::Quake {
        id,
        quake: Some(quake),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor};
    use tungstenite::protocol::Role;

    const MESSAGE: &str = r#"{
        "action":"create",
        "data": {
            "id":"20250305_0000090",
            "geometry":{"type":"Point","coordinates":[28.5381,40.115,-7.0]},
            "properties":{"time":"2025-03-05T09:45:07.125Z","flynn_region":"WESTERN TURKEY","evtype":"ke","mag":4.6}
        }
    }"#;

    struct FixtureSocket(Cursor<Vec<u8>>);

    impl Read for FixtureSocket {
        fn read(&mut self, dst: &mut [u8]) -> io::Result<usize> {
            self.0.read(dst)
        }
    }

    impl Write for FixtureSocket {
        fn write(&mut self, src: &[u8]) -> io::Result<usize> {
            Ok(src.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn receives_and_normalizes_a_push_message() {
        let mut server = WebSocket::from_raw_socket(Cursor::new(Vec::new()), Role::Server, None);
        server.send(Message::Text(MESSAGE.to_owned())).unwrap();
        server.close(None).unwrap();
        let frames = std::mem::take(server.get_mut().get_mut());
        let mut socket =
            WebSocket::from_raw_socket(FixtureSocket(Cursor::new(frames)), Role::Client, None);
        let (tx, rx) = bounded(2);
        let (_stop, stopped) = bounded(0);
        read_updates(&mut socket, &tx, &stopped, &egui::Context::default());
        assert!(matches!(rx.try_recv(), Ok(Update::Connected)));
        let Update::Quake {
            id,
            quake: Some(quake),
        } = rx.try_recv().unwrap()
        else {
            panic!("missing quake update")
        };
        assert_eq!(id, "emsc-20250305_0000090");
        assert_eq!(quake.origin_ms % 1000, 125);
        assert_eq!(quake.epicenter.lat, 40.115);
        assert_eq!(quake.epicenter.lon, 28.5381);
        assert_eq!(quake.severity, crate::model::EventSeverity::Elevated);
    }

    #[test]
    fn removes_a_quake_revised_below_the_layer_threshold() {
        let update = MESSAGE.replace("create", "update").replace("4.6", "2.4");
        assert!(matches!(
            parse_update(&update),
            Some(Update::Quake { quake: None, .. })
        ));
        assert!(parse_update(&MESSAGE.replace("40.115", "91.0")).is_none());
    }
}
